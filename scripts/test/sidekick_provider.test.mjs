import assert from "node:assert/strict";
import test from "node:test";
import {
  assertReasoningBackend,
  CodexAppServerBackend,
} from "../lib/sidekick_provider.mjs";

class FakeAppServerClient {
  constructor() {
    this.started = null;
    this.turn = null;
    this.steer = null;
    this.interrupt = null;
    this.closed = false;
  }

  async start() {}

  async startThread(params) {
    this.started = params;
    return {
      threadId: "thread-a",
      result: { model: "codex-model", serviceTier: "priority" },
    };
  }

  async startTurn(params) {
    this.turn = params;
    return {
      turnId: "turn-a",
      completion: Promise.resolve({
        status: "completed",
        error: null,
        text: "answer",
        firstDeltaMs: 12,
        totalMs: 34,
      }),
    };
  }

  async steerTurn(params) {
    this.steer = params;
    return { turnId: params.turnId };
  }

  async interruptTurn(params) {
    this.interrupt = params;
  }

  close() {
    this.closed = true;
  }
}

test("the Codex adapter contains protocol-specific names behind the generic backend", async () => {
  const client = new FakeAppServerClient();
  const backend = new CodexAppServerBackend(client);
  assertReasoningBackend(backend);
  const session = await backend.startSession({
    cwd: "/tmp",
    baseInstructions: "base",
    developerInstructions: "developer",
  });
  assert.equal(session.provider, "codex-app-server");
  assert.equal(client.started.sandbox, "read-only");

  const turn = await backend.startTurn({
    input: [
      { type: "text", text: "hello" },
      { type: "image", path: "/tmp/screen.png", detail: "high" },
    ],
    outputSchema: { type: "object" },
  });
  assert.deepEqual(client.turn.input, [
    { type: "text", text: "hello", text_elements: [] },
    { type: "localImage", path: "/tmp/screen.png", detail: "high" },
  ]);
  assert.deepEqual(await turn.completion, {
    status: "completed",
    error: null,
    text: "answer",
    firstTokenMs: 12,
    totalMs: 34,
  });

  await backend.steerTurn({ turnId: "turn-a", input: [{ type: "text", text: "steer" }] });
  assert.equal(client.steer.threadId, "thread-a");
  await backend.interruptTurn({ turnId: "turn-a" });
  assert.equal(client.interrupt.turnId, "turn-a");
  backend.close();
  assert.equal(client.closed, true);
});

test("an incomplete backend is rejected before a session can start", () => {
  assert.throws(() => assertReasoningBackend({}), /missing startSession/);
});

