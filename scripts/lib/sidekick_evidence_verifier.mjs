import { readFileSync } from "node:fs";
import { assertReasoningBackend } from "./sidekick_provider.mjs";

export const EVIDENCE_VERDICT_SCHEMA = {
  type: "object",
  additionalProperties: false,
  properties: {
    decision: { type: "string", enum: ["allow", "reject"] },
    reason_code: {
      type: "string",
      enum: [
        "supported",
        "unsupported_fact",
        "unsupported_visual",
        "contradiction",
        "incomplete_material_consequence",
        "uncertain",
      ],
    },
  },
  required: [
    "decision",
    "reason_code",
  ],
};

const BASE_INSTRUCTIONS = readFileSync(
  new URL("../../resources/live_sidekick/verifier_base_instructions.txt", import.meta.url),
  "utf8",
);
const DEVELOPER_INSTRUCTIONS = readFileSync(
  new URL("../../resources/live_sidekick/verifier_developer_instructions.txt", import.meta.url),
  "utf8",
);

function parseVerdict(result) {
  const verdict = JSON.parse(String(result.text ?? "").trim());
  if (
    !["allow", "reject"].includes(verdict.decision) ||
    ![
      "supported",
      "unsupported_fact",
      "unsupported_visual",
      "contradiction",
      "incomplete_material_consequence",
      "uncertain",
    ].includes(verdict.reason_code)
  ) {
    throw new Error("evidence verifier returned an invalid verdict");
  }
  const allowed =
    verdict.decision === "allow" &&
    verdict.reason_code === "supported";
  return {
    ...verdict,
    allowed,
    latency: {
      first_token_ms: result.firstTokenMs,
      total_ms: result.totalMs,
    },
  };
}

const AUTOMATION_DECISION =
  /\b(?:automat\w*|ai[- ]handled|ai[- ]resolved|agent[- ]handled|agent[- ]resolved|model[- ]handled|model[- ]resolved)\b/i;
const HUMAN_ALTERNATIVE =
  /\b(?:humans?|human[- ]in[- ]the[- ]loop|human (?:handling|review)|manual(?:ly| handling| review)?|people|support (?:team|reps?|agents?)|specialists?|operators?)\b/i;
const BINARY_DECISION =
  /\b(?:decid\w*|decision|between|versus|vs\.?|either|instead|alternative|ship|launch|roll\s*out|deploy|keep)\b/i;
const STAKEHOLDER_PROTECTION_REQUEST =
  /\b(?:procurement|customer[- ]side|buyer|contract(?:ual)?|protection|remed(?:y|ies)|audit rights?|service[- ]level|sla)\b/i;
const CONFIDENCE_GATE =
  /\b(?:confidence[- ](?:gated|thresholded|segmented)|confidence (?:gate|threshold|band|cutoff)|(?:gate|threshold|segment)\w*[^.!?\n]{0,35}confidence|(?:high|low)[- ]confidence|(?:above|below)[- ]threshold)\b/i;
const HUMAN_FALLBACK = [
  /\bconfidence[- ]gated\s+automation\b[^.!?\n]{0,35}\bwith\s+(?:a\s+)?human\s+fallback\b/i,
  /\b(?:rout(?:e|ing)|send|return|revert|switch|leave|keep|retain)\w*\b[^.!?\n]{0,60}\b(?:uncertain|low[- ]confidence|below[- ]threshold|below\s+(?:the\s+)?(?:confidence\s+)?threshold|remaining|remainder|rest|balance)\b[^.!?\n]{0,45}\b(?:humans?|human (?:handling|review)|people|support (?:team|reps?|agents?)|specialists?|operators?)\b/i,
  /\b(?:uncertain|low[- ]confidence|below[- ]threshold|below\s+(?:the\s+)?(?:confidence\s+)?threshold|remaining|remainder|rest|balance)\b[^.!?\n]{0,50}\b(?:rout(?:e|ing)|send|return|revert|switch|leave|keep|retain)\w*\b[^.!?\n]{0,45}\b(?:humans?|human (?:handling|review)|people|support (?:team|reps?|agents?)|specialists?|operators?)\b/i,
  /\b(?:humans?|human (?:handling|review)|people|support (?:team|reps?|agents?)|specialists?|operators?)\b[^.!?\n]{0,45}\b(?:below|outside|under|for)\b[^.!?\n]{0,35}\b(?:confidence|threshold|cutoff|uncertain|remainder|rest)\b/i,
  /\b(?:automat\w*|ship|launch|deploy)\b[^.!?\n]{0,45}\bonly\b[^.!?\n]{0,30}\b(?:confident|high[- ]confidence|above[- ]threshold)\b[^.!?\n]{0,55}\b(?:leave|keep|route|send|return)\b[^.!?\n]{0,35}\b(?:everything else|the rest|the remainder|the balance)\b[^.!?\n]{0,30}\b(?:humans?|people|support (?:team|reps?|agents?)|specialists?|operators?)\b/i,
];

/**
 * Minutes-owned deterministic publication rule for a narrow, material
 * automation disposition. Broader factual and semantic checks remain the
 * responsibility of the provider-neutral evidence verifier.
 */
export function deterministicEvidenceRejection({
  candidate,
  transcriptEvidence = [],
  authoritativeContext = null,
}) {
  const evidenceText = transcriptEvidence
    .map((item) => String(item?.text ?? ""))
    .join("\n");
  const authorityText = String(authoritativeContext?.typed_user_message ?? "");
  const decisionContext = `${evidenceText}\n${authorityText}`;
  const candidateText = String(candidate?.text ?? "");
  if (STAKEHOLDER_PROTECTION_REQUEST.test(authorityText)) return null;
  const framesHumanAsAutomationAlternative =
    AUTOMATION_DECISION.test(decisionContext) &&
    HUMAN_ALTERNATIVE.test(decisionContext) &&
    BINARY_DECISION.test(decisionContext);
  const proposesConfidenceGate =
    AUTOMATION_DECISION.test(candidateText) &&
    CONFIDENCE_GATE.test(candidateText);
  const statesHumanDisposition =
    HUMAN_FALLBACK.some((pattern) => pattern.test(candidateText));

  if (
    framesHumanAsAutomationAlternative &&
    proposesConfidenceGate &&
    !statesHumanDisposition
  ) {
    return "incomplete_material_consequence";
  }
  return null;
}

/** Separate provider-neutral semantic evidence check run before publication. */
export class BackendEvidenceVerifier {
  constructor({ backendFactory, shutdownGraceMs = 2_500 }) {
    if (typeof backendFactory !== "function") {
      throw new Error("evidence verifier requires a fresh backend factory");
    }
    this.backendFactory = backendFactory;
    this.shutdownGraceMs = shutdownGraceMs;
    this.started = false;
    this.closed = false;
    this.cwd = null;
    this.readyPromise = null;
    this.activeBackend = null;
    this.preparingBackend = null;
    this.sessionsStarted = 0;
    this.verificationReceipts = [];
  }

  async start({ cwd = process.cwd() } = {}) {
    if (this.started) throw new Error("evidence verifier already started");
    this.cwd = cwd;
    this.closed = false;
    this.readyPromise = this.#beginPreparingSlot();
    const slot = await this.#takePreparedSlot();
    this.started = true;
    return slot.session;
  }

  #beginPreparingSlot() {
    return this.#prepareSlot().then(
      (slot) => ({ slot, error: null }),
      (error) => ({ slot: null, error }),
    );
  }

  async #takePreparedSlot() {
    const prepared = await this.readyPromise;
    if (prepared.error) throw prepared.error;
    return prepared.slot;
  }

  async #prepareSlot() {
    const backend = assertReasoningBackend(await this.backendFactory());
    this.preparingBackend = backend;
    if (this.closed) {
      backend.close();
      if (this.preparingBackend === backend) this.preparingBackend = null;
      throw new Error("evidence verifier is closed");
    }
    try {
      const session = await backend.startSession({
        cwd: this.cwd,
        baseInstructions: BASE_INSTRUCTIONS,
        developerInstructions: DEVELOPER_INSTRUCTIONS,
      });
      this.sessionsStarted += 1;
      // Pay the provider's first-turn cold-start before a live intervention is
      // waiting. The slot may retain only this synthetic payload before its one
      // real candidate; it is closed immediately afterward.
      await this.#verifyWithBackend(backend, {
        candidate: {
          decision: "speak",
          text: "The verifier warmup is ready.",
          evidence_ids: ["synthetic-warmup"],
          visual_evidence_ids: [],
          claims_visual_observation: false,
        },
        transcriptEvidence: [{
          id: "synthetic-warmup",
          text: "The verifier warmup is ready.",
        }],
        screenEvidence: null,
        authoritativeContext: { synthetic_warmup: true },
      });
      if (this.closed) throw new Error("evidence verifier is closed");
      if (this.preparingBackend === backend) this.preparingBackend = null;
      return { backend, session };
    } catch (error) {
      backend.close();
      if (this.preparingBackend === backend) this.preparingBackend = null;
      throw error;
    }
  }

  async verify({
    candidate,
    transcriptEvidence,
    screenEvidence,
    authoritativeContext = null,
    reasoningDepth = "realtime",
  }) {
    if (!this.started) throw new Error("evidence verifier has not started");
    if (this.closed) throw new Error("evidence verifier is closed");
    if (this.activeBackend) throw new Error("evidence verifier already has an active candidate");
    const slot = await this.#takePreparedSlot();
    if (this.closed) {
      slot.backend.close();
      throw new Error("evidence verifier is closed");
    }
    this.activeBackend = slot.backend;
    // Replenish immediately with another synthetic-only slot. The current
    // slot will never see another real evidence window.
    this.readyPromise = this.#beginPreparingSlot();
    try {
      const deterministicReason = deterministicEvidenceRejection({
        candidate,
        transcriptEvidence,
        authoritativeContext,
      });
      const verdict = deterministicReason
        ? {
            decision: "reject",
            reason_code: deterministicReason,
            allowed: false,
            latency: {
              first_token_ms: 0,
              total_ms: 0,
            },
          }
        : await this.#verifyWithBackend(slot.backend, {
            candidate,
            transcriptEvidence,
            screenEvidence,
            authoritativeContext,
            reasoningDepth,
          });
      this.verificationReceipts.push({
        session_id: slot.session.sessionId ?? null,
        decision: verdict.decision,
        reason_code: verdict.reason_code,
      });
      return verdict;
    } finally {
      slot.backend.close();
      if (this.activeBackend === slot.backend) this.activeBackend = null;
    }
  }

  async #verifyWithBackend(backend, {
    candidate,
    transcriptEvidence,
    screenEvidence,
    authoritativeContext = null,
    reasoningDepth = "realtime",
  }) {
    const input = [
      {
        type: "text",
        text: `BEGIN UNTRUSTED EVIDENCE AND CANDIDATE\n${JSON.stringify(
          { authoritative_context: authoritativeContext, transcript_evidence: transcriptEvidence, candidate },
        )}\nEND UNTRUSTED EVIDENCE AND CANDIDATE`,
      },
    ];
    if (screenEvidence) {
      input.push(
        {
          type: "text",
          text: `The following exact-session image has visual_evidence_id=${JSON.stringify(screenEvidence.id)}.`,
        },
        { type: "image", path: screenEvidence.path, detail: "high" },
      );
    }
    const started = await backend.startTurn({
      input,
      outputSchema: EVIDENCE_VERDICT_SCHEMA,
      latencyClass: "fast",
      reasoningDepth,
    });
    const result = await started.completion;
    if (result.status !== "completed" || result.error) {
      throw new Error(result.error?.message ?? `evidence verifier ended as ${result.status}`);
    }
    return parseVerdict(result);
  }

  async close() {
    if (this.closed) return;
    this.closed = true;
    this.started = false;
    this.activeBackend?.close();
    this.activeBackend = null;
    this.preparingBackend?.close();
    this.preparingBackend = null;
    let deadline;
    await Promise.race([
      this.#takePreparedSlot()
        .then((slot) => slot?.backend?.close())
        .catch(() => {}),
      new Promise((resolve) => {
        deadline = setTimeout(resolve, this.shutdownGraceMs);
      }),
    ]);
    clearTimeout(deadline);
    this.readyPromise = null;
  }
}

export function assertEvidenceVerifier(verifier) {
  if (typeof verifier?.verify !== "function") {
    throw new Error("Sidekick requires an independent evidence verifier");
  }
  return verifier;
}
