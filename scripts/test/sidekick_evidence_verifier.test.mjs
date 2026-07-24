import assert from "node:assert/strict";
import test from "node:test";
import {
  BackendEvidenceVerifier,
  deterministicEvidenceRejection,
} from "../lib/sidekick_evidence_verifier.mjs";

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

const automationDecisionEvidence = [
  {
    id: "decision",
    text: "We must decide between full automation and keeping a human in the loop.",
  },
];

test("deterministic policy rejects a confidence gate that strands uncertain work", () => {
  assert.equal(
    deterministicEvidenceRejection({
      candidate: {
        text: "Stage automated resolution behind a confidence gate. Which confidence band should launch?",
      },
      transcriptEvidence: automationDecisionEvidence,
      authoritativeContext: {
        typed_user_message: "What is the safest decision?",
      },
    }),
    "incomplete_material_consequence",
  );
});

test("deterministic policy allows a confidence gate with an explicit human disposition", () => {
  for (const text of [
    "Ship confidence-gated automation with human handling below threshold.",
    "Automate only the high-confidence queue and send the balance to the support team.",
    "Launch above the confidence threshold; route uncertain work to specialists.",
    "Stage automated resolution by confidence, routing below-threshold tickets to humans.",
    "Full automation creates $800k/month contractual exposure (4,000 wrong × $200); 90% accuracy stops being decisive. Stage a confidence gate: automate only high-confidence tickets; route all others to humans.",
    "Full automation creates $800k/month contractual exposure: 4,000 wrong resolutions × $200 Meridian credit. The 90% headline stops deciding; stage only high-confidence tickets and route all other tickets to humans.",
  ]) {
    assert.equal(
      deterministicEvidenceRejection({
        candidate: { text },
        transcriptEvidence: automationDecisionEvidence,
      }),
      null,
      text,
    );
  }
});

test("deterministic policy does not broaden into unrelated confidence advice", () => {
  assert.equal(
    deterministicEvidenceRejection({
      candidate: {
        text: "Use a confidence band when negotiating the renewal discount.",
      },
      transcriptEvidence: [{
        id: "negotiation",
        text: "A human account lead is deciding whether to offer a renewal discount.",
      }],
    }),
    null,
  );
});

test("deterministic policy leaves procurement completeness to the semantic verifier", () => {
  assert.equal(
    deterministicEvidenceRejection({
      candidate: {
        text: "For Meridian, require a confidence-threshold SLA, case-level audit records, and a unilateral right to revert affected work to humans.",
      },
      transcriptEvidence: automationDecisionEvidence,
      authoritativeContext: {
        typed_user_message: "Advise me as Meridian's procurement lead. What protections do I need?",
      },
    }),
    null,
  );
});

test("deterministic rejection retains a unique verifier receipt without model inference", async () => {
  const factory = verifierFactory();
  const verifier = new BackendEvidenceVerifier({ backendFactory: factory.create });
  await verifier.start();
  const verdict = await verifier.verify({
    candidate: {
      text: "Stage automated resolution behind a confidence gate.",
    },
    transcriptEvidence: automationDecisionEvidence,
    screenEvidence: null,
  });
  assert.equal(verdict.allowed, false);
  assert.equal(verdict.reason_code, "incomplete_material_consequence");
  assert.equal(verdict.latency.total_ms, 0);
  assert.equal(factory.backends[0].turns.length, 1);
  assert.deepEqual(
    verifier.verificationReceipts.map(({ session_id }) => session_id),
    ["verifier-1"],
  );
  await verifier.close();
});

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

test("close bounds a wedged verifier preparation after closing its backend", async () => {
  let resolveStart;
  const backend = new FakeBackend({ id: "wedged" });
  backend.startSession = () =>
    new Promise((resolve) => {
      resolveStart = resolve;
    });
  const verifier = new BackendEvidenceVerifier({
    backendFactory: () => backend,
    shutdownGraceMs: 5,
  });
  const starting = verifier.start();
  await new Promise((resolve) => setImmediate(resolve));
  await verifier.close();
  assert.equal(backend.closed, true);
  resolveStart({ sessionId: "late-session" });
  await assert.rejects(starting, /closed/);
});
