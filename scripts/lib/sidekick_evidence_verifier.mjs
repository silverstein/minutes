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

/** Separate provider-neutral semantic evidence check run before publication. */
export class BackendEvidenceVerifier {
  constructor({ backendFactory }) {
    if (typeof backendFactory !== "function") {
      throw new Error("evidence verifier requires a fresh backend factory");
    }
    this.backendFactory = backendFactory;
    this.started = false;
    this.closed = false;
    this.cwd = null;
    this.readyPromise = null;
    this.activeBackend = null;
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
    if (this.closed) {
      backend.close();
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
      return { backend, session };
    } catch (error) {
      backend.close();
      throw error;
    }
  }

  async verify({ candidate, transcriptEvidence, screenEvidence, authoritativeContext = null }) {
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
      const verdict = await this.#verifyWithBackend(slot.backend, {
        candidate,
        transcriptEvidence,
        screenEvidence,
        authoritativeContext,
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
    try {
      const slot = await this.#takePreparedSlot();
      slot?.backend?.close();
    } catch {
      // Preparation may have been interrupted by shutdown.
    }
    this.readyPromise = null;
  }
}

export function assertEvidenceVerifier(verifier) {
  if (typeof verifier?.verify !== "function") {
    throw new Error("Sidekick requires an independent evidence verifier");
  }
  return verifier;
}
