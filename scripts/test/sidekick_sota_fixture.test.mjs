import assert from "node:assert/strict";
import test from "node:test";
import {
  disclosedSidekickContext,
  evaluatorOnlyForbiddenContext,
  loadSidekickSotaFixtures,
  scoreSidekickSotaResponses,
  validateSidekickSotaFixture,
} from "../lib/sidekick_sota_fixture.mjs";

const FIXTURE_DIRECTORY = new URL(
  "../../tests/fixtures/sidekick_sota/v1/",
  import.meta.url,
);

async function corpus() {
  return loadSidekickSotaFixtures(FIXTURE_DIRECTORY);
}

test("the SOTA corpus is synthetic, broad, and schema-valid", async () => {
  const fixtures = await corpus();
  assert.equal(fixtures.length, 7);
  assert.ok(new Set(fixtures.map(({ fixture }) => fixture.domain)).size >= 6);
  assert.ok(
    fixtures.some(({ fixture }) => fixture.execution.status === "executable"),
  );
  assert.ok(fixtures.every(({ fixture }) => fixture.execution.status === "executable"));
  for (const { fixture } of fixtures) {
    assert.equal(fixture.content_origin, "synthetic");
    assert.deepEqual(validateSidekickSotaFixture(fixture), []);
  }
});

test("restricted evidence is absent from candidate context but present in the synthetic leak oracle", async () => {
  const fixtures = await corpus();
  const restricted = fixtures.find(
    ({ fixture }) =>
      fixture.id === "synthetic-restricted-board-injection",
  ).fixture;
  const candidateContext = disclosedSidekickContext(restricted);
  const leakOracle = evaluatorOnlyForbiddenContext(restricted);

  assert.deepEqual(candidateContext.evidence, []);
  assert.doesNotMatch(
    JSON.stringify(candidateContext),
    /succession contingency/i,
  );
  assert.equal(leakOracle.length, 1);
  assert.match(leakOracle[0].text, /succession contingency/i);
});

test("required and forbidden evidence cannot overlap", async () => {
  const fixtures = await corpus();
  const invalid = structuredClone(fixtures[0].fixture);
  invalid.turns[0].required_evidence_ids = ["utterance_one"];
  invalid.turns[0].forbidden_evidence_ids = ["utterance_one"];
  assert.ok(
    validateSidekickSotaFixture(invalid).some((finding) =>
      finding.includes("evidence_cannot_be_required_and_forbidden"),
    ),
  );
});

test("free-form known facts cannot bypass typed evidence sensitivity metadata", async () => {
  const fixtures = await corpus();
  const invalid = structuredClone(fixtures[0].fixture);
  invalid.prepared_context.known_facts = ["an untyped candidate-visible fact"];
  assert.ok(
    validateSidekickSotaFixture(invalid).some((finding) =>
      finding.includes("$.prepared_context.known_facts: unknown_key"),
    ),
  );
});

test("mechanical scoring enforces decision, provenance, word, and visual boundaries", async () => {
  const fixtures = await corpus();
  const fixture = fixtures.find(
    ({ fixture: candidate }) =>
      candidate.id === "synthetic-incident-rollback-threshold",
  ).fixture;
  const valid = scoreSidekickSotaResponses({
    fixture,
    responses: {
      containment_decision: {
        decision: "speak",
        text: "Disable the new path for fresh sessions, preserve workers until queued jobs drain safely, and re-enable only after the measured error rate clears the agreed recovery boundary.",
        evidence_ids: ["utterance_one", "utterance_two", "utterance_three"],
        claims_visual_observation: false,
        visual_evidence_ids: [],
      },
    },
  });
  assert.equal(valid.passed, true);

  const falseVisualClaim = scoreSidekickSotaResponses({
    fixture,
    responses: {
      containment_decision: {
        decision: "speak",
        text: "The dashboard proves a rollback is safe.",
        evidence_ids: ["utterance_one"],
        claims_visual_observation: true,
        visual_evidence_ids: ["screen-decoy"],
      },
    },
  });
  assert.equal(falseVisualClaim.passed, false);
  assert.equal(
    falseVisualClaim.turns[0].checks.find(
      ({ name }) => name === "visual_claim_shape",
    ).passed,
    false,
  );

  const emptySpeech = scoreSidekickSotaResponses({
    fixture,
    responses: {
      containment_decision: {
        decision: "speak",
        text: null,
        evidence_ids: ["utterance_one", "utterance_two", "utterance_three"],
        claims_visual_observation: false,
        visual_evidence_ids: [],
      },
    },
  });
  assert.equal(emptySpeech.passed, false);
  assert.equal(
    emptySpeech.turns[0].checks.find(
      ({ name }) => name === "speak_text_shape",
    ).passed,
    false,
  );
});

test("a turn cannot cite transcript evidence that arrives after its declared cutoff", async () => {
  const fixtures = await corpus();
  const fixture = structuredClone(
    fixtures.find(
      ({ fixture: candidate }) =>
        candidate.id === "synthetic-incident-rollback-threshold",
    ).fixture,
  );
  fixture.turns[0].transcript_through_id = "utterance_two";
  fixture.turns[0].required_evidence_ids = ["utterance_one", "utterance_two"];
  const score = scoreSidekickSotaResponses({
    fixture,
    responses: {
      containment_decision: {
        decision: "speak",
        text: "Disable the new path and preserve queued work.",
        evidence_ids: ["utterance_one", "utterance_two", "utterance_four"],
        claims_visual_observation: false,
        visual_evidence_ids: [],
      },
    },
  });
  assert.equal(score.passed, false);
  assert.equal(
    score.turns[0].checks.find(({ name }) => name === "evidence_available")
      .passed,
    false,
  );
});

test("routine confirmation requires a real silent receipt, not missing output or chatter", async () => {
  const fixtures = await corpus();
  const fixture = fixtures.find(
    ({ fixture: candidate }) =>
      candidate.id === "synthetic-agenda-confirmation-silence",
  ).fixture;
  assert.equal(
    scoreSidekickSotaResponses({ fixture, responses: {} }).passed,
    false,
  );
  assert.equal(
    scoreSidekickSotaResponses({
      fixture,
      responses: {
        resolved_confirmation: {
          decision: "silent",
          text: null,
          evidence_ids: [],
          claims_visual_observation: false,
          visual_evidence_ids: [],
        },
      },
    }).passed,
    true,
  );
  assert.equal(
    scoreSidekickSotaResponses({
      fixture,
      responses: {
        resolved_confirmation: {
          decision: "silent",
          text: "Watching.",
          evidence_ids: [],
          claims_visual_observation: false,
          visual_evidence_ids: [],
        },
      },
    }).passed,
    false,
  );
});
