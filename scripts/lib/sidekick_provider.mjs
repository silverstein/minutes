import { readFileSync } from "node:fs";

export const CODEX_REALTIME_MODEL = readFileSync(
  new URL("../../resources/live_sidekick/codex_realtime_model.txt", import.meta.url),
  "utf8",
).trim();

export const CODEX_REALTIME_EFFORT = readFileSync(
  new URL("../../resources/live_sidekick/codex_realtime_effort.txt", import.meta.url),
  "utf8",
).trim();

export const CODEX_VERIFIER_MODEL = readFileSync(
  new URL("../../resources/live_sidekick/codex_verifier_model.txt", import.meta.url),
  "utf8",
).trim();

/**
 * Vendor-neutral persistent reasoning backend contract used by Sidekick.
 *
 * Minutes owns evidence, policy, session state, corrections, and publication.
 * Backends receive only a bounded turn input and return a streamed reasoning
 * result. Codex app-server is one adapter; it is not part of the engine.
 */
export function assertReasoningBackend(backend) {
  for (const method of [
    "startSession",
    "startTurn",
    "steerTurn",
    "interruptTurn",
    "close",
  ]) {
    if (typeof backend?.[method] !== "function") {
      throw new Error(`Sidekick reasoning backend is missing ${method}()`);
    }
  }
  return backend;
}

function codexInput(item) {
  if (item.type === "text") {
    return { type: "text", text: item.text, text_elements: [] };
  }
  if (item.type === "image") {
    return { type: "localImage", path: item.path, detail: item.detail ?? "high" };
  }
  throw new Error(`Unsupported Sidekick backend input type: ${item.type}`);
}

/** Codex app-server implementation of the vendor-neutral backend contract. */
export class CodexAppServerBackend {
  constructor(
    client,
    { model = CODEX_REALTIME_MODEL, reasoningEffort = CODEX_REALTIME_EFFORT } = {},
  ) {
    this.client = client;
    this.model = model;
    this.reasoningEffort = reasoningEffort;
    this.threadId = null;
  }

  async startSession({ cwd, baseInstructions, developerInstructions }) {
    await this.client.start();
    const { threadId, result } = await this.client.startThread({
      cwd,
      approvalPolicy: "never",
      sandbox: "read-only",
      serviceTier: "fast",
      ephemeral: true,
      ...(this.model ? { model: this.model } : {}),
      baseInstructions,
      developerInstructions,
    });
    this.threadId = threadId;
    return {
      sessionId: threadId,
      provider: "codex-app-server",
      model: result.model ?? null,
      serviceTier: result.serviceTier ?? null,
    };
  }

  async startTurn({ input, outputSchema, latencyClass = "fast" }) {
    this.#assertStarted();
    const started = await this.client.startTurn({
      threadId: this.threadId,
      input: input.map(codexInput),
      outputSchema,
      serviceTier: latencyClass,
      effort: this.reasoningEffort,
    });
    return {
      turnId: started.turnId,
      completion: started.completion.then((result) => ({
        status: result.status,
        error: result.error,
        text: result.text,
        firstTokenMs: result.firstDeltaMs,
        totalMs: result.totalMs,
      })),
    };
  }

  steerTurn({ turnId, input }) {
    this.#assertStarted();
    return this.client.steerTurn({
      threadId: this.threadId,
      turnId,
      input: input.map(codexInput),
    });
  }

  interruptTurn({ turnId }) {
    this.#assertStarted();
    return this.client.interruptTurn({ threadId: this.threadId, turnId });
  }

  close() {
    this.client.close();
  }

  #assertStarted() {
    if (!this.threadId) throw new Error("Codex Sidekick backend has not started");
  }
}
