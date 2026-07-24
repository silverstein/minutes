import { readFileSync } from "node:fs";
import path from "node:path";
import { assertReasoningBackend } from "./sidekick_provider.mjs";
import { assertEvidenceVerifier } from "./sidekick_evidence_verifier.mjs";

const INTERVENTION_CONTRACT = JSON.parse(readFileSync(
  new URL("../../resources/live_sidekick/intervention_contract.json", import.meta.url),
  "utf8",
));

export const SIDEKICK_OUTPUT_SCHEMA = {
  type: "object",
  additionalProperties: false,
  properties: {
    decision: { type: "string", enum: ["silent", "speak"] },
    kind: {
      type: ["string", "null"],
      enum: ["insight", "question", "risk", "opening", "answer", "strategy", null],
    },
    text: {
      type: ["string", "null"],
      description: INTERVENTION_CONTRACT.text_description,
    },
    evidence_ids: {
      type: "array",
      items: { type: "string" },
      description:
        "Exact transcript evidence IDs supporting every visible factual claim, number, remedy condition, and fallback control. Include all distinct items needed for a synthesis; do not cite an item merely because it is available. If recommending human reversion when the evidence contains a human-in-loop versus automation decision, cite that decision item as well as any contract-remedy item.",
    },
    visual_evidence_ids: { type: "array", items: { type: "string" } },
    claims_visual_observation: {
      type: "boolean",
      description:
        "True iff the visible response relies on pixels from the supplied exact-session image; false otherwise.",
    },
    confidence: { type: "integer", minimum: 0, maximum: 100 },
  },
  required: [
    "decision",
    "kind",
    "text",
    "evidence_ids",
    "visual_evidence_ids",
    "claims_visual_observation",
    "confidence",
  ],
};

export function sidekickOutputSchemaFor(mode) {
  if (!["background", "foreground"].includes(mode)) {
    throw new Error(`Unsupported Sidekick turn mode: ${mode}`);
  }
  // A background turn can be promoted in place through provider steering, so
  // every started turn needs enough schema headroom for a foreground answer.
  // Minutes still enforces the stricter 50-word background publication limit.
  const maxLength = INTERVENTION_CONTRACT.max_characters;
  const targetWords = mode === "background"
    ? INTERVENTION_CONTRACT.background_target_words
    : INTERVENTION_CONTRACT.foreground_target_words;
  return {
    ...SIDEKICK_OUTPUT_SCHEMA,
    properties: {
      ...SIDEKICK_OUTPUT_SCHEMA.properties,
      text: {
        ...SIDEKICK_OUTPUT_SCHEMA.properties.text,
        maxLength,
        description: `${SIDEKICK_OUTPUT_SCHEMA.properties.text.description} Target at most ${targetWords} words and never exceed ${maxLength} characters for this ${mode} turn.`,
      },
    },
  };
}

const BASE_INSTRUCTIONS = readFileSync(
  new URL("../../resources/live_sidekick/base_instructions.txt", import.meta.url),
  "utf8",
);

const DEVELOPER_INSTRUCTIONS = readFileSync(
  new URL("../../resources/live_sidekick/developer_instructions.txt", import.meta.url),
  "utf8",
);

function textInput(text) {
  return { type: "text", text };
}

function stableJson(value) {
  return JSON.stringify(value, null, 2);
}

function parseDecision(raw) {
  const trimmed = String(raw ?? "").trim();
  const candidate = trimmed.startsWith("```")
    ? trimmed
        .replace(/^```(?:json)?\s*/i, "")
        .replace(/\s*```$/, "")
        .trim()
    : trimmed;
  const parsed = JSON.parse(candidate);
  if (!parsed || !["silent", "speak"].includes(parsed.decision)) {
    throw new Error("Sidekick response has no valid decision");
  }
  if (!Array.isArray(parsed.evidence_ids) || !Array.isArray(parsed.visual_evidence_ids)) {
    throw new Error("Sidekick response has invalid evidence provenance");
  }
  if (typeof parsed.claims_visual_observation !== "boolean") {
    throw new Error("Sidekick response has no visual-observation declaration");
  }
  if (!Number.isInteger(parsed.confidence) || parsed.confidence < 0 || parsed.confidence > 100) {
    throw new Error("Sidekick response has invalid confidence");
  }
  if (parsed.decision === "speak" && !String(parsed.text ?? "").trim()) {
    throw new Error("Sidekick chose speak without text");
  }
  return parsed;
}

function visibleWordCount(text) {
  return String(text ?? "").trim().match(/\S+/g)?.length ?? 0;
}

function terminalCore(word) {
  return String(word ?? "").replace(/["'”’\)\]\}]+$/gu, "");
}

function endsSentence(word) {
  return /[.!?]$/u.test(terminalCore(word));
}

function sentenceIsQuestion(sentence) {
  const lastWord = String(sentence ?? "").trim().match(/\S+$/u)?.[0] ?? "";
  const punctuation = terminalCore(lastWord).match(/[.!?]+$/u)?.[0] ?? "";
  return punctuation.includes("?");
}

function hasFragmentBoundary(word) {
  return /[,:;.!?]$/u.test(terminalCore(word));
}

function truncateFragment(text, maximumWords, terminal) {
  const words = String(text ?? "").trim().match(/\S+/g) ?? [];
  if (words.length <= maximumWords) return words.join(" ");
  const selected = words.slice(0, maximumWords);
  let boundary = -1;
  for (let index = selected.length - 1; index >= Math.floor(selected.length / 2); index -= 1) {
    if (hasFragmentBoundary(selected[index])) {
      boundary = index;
      break;
    }
  }
  const kept = selected.slice(0, boundary >= 0 ? boundary + 1 : selected.length);
  let compacted = kept.join(" ").replace(/[,:;.!?"'”’\)\]\}]+$/gu, "");
  compacted += terminal === "?" ? "?" : "…";
  return compacted;
}

export function compactVisibleText(text, maximumWords) {
  const normalized = String(text ?? "").trim().replace(/\s+/g, " ");
  if (visibleWordCount(normalized) <= maximumWords) return normalized;
  const sentences = [];
  let current = [];
  for (const word of normalized.match(/\S+/gu) ?? []) {
    current.push(word);
    if (endsSentence(word)) {
      sentences.push(current.join(" "));
      current = [];
    }
  }
  if (current.length > 0) sentences.push(current.join(" "));
  const questionIndex = sentences.findLastIndex(sentenceIsQuestion);
  const question = questionIndex >= 0 ? sentences[questionIndex] : null;
  const questionText = question
    ? truncateFragment(question, Math.min(maximumWords, visibleWordCount(question)), "?")
    : null;
  const questionWords = visibleWordCount(questionText);
  const bodyBudget = Math.max(0, maximumWords - questionWords);
  const bodyCandidates = questionIndex >= 0 ? sentences.slice(0, questionIndex) : sentences;
  const selected = [];
  let selectedWords = 0;
  for (const sentence of bodyCandidates) {
    const count = visibleWordCount(sentence);
    if (selectedWords + count > bodyBudget) break;
    selected.push(sentence);
    selectedWords += count;
  }
  if (selected.length === 0 && bodyBudget > 0 && bodyCandidates[0]) {
    selected.push(truncateFragment(bodyCandidates[0], bodyBudget, "…"));
  }
  const compacted = [...selected, ...(questionText ? [questionText] : [])]
    .filter(Boolean)
    .join(" ")
    .trim();
  return compacted || truncateFragment(normalized, maximumWords, sentenceIsQuestion(normalized) ? "?" : "…");
}

function referencesVisualDetail(text) {
  return /\b(?:on (?:the|your) screen|according to (?:the|your) (?:deck|screen|slide|page|window|chart|graph|table|spreadsheet|diagram)|(?:the|your) (?:deck|screen|slide|page|window|chart|graph|table|spreadsheet|diagram)\b|i (?:can )?see|visible (?:on|in))\b/i.test(
    String(text ?? ""),
  );
}

export class SidekickSession {
  constructor({
    backend,
    evidenceVerifier,
    captureSessionId,
    brief = {},
    minimumProactiveConfidence = 70,
    maxTranscriptChars = 6_000,
    shutdownGraceMs = 2_500,
    onPublish = () => {},
    now = () => performance.now(),
  }) {
    assertReasoningBackend(backend);
    this.evidenceVerifier = assertEvidenceVerifier(evidenceVerifier ?? backend);
    if (!String(captureSessionId ?? "").trim()) {
      throw new Error("SidekickSession requires a capture session id");
    }
    this.backend = backend;
    this.captureSessionId = captureSessionId;
    this.brief = structuredClone(brief);
    this.contextEvidence = Array.isArray(this.brief.evidence)
      ? this.brief.evidence.map((item) => structuredClone(item))
      : [];
    delete this.brief.evidence;
    this.minimumProactiveConfidence = minimumProactiveConfidence;
    this.maxTranscriptChars = maxTranscriptChars;
    this.shutdownGraceMs = shutdownGraceMs;
    this.onPublish = onPublish;
    this.now = now;
    this.backendSessionId = null;
    this.stopped = false;
    this.transcript = [];
    this.screens = new Map();
    this.latestScreenId = null;
    this.active = null;
    this.nextInvocation = 1;
    this.userGeneration = 0;
    this.evidenceRevision = 0;
    this.transcriptRevision = 0;
    this.screenRevision = 0;
    this.lastProactiveRevision = -1;
    this.trace = [];
  }

  async start({ cwd = process.cwd() } = {}) {
    const result = await this.backend.startSession({
      cwd,
      baseInstructions: BASE_INSTRUCTIONS,
      developerInstructions: DEVELOPER_INSTRUCTIONS,
    });
    this.backendSessionId = result.sessionId;
    this.stopped = false;
    this.#record("session_started", {
      backend_session_id: result.sessionId,
      provider: result.provider ?? null,
      model: result.model ?? null,
      service_tier: result.serviceTier ?? null,
    });
    return result;
  }

  observeTranscript(evidence) {
    this.#assertExactCapture(evidence.captureSessionId);
    const id = String(evidence.id ?? "").trim();
    const text = String(evidence.text ?? "").trim();
    if (!id || !text) throw new Error("Transcript evidence requires id and text");
    if (this.transcript.some((item) => item.id === id)) return false;
    const item = {
      id,
      speaker: String(evidence.speaker ?? "unverified speaker"),
      text,
      offset_ms: Number(evidence.offsetMs ?? 0),
      duration_ms: Number(evidence.durationMs ?? 0),
    };
    this.transcript.push(item);
    this.evidenceRevision += 1;
    this.transcriptRevision += 1;
    this.#record("transcript_observed", { evidence_id: id });
    return true;
  }

  observeScreen(evidence) {
    this.#assertExactCapture(evidence.captureSessionId);
    const id = String(evidence.id ?? "").trim();
    const imagePath = String(evidence.path ?? "").trim();
    if (!id || !imagePath || !path.isAbsolute(imagePath)) {
      throw new Error("Screen evidence requires an id and absolute path");
    }
    this.screens.set(id, { id, path: imagePath });
    this.latestScreenId = id;
    this.evidenceRevision += 1;
    this.screenRevision += 1;
    this.#record("screen_observed", { evidence_id: id });
  }

  async evaluateProactive() {
    this.#assertStarted();
    if (this.active) {
      this.#record("background_suppressed_busy", { active_mode: this.active.mode });
      return null;
    }
    if (this.lastProactiveRevision === this.evidenceRevision) {
      this.#record("background_suppressed_no_new_evidence", {
        evidence_revision: this.evidenceRevision,
      });
      return null;
    }
    return this.#startInference({ mode: "background", typedMessage: null });
  }

  async sendUser(message) {
    this.#assertStarted();
    const text = String(message ?? "").trim();
    if (!text) throw new Error("Typed user message cannot be empty");
    this.userGeneration += 1;

    if (this.active) {
      const invocation = this.active;
      invocation.supersededByUserGeneration = this.userGeneration;
      if (invocation.stage === "generating") {
        const turn = this.#turnInput({ mode: "foreground", typedMessage: text });
        try {
          await this.backend.steerTurn({
            turnId: invocation.turnId,
            input: turn.input,
          });
          // Promotion is committed only after the provider acknowledges the
          // steer. A background completion racing the acknowledgement remains
          // background work and cannot impersonate a foreground answer.
          if (this.active !== invocation) {
            return this.#startInference({ mode: "foreground", typedMessage: text });
          }
          invocation.mode = "foreground";
          invocation.typedMessage = text;
          invocation.userGeneration = this.userGeneration;
          invocation.evidenceRevision = this.evidenceRevision;
          invocation.generationEvidenceRevision = this.evidenceRevision;
          invocation.freshnessRetry = 0;
          invocation.completenessRetry = 0;
          invocation.verificationRetry = 0;
          invocation.policyFeedback = null;
          invocation.carriedTotalMs = 0;
          invocation.initialFirstTokenMs = null;
          invocation.supersededByUserGeneration = null;
          invocation.allowedVisualIds = turn.visualIds;
          invocation.allowedEvidenceIds = turn.evidenceIds;
          invocation.transcriptEvidence = turn.transcriptEvidence;
          invocation.screenEvidence = turn.screenEvidence;
          invocation.authoritativeContext = turn.authoritativeContext;
          this.#record("foreground_steered", {
            invocation: invocation.id,
            turn_id: invocation.turnId,
            user_generation: this.userGeneration,
          });
          return invocation.completion;
        } catch (error) {
          this.#record("foreground_steer_missed", {
            invocation: invocation.id,
            error: String(error),
          });
          if (this.active === invocation) this.active = null;
          try {
            await this.backend.interruptTurn({
              turnId: invocation.turnId,
            });
          } catch {
            // A completed turn needs no interruption. The completion path below
            // still rejects stale publication by invocation identity.
          }
        }
      } else {
        // Verification is isolated from the strategist backend and cannot be
        // steered. Its eventual verdict is rejected by the user generation.
        if (this.active === invocation) this.active = null;
        this.#record("foreground_superseded_verification", { invocation: invocation.id });
      }
    }

    return this.#startInference({ mode: "foreground", typedMessage: text });
  }

  async stop() {
    this.stopped = true;
    const active = this.active;
    this.active = null;
    let interruption = null;
    if (active) {
      try {
        interruption = this.backend.interruptTurn({
          turnId: active.turnId,
        });
      } catch {
        // Best-effort shutdown.
      }
    }
    // Close immediately so a wedged protocol request cannot hold shutdown open
    // for the full provider timeout. Any in-flight interruption is best effort.
    this.backend.close();
    if (interruption) {
      let deadline;
      await Promise.race([
        Promise.resolve(interruption).catch(() => {}),
        new Promise((resolve) => {
          deadline = setTimeout(resolve, this.shutdownGraceMs);
        }),
      ]);
      clearTimeout(deadline);
    }
    this.#record("session_stopped", {});
  }

  async #startInference({
    mode,
    typedMessage,
    freshnessRetry = 0,
    completenessRetry = 0,
    verificationRetry = 0,
    policyFeedback = null,
    carriedTotalMs = 0,
    initialFirstTokenMs = null,
  }) {
    const invocation = {
      id: this.nextInvocation++,
      mode,
      userGeneration: this.userGeneration,
      evidenceRevision: this.evidenceRevision,
      generationEvidenceRevision: this.evidenceRevision,
      verifiedEvidenceRevision: null,
      freshnessRetry,
      completenessRetry,
      verificationRetry,
      policyFeedback,
      carriedTotalMs,
      initialFirstTokenMs,
      typedMessage,
      supersededByUserGeneration: null,
      stage: "generating",
      turnId: null,
      allowedEvidenceIds: new Set(),
      allowedVisualIds: new Set(),
      transcriptEvidence: [],
      screenEvidence: null,
      authoritativeContext: null,
      verificationRefreshes: 0,
      completion: null,
    };
    const turn = this.#turnInput({ mode, typedMessage, policyFeedback });
    invocation.allowedEvidenceIds = turn.evidenceIds;
    invocation.allowedVisualIds = turn.visualIds;
    invocation.transcriptEvidence = turn.transcriptEvidence;
    invocation.screenEvidence = turn.screenEvidence;
    invocation.authoritativeContext = turn.authoritativeContext;
    const started = await this.backend.startTurn({
      input: turn.input,
      outputSchema: sidekickOutputSchemaFor(mode),
    });
    invocation.turnId = started.turnId;
    this.active = invocation;
    this.#record(`${mode}_started`, {
      invocation: invocation.id,
      turn_id: invocation.turnId,
      user_generation: invocation.userGeneration,
    });
    invocation.completion = started.completion.then(
      (result) => this.#complete(invocation, result),
      (error) => this.#fail(invocation, error),
    );
    return invocation.completion;
  }

  #turnInput({ mode, typedMessage, policyFeedback = null }) {
    const transcript = this.#boundedTranscript();
    const prompt = {
      turn_mode: mode,
      prepared_context: this.brief,
      bounded_context_evidence: this.contextEvidence,
      exact_capture_session_id: this.captureSessionId,
      bounded_transcript_evidence: transcript,
      response_rule:
        mode === "foreground"
          ? "The typed user message is authoritative and must receive a direct speak response."
          : "No user message is waiting. Choose silent unless intervention is materially useful now.",
    };
    const input = [
      textInput(
        `BEGIN UNTRUSTED MEETING DATA\n${stableJson(prompt)}\nEND UNTRUSTED MEETING DATA`,
      ),
    ];
    if (typedMessage) {
      input.push(textInput(`AUTHORITATIVE TYPED USER MESSAGE\n${typedMessage}`));
    }
    if (policyFeedback) {
      input.push(textInput(`MINUTES PUBLICATION POLICY FEEDBACK\n${policyFeedback}`));
    }
    const visualIds = new Set();
    const screen = this.latestScreenId ? this.screens.get(this.latestScreenId) : null;
    if (screen) {
      visualIds.add(screen.id);
      input.push(
        textInput(
          `The following exact-session image has visual_evidence_id=${JSON.stringify(screen.id)}.`,
        ),
        { type: "image", path: screen.path, detail: "high" },
      );
    }
    return {
      input,
      evidenceIds: new Set([
        ...this.contextEvidence.map((item) => item.id),
        ...transcript.map((item) => item.id),
      ]),
      visualIds,
      transcriptEvidence: transcript,
      screenEvidence: screen,
      authoritativeContext: {
        prepared_context: this.brief,
        context_evidence: this.contextEvidence,
        typed_user_message: typedMessage ?? null,
      },
    };
  }

  #boundedTranscript() {
    const selected = [];
    let characters = 0;
    for (let index = this.transcript.length - 1; index >= 0; index -= 1) {
      const item = this.transcript[index];
      const itemCharacters = item.text.length + item.speaker.length;
      if (selected.length > 0 && characters + itemCharacters > this.maxTranscriptChars) break;
      if (itemCharacters > this.maxTranscriptChars) {
        selected.push({ ...item, text: item.text.slice(-this.maxTranscriptChars) });
        break;
      }
      selected.push(item);
      characters += itemCharacters;
    }
    return selected.reverse();
  }

  async #complete(invocation, result) {
    if (
      this.stopped ||
      this.active !== invocation ||
      invocation.userGeneration !== this.userGeneration ||
      invocation.supersededByUserGeneration !== null
    ) {
      if (this.active === invocation) this.active = null;
      this.#record("stale_completion_rejected", { invocation: invocation.id });
      return { published: false, stale: true, result };
    }
    let decision;
    try {
      if (result.status !== "completed" || result.error) {
        throw new Error(result.error?.message ?? `Reasoning turn ended as ${result.status}`);
      }
      decision = parseDecision(result.text);
      const maxWords = invocation.mode === "background" ? 50 : 70;
      if (decision.decision === "speak" && visibleWordCount(decision.text) > maxWords) {
        const originalWords = visibleWordCount(decision.text);
        decision = {
          ...decision,
          text: compactVisibleText(decision.text, maxWords),
        };
        this.#record("visible_response_compacted", {
          invocation: invocation.id,
          mode: invocation.mode,
          original_words: originalWords,
          published_words: visibleWordCount(decision.text),
        });
      }
      this.#validateProvenance(invocation, decision);
      if (invocation.mode === "foreground" && decision.decision !== "speak") {
        throw new Error("Foreground response chose silence");
      }
      if (decision.decision === "speak") {
        invocation.stage = "verifying";
        let verdict;
        let verificationTotalMs = 0;
        for (;;) {
          const verificationWindow = this.#turnInput({
            mode: invocation.mode,
            typedMessage: invocation.typedMessage,
            policyFeedback: invocation.policyFeedback,
          });
          const sealedEvidenceRevision = this.evidenceRevision;
          const sealedTranscriptRevision = this.transcriptRevision;
          const sealedScreenRevision = this.screenRevision;
          verdict = await this.evidenceVerifier.verify({
            candidate: decision,
            transcriptEvidence: verificationWindow.transcriptEvidence,
            screenEvidence: decision.claims_visual_observation
              ? verificationWindow.screenEvidence
              : null,
            authoritativeContext: verificationWindow.authoritativeContext,
          });
          verificationTotalMs += verdict.latency?.total_ms ?? 0;
          if (
            this.stopped ||
            this.active !== invocation ||
            this.userGeneration !== invocation.userGeneration ||
            invocation.supersededByUserGeneration !== null
          ) {
            if (this.active === invocation) this.active = null;
            this.#record("stale_verification_rejected", { invocation: invocation.id });
            return { published: false, stale: true, result };
          }
          const transcriptChanged = sealedTranscriptRevision !== this.transcriptRevision;
          const relevantScreenChanged =
            decision.claims_visual_observation && sealedScreenRevision !== this.screenRevision;
          if (
            (transcriptChanged || relevantScreenChanged) &&
            invocation.verificationRefreshes === 0
          ) {
            invocation.verificationRefreshes += 1;
            this.#record("stale_evidence_reverify", {
              invocation: invocation.id,
              from_revision: sealedEvidenceRevision,
              to_revision: this.evidenceRevision,
            });
            continue;
          }
          if (transcriptChanged || relevantScreenChanged) {
            this.#record("bounded_verification_lag", {
              invocation: invocation.id,
              verified_revision: sealedEvidenceRevision,
              current_revision: this.evidenceRevision,
            });
          }
          invocation.verifiedEvidenceRevision = sealedEvidenceRevision;
          break;
        }
        if (!verdict.allowed) {
          this.#record("evidence_verification_rejected", {
            invocation: invocation.id,
            reason_code: verdict.reason_code,
            candidate: decision,
            verification_total_ms: verdict.latency?.total_ms ?? null,
          });
          if (invocation.verifiedEvidenceRevision !== invocation.generationEvidenceRevision) {
            return this.#restartForFreshEvidence(
              invocation,
              result,
              "fresh_verifier_rejected_stale_candidate",
              verificationTotalMs,
            );
          }
          if (
            invocation.mode === "foreground" &&
            (
              (verdict.reason_code === "incomplete_material_consequence" &&
                invocation.completenessRetry === 0) ||
              (verdict.reason_code !== "incomplete_material_consequence" &&
                invocation.verificationRetry === 0)
            )
          ) {
            return this.#restartForCompleteness(
              invocation,
              result,
              verificationTotalMs,
              verdict.reason_code,
            );
          }
          throw new Error(`Independent evidence verification rejected the response: ${verdict.reason_code}`);
        }
        this.#record("evidence_verified", {
          invocation: invocation.id,
          generation_total_ms: result.totalMs ?? null,
          verification_total_ms: verificationTotalMs,
        });
        result = {
          ...result,
          firstTokenMs: invocation.initialFirstTokenMs ?? result.firstTokenMs,
          totalMs:
            invocation.carriedTotalMs +
            (result.totalMs ?? 0) +
            verificationTotalMs,
        };
      }
    } catch (error) {
      if (this.active === invocation) this.active = null;
      this.#record("invalid_model_response", {
        invocation: invocation.id,
        mode: invocation.mode,
        error: String(error),
      });
      return { published: false, invalid: true, error: String(error), result };
    }

    if (
      this.stopped ||
      this.active !== invocation ||
      this.userGeneration !== invocation.userGeneration ||
      invocation.supersededByUserGeneration !== null
    ) {
      if (this.active === invocation) this.active = null;
      this.#record("stale_verification_rejected", { invocation: invocation.id });
      return { published: false, stale: true, result };
    }

    if (invocation.mode === "background") {
      this.lastProactiveRevision = invocation.verifiedEvidenceRevision ?? invocation.evidenceRevision;
    }
    if (this.active === invocation) this.active = null;

    const publish =
      decision.decision === "speak" &&
      (invocation.mode === "foreground" || decision.confidence >= this.minimumProactiveConfidence);
    if (publish) {
      const publication = {
        mode: invocation.mode,
        invocation: invocation.id,
        user_generation: invocation.userGeneration,
        decision,
        latency: {
          first_token_ms: result.firstTokenMs,
          total_ms: result.totalMs,
        },
      };
      this.onPublish(publication);
      this.#record("published", publication);
      return { published: true, publication, result };
    }
    this.#record("silenced", {
      invocation: invocation.id,
      mode: invocation.mode,
      confidence: decision.confidence,
    });
    return { published: false, decision, result };
  }

  #restartForFreshEvidence(invocation, result, boundary, verificationTotalMs) {
    if (
      this.stopped ||
      this.active !== invocation ||
      this.userGeneration !== invocation.userGeneration ||
      invocation.supersededByUserGeneration !== null
    ) {
      if (this.active === invocation) this.active = null;
      this.#record("stale_completion_rejected", { invocation: invocation.id });
      return { published: false, stale: true, result };
    }
    this.active = null;
    this.#record("stale_evidence_restart", {
      invocation: invocation.id,
      boundary,
      from_revision: invocation.evidenceRevision,
      to_revision: this.evidenceRevision,
      retry: invocation.freshnessRetry + 1,
    });
    if (invocation.freshnessRetry >= 2) {
      this.#record("stale_evidence_retry_exhausted", { invocation: invocation.id });
      return { published: false, stale: true, retry_exhausted: true, result };
    }
    return this.#startInference({
      mode: invocation.mode,
      typedMessage: invocation.typedMessage,
      freshnessRetry: invocation.freshnessRetry + 1,
      completenessRetry: invocation.completenessRetry,
      verificationRetry: invocation.verificationRetry,
      policyFeedback: invocation.policyFeedback,
      carriedTotalMs:
        invocation.carriedTotalMs +
        (result.totalMs ?? 0) +
        verificationTotalMs,
      initialFirstTokenMs: invocation.initialFirstTokenMs ?? result.firstTokenMs,
    });
  }

  #restartForCompleteness(
    invocation,
    result,
    verificationTotalMs,
    reasonCode = "incomplete_material_consequence",
  ) {
    if (
      this.stopped ||
      this.active !== invocation ||
      this.userGeneration !== invocation.userGeneration ||
      invocation.supersededByUserGeneration !== null
    ) {
      if (this.active === invocation) this.active = null;
      this.#record("stale_completion_rejected", { invocation: invocation.id });
      return { published: false, stale: true, result };
    }
    this.active = null;
    this.#record(
      reasonCode === "incomplete_material_consequence"
        ? "material_completeness_retry"
        : "semantic_verification_retry",
      {
        invocation: invocation.id,
        retry: reasonCode === "incomplete_material_consequence"
          ? invocation.completenessRetry + 1
          : invocation.verificationRetry + 1,
        reason_code: reasonCode,
      },
    );
    return this.#startInference({
      mode: invocation.mode,
      typedMessage: invocation.typedMessage,
      freshnessRetry: invocation.freshnessRetry,
      completenessRetry: invocation.completenessRetry +
        Number(reasonCode === "incomplete_material_consequence"),
      verificationRetry: invocation.verificationRetry +
        Number(reasonCode !== "incomplete_material_consequence"),
      policyFeedback: reasonCode === "incomplete_material_consequence"
        ? "The prior candidate omitted a relevant explicitly evidenced material consequence required by the user's request. Re-read the bounded evidence and produce a complete answer without inventing or broadening terms."
        : "The prior candidate did not pass independent evidence verification. Re-read the exact bounded evidence, recompute every material claim, remove unsupported or contradictory statements, and answer the user's request fully without inventing facts.",
      carriedTotalMs:
        invocation.carriedTotalMs +
        (result.totalMs ?? 0) +
        verificationTotalMs,
      initialFirstTokenMs: invocation.initialFirstTokenMs ?? result.firstTokenMs,
    });
  }

  #fail(invocation, error) {
    if (this.active === invocation) this.active = null;
    this.#record("inference_failed", { invocation: invocation.id, error: String(error) });
    return { published: false, failed: true, error: String(error) };
  }

  #validateProvenance(invocation, decision) {
    for (const id of decision.evidence_ids) {
      if (!invocation.allowedEvidenceIds.has(id)) {
        throw new Error(`Response cited unavailable transcript evidence ${id}`);
      }
    }
    for (const id of decision.visual_evidence_ids) {
      if (!invocation.allowedVisualIds.has(id)) {
        throw new Error(`Response cited unavailable visual evidence ${id}`);
      }
    }
    if (decision.claims_visual_observation === (decision.visual_evidence_ids.length === 0)) {
      throw new Error("Response visual-observation declaration does not match its receipts");
    }
    if (decision.visual_evidence_ids.length === 0 && referencesVisualDetail(decision.text)) {
      throw new Error("Response made a visual claim without inspected image provenance");
    }
    if (
      decision.decision === "speak" &&
      decision.evidence_ids.length === 0 &&
      decision.visual_evidence_ids.length === 0
    ) {
      throw new Error("Visible Sidekick response requires exact-session evidence provenance");
    }
  }

  #assertExactCapture(captureSessionId) {
    if (captureSessionId !== this.captureSessionId) {
      throw new Error(
        `Evidence belongs to capture ${captureSessionId}; active capture is ${this.captureSessionId}`,
      );
    }
  }

  #assertStarted() {
    if (!this.backendSessionId || this.stopped) {
      throw new Error("Sidekick session has not started");
    }
  }

  #record(type, detail) {
    this.trace.push({ sequence: this.trace.length + 1, at_ms: this.now(), type, ...detail });
  }
}
