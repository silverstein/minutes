import assert from "node:assert/strict";
import test from "node:test";
import {
  semanticJudgeCriteria,
  SidekickSemanticJudge,
} from "../lib/sidekick_semantic_judge.mjs";

function completedVerdict(overrides = {}) {
  const turn = (criteria) => Object.fromEntries([
    ...criteria.map((criterion) => [criterion, true]),
    ["reason", "meets the explicit rubric"],
  ]);
  return {
    turn_1: turn(semanticJudgeCriteria.turn_1),
    turn_2: turn(semanticJudgeCriteria.turn_2),
    overall_pass: true,
    overall_reason: "all criteria pass",
    ...overrides,
  };
}

class FakeBackend {
  constructor(verdict) {
    this.verdict = verdict;
    this.session = null;
    this.turn = null;
    this.closed = false;
  }

  async startSession(config) {
    this.session = config;
    return { sessionId: "judge-session", provider: "fake" };
  }

  async startTurn(turn) {
    this.turn = turn;
    return {
      turnId: "judge-turn",
      completion: Promise.resolve({
        status: "completed",
        text: JSON.stringify(this.verdict),
        firstTokenMs: 20,
        totalMs: 40,
      }),
    };
  }

  async steerTurn() {}
  async interruptTurn() {}
  close() { this.closed = true; }
}

const fixture = {
  prepared_context: {},
  transcript: [{ sequence: 1, text: "evidence" }],
  turns: [
    { id: "one", typed_prompt: "one", required_behaviors: ["one"] },
    { id: "two", typed_prompt: "two", required_behaviors: ["two"] },
  ],
  forbidden_behaviors: ["contradictions"],
};

test("semantic judge passes only a structurally consistent all-true verdict", async () => {
  const backend = new FakeBackend(completedVerdict());
  const judge = new SidekickSemanticJudge({ backend });
  await judge.start({ cwd: "/tmp" });
  const result = await judge.grade({ fixture, responses: { turn_1: "a", turn_2: "b" } });
  assert.equal(result.passed, true);
  assert.equal(result.latency.total_ms, 40);
  assert.deepEqual(result.judge_receipt, {
    session_id: "judge-session",
    turn_id: "judge-turn",
  });
  assert.match(backend.turn.input[0].text, /BEGIN UNTRUSTED EVAL DATA/);
  assert.equal(backend.turn.outputSchema.additionalProperties, false);
  judge.close();
  assert.equal(backend.closed, true);
});

test("semantic judge fails closed when one natural-language criterion fails", async () => {
  const verdict = completedVerdict();
  verdict.turn_2.unilateral_human_reversion = false;
  verdict.turn_2.reason = "supplier approval still controls handback";
  verdict.overall_pass = false;
  const judge = new SidekickSemanticJudge({ backend: new FakeBackend(verdict) });
  await judge.start();
  const result = await judge.grade({ fixture, responses: { turn_1: "a", turn_2: "b" } });
  assert.equal(result.computed_pass, false);
  assert.equal(result.passed, false);
});

test("semantic judge rejects a self-inconsistent overall pass", async () => {
  const verdict = completedVerdict();
  verdict.turn_1.no_contradiction = false;
  const judge = new SidekickSemanticJudge({ backend: new FakeBackend(verdict) });
  await judge.start();
  const result = await judge.grade({ fixture, responses: { turn_1: "a", turn_2: "b" } });
  assert.equal(result.computed_pass, false);
  assert.equal(result.passed, false);
});

test("semantic calibration compares predictions to hidden human labels", async () => {
  const backend = new FakeBackend({
    results: [
      { id: "natural", predicted_pass: true, reason: "valid paraphrase" },
      { id: "veto", predicted_pass: false, reason: "vendor veto reverses control" },
    ],
  });
  const judge = new SidekickSemanticJudge({ backend });
  await judge.start();
  const result = await judge.calibrate({
    fixture,
    examples: [
      { id: "natural", expectedPass: true, responses: { turn_1: "a", turn_2: "b" } },
      { id: "veto", expectedPass: false, responses: { turn_1: "a", turn_2: "bad" } },
    ],
  });
  assert.equal(result.passed, true);
  assert.equal(result.accuracy, 1);
  assert.equal(backend.turn.outputSchema.properties.results.minItems, 2);
});
