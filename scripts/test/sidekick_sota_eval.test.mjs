import assert from "node:assert/strict";
import test from "node:test";
import {
  assertDistinctSotaJudgeModels,
  buildSidekickSotaEvalPlan,
  scoreSidekickSotaLatency,
  sidekickSotaExitCode,
} from "../sidekick_sota_eval.mjs";
import {
  loadSidekickSotaFixtures,
} from "../lib/sidekick_sota_fixture.mjs";

const FIXTURE_DIRECTORY = new URL(
  "../../tests/fixtures/sidekick_sota/v1/",
  import.meta.url,
);

test("the autonomous SOTA runner executes only current production-path scenarios by default", async () => {
  const fixtures = await loadSidekickSotaFixtures(FIXTURE_DIRECTORY);
  const plan = buildSidekickSotaEvalPlan(fixtures);
  assert.deepEqual(plan.counts, {
    total: 7,
    matched: 7,
    runnable: 7,
    skipped: 0,
  });
  assert.ok(
    plan.runnable.every(
      ({ fixture }) => fixture.execution.status === "executable",
    ),
  );
  assert.deepEqual(plan.skipped, []);
});

test("repository context scenario runs through the current production evidence contract", async () => {
  const fixtures = await loadSidekickSotaFixtures(FIXTURE_DIRECTORY);
  const scenario = "synthetic-repository-release-boundary";
  assert.deepEqual(
    buildSidekickSotaEvalPlan(fixtures, { scenario }).counts,
    { total: 7, matched: 1, runnable: 1, skipped: 0 },
  );
});

test("unknown scenario names fail before provider startup", async () => {
  const fixtures = await loadSidekickSotaFixtures(FIXTURE_DIRECTORY);
  assert.throws(
    () => buildSidekickSotaEvalPlan(fixtures, { scenario: "missing" }),
    /unknown Sidekick SOTA scenario/,
  );
});

test("candidate and semantic judge model identities must be distinct", () => {
  assert.throws(
    () =>
      assertDistinctSotaJudgeModels({
        strategistModel: "model-a",
        judgeModel: "model-a",
      }),
    /must be distinct/,
  );
  assert.doesNotThrow(() =>
    assertDistinctSotaJudgeModels({
      strategistModel: "model-a",
      judgeModel: "model-b",
    }),
  );
});

test("partial corpus success requires an explicit non-release opt-in", () => {
  const aggregate = {
    behavioral_path_all_passed: true,
    full_corpus_passed: false,
  };
  assert.equal(sidekickSotaExitCode(aggregate), 1);
  assert.equal(sidekickSotaExitCode(aggregate, { allowPartial: true }), 0);
  assert.equal(
    sidekickSotaExitCode({
      behavioral_path_all_passed: true,
      full_corpus_passed: true,
    }),
    0,
  );
});

test("foreground latency is a fail-closed part of the SOTA result", async () => {
  const fixtures = await loadSidekickSotaFixtures(FIXTURE_DIRECTORY);
  const fixture = fixtures.find(
    ({ fixture: candidate }) =>
      candidate.id === "synthetic-runway-hiring-tradeoff",
  ).fixture;
  const fast = scoreSidekickSotaLatency({
    fixture,
    latencies: {
      runway_decision: { first_token_ms: 1_200, total_ms: 4_800 },
    },
  });
  assert.equal(fast.passed, true);

  const slow = scoreSidekickSotaLatency({
    fixture,
    latencies: {
      runway_decision: { first_token_ms: 1_200, total_ms: 17_000 },
    },
  });
  assert.equal(slow.passed, false);
  assert.equal(slow.total_p95_ms, 17_000);

  const missing = scoreSidekickSotaLatency({ fixture, latencies: {} });
  assert.equal(missing.passed, false);
});
