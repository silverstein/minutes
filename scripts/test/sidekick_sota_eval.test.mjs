import assert from "node:assert/strict";
import test from "node:test";
import {
  assertDistinctSotaJudgeModels,
  buildSidekickSotaEvalPlan,
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
    runnable: 4,
    skipped: 3,
  });
  assert.ok(
    plan.runnable.every(
      ({ fixture }) => fixture.execution.status === "executable",
    ),
  );
  assert.ok(
    plan.skipped.every(({ status }) => status === "executable_projection"),
  );
});

test("projection scenarios cannot be promoted into production-path passes by a runner flag", async () => {
  const fixtures = await loadSidekickSotaFixtures(FIXTURE_DIRECTORY);
  const scenario = "synthetic-repository-release-boundary";
  assert.deepEqual(
    buildSidekickSotaEvalPlan(fixtures, { scenario }).counts,
    { total: 7, matched: 1, runnable: 0, skipped: 1 },
  );
  assert.deepEqual(
    buildSidekickSotaEvalPlan(fixtures, {
      scenario,
      includeProjections: true,
    }).counts,
    { total: 7, matched: 1, runnable: 0, skipped: 1 },
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
