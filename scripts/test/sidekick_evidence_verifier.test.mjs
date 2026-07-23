import assert from "node:assert/strict";
import test from "node:test";
import { BackendEvidenceVerifier } from "../lib/sidekick_evidence_verifier.mjs";

const supported = { decision: "allow", reason_code: "supported" };

class FakeBackend {
  constructor({ id, verdicts = [supported, supported] }) {
    this.id = id;
    this.verdicts = [...verdicts];
    this.turns = [];
    this.closed = false;
  }

  async startSession() {
    return { sessionId: `verifier-${this.id}` };
  }

  async startTurn(params) {
    this.turns.push(params);
    const verdict = this.verdicts.shift() ?? supported;
    return {
      turnId: `verify-${this.id}-${this.turns.length}`,
      completion: Promise.resolve({
        status: "completed",
        text: JSON.stringify(verdict),
        firstTokenMs: 10,
        totalMs: 20,
      }),
    };
  }

  async steerTurn() {}
  async interruptTurn() {}
  close() { this.closed = true; }
}

function verifierFactory(configure = () => [supported, supported]) {
  const backends = [];
  return {
    backends,
    create: () => {
      const id = backends.length + 1;
      const backend = new FakeBackend({ id, verdicts: configure(id) });
      backends.push(backend);
      return backend;
    },
  };
}

test("verifier fails closed on an allow verdict with unsupported claims", async () => {
  const factory = verifierFactory((id) => id === 1
    ? [supported, { decision: "allow", reason_code: "unsupported_fact" }]
    : [supported, supported]);
  const verifier = new BackendEvidenceVerifier({ backendFactory: factory.create });
  await verifier.start();
  const verdict = await verifier.verify({
    candidate: { text: "They approved $1M." },
    transcriptEvidence: [{ id: "weather", text: "Nice weather." }],
    screenEvidence: null,
  });
  assert.equal(verdict.allowed, false);
  assert.equal(factory.backends[0].turns[1].outputSchema.additionalProperties, false);
  assert.match(factory.backends[0].turns[1].input[0].text, /Nice weather/);
  assert.equal(factory.backends[0].closed, true);
  await verifier.close();
});

test("verifier accepts an internally consistent supported verdict", async () => {
  const factory = verifierFactory();
  const verifier = new BackendEvidenceVerifier({ backendFactory: factory.create });
  await verifier.start();
  const verdict = await verifier.verify({
    candidate: { text: "Four thousand times $200 is $800K." },
    transcriptEvidence: [{ id: "math", text: "Four thousand wrong at $200 each." }],
    screenEvidence: null,
  });
  assert.equal(verdict.allowed, true);
  assert.equal(verdict.latency.total_ms, 20);
  assert.equal(verifier.sessionsStarted, 2);
  await verifier.close();
});

test("meeting evidence never persists into the next candidate verifier slot", async () => {
  const factory = verifierFactory((id) => id === 2
    ? [supported, { decision: "reject", reason_code: "unsupported_fact" }]
    : [supported, supported]);
  const verifier = new BackendEvidenceVerifier({ backendFactory: factory.create });
  await verifier.start();

  const first = await verifier.verify({
    candidate: { text: "The $1M commitment is approved." },
    transcriptEvidence: [{ id: "old-approval", text: "The $1M commitment is approved." }],
    screenEvidence: null,
  });
  assert.equal(first.allowed, true);
  assert.equal(factory.backends[0].closed, true);

  const second = await verifier.verify({
    candidate: { text: "The $1M commitment is approved." },
    transcriptEvidence: [{ id: "weather", text: "Nice weather." }],
    screenEvidence: null,
  });
  assert.equal(second.allowed, false);
  assert.notEqual(factory.backends[0], factory.backends[1]);
  assert.doesNotMatch(factory.backends[1].turns[1].input[0].text, /old-approval/);
  assert.match(factory.backends[1].turns[1].input[0].text, /Nice weather/);
  assert.equal(factory.backends[1].closed, true);
  await verifier.close();
});
