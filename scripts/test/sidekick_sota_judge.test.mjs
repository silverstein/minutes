import assert from "node:assert/strict";
import test from "node:test";
import {
  loadSidekickSotaFixtures,
} from "../lib/sidekick_sota_fixture.mjs";
import {
  parseSidekickSotaVerdict,
  SidekickSotaJudge,
  sidekickSotaJudgeSchema,
} from "../lib/sidekick_sota_judge.mjs";

const FIXTURE_DIRECTORY = new URL(
  "../../tests/fixtures/sidekick_sota/v1/",
  import.meta.url,
);

class FakeBackend {
  constructor(verdict) {
    this.verdict = verdict;
    this.session = null;
    this.turn = null;
    this.turns = [];
    this.closed = false;
  }

  async startSession(config) {
    this.session = config;
    return { sessionId: "sota-judge-session", provider: "fake" };
  }

  async startTurn(turn) {
    this.turn = turn;
    this.turns.push(turn);
    return {
      turnId: "sota-judge-turn",
      completion: Promise.resolve({
        status: "completed",
        text: JSON.stringify(this.verdict),
        firstTokenMs: 12,
        totalMs: 27,
      }),
    };
  }

  async steerTurn() {}
  async interruptTurn() {}
  close() { this.closed = true; }
}

function passingVerdict(fixture) {
  return {
    turns: Object.fromEntries(
      fixture.turns.map((turn) => [
        turn.id,
        {
          criteria: Object.fromEntries(
            turn.rubric.map((criterion) => [criterion.id, true]),
          ),
          no_forbidden_behavior: true,
          evidence_supported: true,
          concise_direct: true,
          reason: "All fixture-specific criteria pass.",
        },
      ]),
    ),
    overall_pass: true,
    overall_reason: "Every turn passes.",
  };
}

async function fixtureById(id) {
  const fixtures = await loadSidekickSotaFixtures(FIXTURE_DIRECTORY);
  return fixtures.find(({ fixture }) => fixture.id === id).fixture;
}

test("the generic schema is generated from each fixture's own rubric", async () => {
  const fixture = await fixtureById("synthetic-runway-hiring-tradeoff");
  const schema = sidekickSotaJudgeSchema(fixture);
  const criteria =
    schema.properties.turns.properties.runway_decision.properties.criteria;
  assert.deepEqual(
    criteria.required,
    fixture.turns[0].rubric.map(({ id }) => id),
  );
  assert.ok(criteria.properties.compute_current_runway);
  assert.equal(schema.additionalProperties, false);
});

test("the judge separates candidate-visible context from its synthetic privacy oracle", async () => {
  const fixture = await fixtureById("synthetic-restricted-board-injection");
  const backend = new FakeBackend(passingVerdict(fixture));
  const judge = new SidekickSotaJudge({ backend });
  await judge.start({ cwd: "/tmp" });
  const result = await judge.grade({
    fixture,
    responses: {
      protect_restricted_context: {
        decision: "speak",
        text: "Keep this to information available in this meeting.",
        evidence_ids: ["utterance_one"],
      },
    },
  });

  assert.equal(result.passed, true);
  const payload = backend.turn.input[0].text;
  assert.match(payload, /evaluator_only_forbidden_context/);
  assert.match(payload, /succession contingency/i);
  const parsedPayload = JSON.parse(
    payload
      .replace(/^BEGIN UNTRUSTED SOTA EVAL DATA\n/, "")
      .replace(/\nEND UNTRUSTED SOTA EVAL DATA$/, ""),
  );
  assert.deepEqual(parsedPayload.prepared_context.evidence, []);
  assert.equal(parsedPayload.evaluator_only_forbidden_context.length, 1);
  assert.deepEqual(
    parsedPayload.turn.forbidden_evidence_ids,
    ["restricted_board_artifact"],
  );
  assert.match(
    backend.session.developerInstructions,
    /never disclosed to the candidate/,
  );
  assert.equal(result.latency.total_ms, 27);
});

test("the judge sees only transcript evidence available through each turn cutoff", async () => {
  const fixture = await fixtureById("synthetic-incident-rollback-threshold");
  fixture.turns[0].transcript_through_id = "utterance_two";
  fixture.turns[0].required_evidence_ids = ["utterance_one", "utterance_two"];
  const backend = new FakeBackend(passingVerdict(fixture));
  const judge = new SidekickSotaJudge({ backend });
  await judge.start();
  await judge.grade({
    fixture,
    responses: {
      containment_decision: {
        decision: "speak",
        text: "Use the known error boundary.",
        evidence_ids: ["utterance_one", "utterance_two"],
      },
    },
  });
  const payload = JSON.parse(
    backend.turn.input[0].text
      .replace(/^BEGIN UNTRUSTED SOTA EVAL DATA\n/, "")
      .replace(/\nEND UNTRUSTED SOTA EVAL DATA$/, ""),
  );
  assert.deepEqual(
    payload.turn.transcript_evidence.map(({ id }) => id),
    ["utterance_one", "utterance_two"],
  );
});

test("multi-turn judging seals each verdict before any future transcript is disclosed", async () => {
  const fixture = await fixtureById("synthetic-incident-rollback-threshold");
  const laterTurn = structuredClone(fixture.turns[0]);
  fixture.turns[0].transcript_through_id = "utterance_two";
  fixture.turns[0].required_evidence_ids = ["utterance_one", "utterance_two"];
  laterTurn.id = "later_decision";
  laterTurn.transcript_through_id = "utterance_four";
  fixture.turns.push(laterTurn);
  const backend = new FakeBackend(passingVerdict(fixture));
  const judge = new SidekickSotaJudge({ backend });
  await judge.start();
  await judge.grade({
    fixture,
    responses: {
      containment_decision: {
        decision: "speak",
        text: "Contain the known error increase.",
        evidence_ids: ["utterance_one", "utterance_two"],
      },
      later_decision: {
        decision: "speak",
        text: "Use the reversible path.",
        evidence_ids: ["utterance_one", "utterance_two", "utterance_three"],
      },
    },
  });
  assert.equal(backend.turns.length, 2);
  const firstPayload = backend.turns[0].input[0].text;
  const secondPayload = backend.turns[1].input[0].text;
  assert.doesNotMatch(firstPayload, /later_decision/);
  assert.doesNotMatch(firstPayload, /Do we roll back or keep going/);
  assert.match(secondPayload, /later_decision/);
  assert.match(secondPayload, /Do we roll back or keep going/);
});

test("one failed scenario-specific criterion fails the whole verdict", async () => {
  const fixture = await fixtureById("synthetic-incident-rollback-threshold");
  const verdict = passingVerdict(fixture);
  verdict.turns.containment_decision.criteria.preserve_queued_jobs = false;
  verdict.turns.containment_decision.reason =
    "The response strands queued jobs.";
  verdict.overall_pass = false;
  const parsed = parseSidekickSotaVerdict(fixture, JSON.stringify(verdict));
  assert.equal(parsed.computed_pass, false);
  assert.equal(parsed.passed, false);
});

test("an inconsistent overall pass fails closed", async () => {
  const fixture = await fixtureById("synthetic-runway-hiring-tradeoff");
  const verdict = passingVerdict(fixture);
  verdict.turns.runway_decision.evidence_supported = false;
  const parsed = parseSidekickSotaVerdict(fixture, JSON.stringify(verdict));
  assert.equal(parsed.computed_pass, false);
  assert.equal(parsed.passed, false);
});

test("a missing dynamic rubric result is rejected", async () => {
  const fixture = await fixtureById("synthetic-runway-hiring-tradeoff");
  const verdict = passingVerdict(fixture);
  delete verdict.turns.runway_decision.criteria.compute_hiring_runway;
  assert.throws(
    () => parseSidekickSotaVerdict(fixture, JSON.stringify(verdict)),
    /omitted runway_decision\.compute_hiring_runway/,
  );
});
