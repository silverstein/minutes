import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  sidekickProducerReceiptMatches,
  sidekickHybridQualityReceiptPasses,
  validateSidekickHybridQualityArtifact,
} from "../lib/sidekick_hybrid_quality_gate.mjs";
import {
  currentSidekickQualitySourceBinding,
} from "../lib/sidekick_quality_source_binding.mjs";
import {
  CODEX_REALTIME_EFFORT,
  CODEX_VERIFIER_EFFORT,
  CODEX_VERIFIER_MODEL,
} from "../lib/sidekick_provider.mjs";
import { semanticJudgeCriteria } from "../lib/sidekick_semantic_judge.mjs";
import { scoreMeridianResponses } from "../../tests/eval/sidekick_rehearsal_golden.mjs";
import { meridianSemanticCalibrationCases } from "../../tests/eval/sidekick_semantic_calibration.mjs";
import { sidekickVerifierCalibrationCases } from "../../tests/eval/sidekick_verifier_calibration.mjs";

const sourceBinding = Object.freeze({
  git_commit: "a".repeat(40),
  quality_surface_sha256: "b".repeat(64),
  fixture_sha256: "c".repeat(64),
});
const providerExecutable = Object.freeze({
  path: "/opt/homebrew/bin/codex",
  sha256: "d".repeat(64),
  version: "codex-cli 1.0.0",
});
const verifierCalibrationSessionIds = sidekickVerifierCalibrationCases.map(
  (_, index) => `verifier-calibration-${index + 1}`,
);

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function semanticPass(number) {
  const turn = (criteria) => Object.fromEntries([
    ...criteria.map((name) => [name, true]),
    ["reason", "Meets the criterion."],
  ]);
  return {
    turn_1: turn(semanticJudgeCriteria.turn_1),
    turn_2: turn(semanticJudgeCriteria.turn_2),
    overall_pass: true,
    overall_reason: "All criteria pass.",
    computed_pass: true,
    passed: true,
    judge_receipt: {
      session_id: `judge-session-${number}`,
      turn_id: `judge-turn-${number}`,
    },
    latency: {
      first_token_ms: 500 + number,
      total_ms: 1_000 + number,
    },
  };
}

function passingArtifact() {
  const responses = {
    proactive_hero_insight:
      "This is $800K per month in contractual liability, not a 90% quality decision. Confidence-gate automation and route uncertain tickets to a human. What confidence distribution and threshold by ticket type bounds the error rate?",
    procurement_role_flip:
      "For Meridian procurement, require every wrong automated resolution to make the vendor owe Meridian a $200 credit, a written confidence-threshold SLA, audited case-level error records, and Meridian's unilateral right to revert affected work to human handling without vendor permission.",
  };
  const responseEvidence = {
    proactive_hero_insight: ["utterance-1", "utterance-3", "utterance-4"],
    procurement_role_flip: ["utterance-3", "utterance-6"],
  };
  const golden = scoreMeridianResponses({
    turn_1: responses.proactive_hero_insight,
    turn_2: responses.procurement_role_flip,
    turn_1_evidence_ids: responseEvidence.proactive_hero_insight,
    turn_2_evidence_ids: responseEvidence.procurement_role_flip,
  });
  const run = (number) => ({
    run: number,
    provider: "codex-app-server",
    requested_model: "gpt-5.6-terra",
    requested_effort: CODEX_REALTIME_EFFORT,
    requested_verifier_model: CODEX_VERIFIER_MODEL,
    requested_verifier_effort: CODEX_VERIFIER_EFFORT,
    provider_executable: structuredClone(providerExecutable),
    model: "gpt-5.6-terra",
    backend_session_id: `strategist-session-${number}`,
    semantic_judge_provider: "codex-app-server",
    semantic_judge_model: "gpt-5.6-terra",
    semantic_judge_session_id: `judge-session-${number}`,
    verifier_provider: "codex-app-server",
    verifier_model: CODEX_VERIFIER_MODEL,
    published_count: 2,
    responses: structuredClone(responses),
    response_evidence_ids: structuredClone(responseEvidence),
    latency: {
      proactive: { first_token_ms: 1_000 + number, total_ms: 4_000 + number },
      role_flip: { first_token_ms: 1_100 + number, total_ms: 4_100 + number },
    },
    golden: structuredClone(golden),
    semantic: semanticPass(number),
    trace: [{
      type: "session_started",
      backend_session_id: `strategist-session-${number}`,
      provider: "codex-app-server",
      model: "gpt-5.6-terra",
    }],
  });
  const runs = [run(1), run(2), run(3)];
  runs[0].semantic_calibration = {
    passed: true,
    accuracy: 1,
    results: meridianSemanticCalibrationCases.map((item) => ({
      id: item.id,
      expected_pass: item.expectedPass,
      predicted_pass: item.expectedPass,
      correct: true,
      reason: "Correctly classified.",
    })),
  };
  return {
    schema_version: 1,
    fixture_id: "synthetic-meridian-ship-decision",
    benchmark: "persistent-provider-neutral-sidekick",
    requested_model: "gpt-5.6-terra",
    requested_effort: CODEX_REALTIME_EFFORT,
    requested_verifier_model: CODEX_VERIFIER_MODEL,
    requested_verifier_effort: CODEX_VERIFIER_EFFORT,
    provider_executable: structuredClone(providerExecutable),
    source_binding: structuredClone(sourceBinding),
    runs,
    verifier_calibration: {
      model: CODEX_VERIFIER_MODEL,
      effort: CODEX_VERIFIER_EFFORT,
      passed: true,
      results: sidekickVerifierCalibrationCases.map((item, index) => ({
        id: item.id,
        expected_allowed: item.expected_allowed,
        expected_reason_code: item.expected_reason_code,
        allowed: item.expected_allowed,
        decision: item.expected_allowed ? "allow" : "reject",
        reason_code:
          item.expected_reason_code ??
          (item.expected_allowed ? "supported" : "unsupported_fact"),
        latency: { first_token_ms: 300 + index, total_ms: 600 + index },
        passed: true,
      })),
      session_ids: sidekickVerifierCalibrationCases.map(
        (_, index) => `verifier-calibration-${index + 1}`,
      ),
    },
    aggregate: {
      budgets: {
        max_first_token_p95_ms: 4_000,
        max_total_median_ms: 6_000,
        service_target_total_ms: 8_000,
        min_service_target_pass_count: 5,
        max_total_p95_ms: 10_000,
      },
    },
  };
}

test("hybrid gate recomputes the full calibrated artifact and binds its source", () => {
  const receipt = validateSidekickHybridQualityArtifact(
    passingArtifact(),
    Buffer.from("complete artifact"),
    sourceBinding,
  );
  assert.equal(receipt.passed, true);
  assert.equal(
    sidekickHybridQualityReceiptPasses(receipt, sourceBinding, providerExecutable),
    true,
  );
});

test("real quality-surface manifest resolves every bound source file", async () => {
  const binding = await currentSidekickQualitySourceBinding(repoRoot);
  assert.match(binding.git_commit, /^[a-f0-9]{40,64}$/);
  assert.match(binding.quality_surface_sha256, /^[a-f0-9]{64}$/);
  assert.match(binding.fixture_sha256, /^[a-f0-9]{64}$/);
});

test("live producer receipt binds artifact bytes, source, provider, and three fresh sessions", () => {
  const artifactBytes = Buffer.from(JSON.stringify({
    runs: [1, 2, 3].map((number) => ({
      backend_session_id: `strategist-${number}`,
      semantic_judge_session_id: `judge-${number}`,
    })),
    verifier_calibration: {
      session_ids: verifierCalibrationSessionIds,
    },
  }));
  const producerReceipt = {
    schema_version: 1,
    artifact_sha256: createHash("sha256").update(artifactBytes).digest("hex"),
    source_binding: sourceBinding,
    provider_executable: providerExecutable,
    strategist_session_ids: ["strategist-1", "strategist-2", "strategist-3"],
    semantic_judge_session_ids: ["judge-1", "judge-2", "judge-3"],
    verifier_calibration_session_ids: verifierCalibrationSessionIds,
  };
  const matches = (receipt = producerReceipt, bytes = artifactBytes) =>
    sidekickProducerReceiptMatches({
      producerReceipt: receipt,
      artifactBytes: bytes,
      expectedSourceBinding: sourceBinding,
      expectedProviderExecutable: providerExecutable,
    });
  assert.equal(matches(), true);
  assert.equal(matches(producerReceipt, Buffer.from("changed artifact")), false);
  assert.equal(matches({
    ...producerReceipt,
    provider_executable: { ...providerExecutable, sha256: "0".repeat(64) },
  }), false);
  assert.equal(matches({
    ...producerReceipt,
    strategist_session_ids: [
      "unrelated-strategist-1",
      "unrelated-strategist-2",
      "unrelated-strategist-3",
    ],
  }), false);
  assert.equal(matches({
    ...producerReceipt,
    semantic_judge_session_ids: [
      "unrelated-judge-1",
      "unrelated-judge-2",
      "unrelated-judge-3",
    ],
  }), false);
  assert.equal(matches({
    ...producerReceipt,
    verifier_calibration_session_ids: verifierCalibrationSessionIds.map(
      (_, index) => `unrelated-verifier-${index + 1}`,
    ),
  }), false);
  assert.equal(matches({
    ...producerReceipt,
    verifier_calibration_session_ids: verifierCalibrationSessionIds.map(
      (id, index) => index === 1 ? verifierCalibrationSessionIds[0] : id,
    ),
  }), false);
  assert.equal(matches({
    ...producerReceipt,
    strategist_session_ids: ["strategist-1", "strategist-1", "strategist-3"],
  }), false);
});

test("hybrid gate rejects thin self-attestation and recomputed failures", () => {
  const mutations = [
    (report) => { delete report.runs[0].responses; },
    (report) => { report.runs[0].run = 2; },
    (report) => { report.runs[1].semantic.turn_1.no_contradiction = false; },
    (report) => { report.runs[2].published_count = 1; },
    (report) => { report.runs[0].semantic_calibration.results[0].predicted_pass = false; },
    (report) => { report.verifier_calibration.results[1].allowed = true; },
    (report) => {
      report.verifier_calibration.results.find(
        (item) => item.id === "procurement_omission_of_material_remedy",
      ).reason_code = "unsupported_fact";
    },
    (report) => { report.verifier_calibration.session_ids[3] = report.verifier_calibration.session_ids[0]; },
    (report) => { report.runs[1].latency.role_flip.total_ms = 30_000; },
    (report) => {
      for (const run of report.runs.slice(0, 2)) {
        run.latency.proactive.total_ms = 6_500;
        run.latency.role_flip.total_ms = 6_500;
      }
    },
    (report) => {
      // Even with a healthy median and bounded 10s maximum, two completions
      // outside the 8s interactive target must fail the distribution gate.
      report.runs[0].latency.proactive.total_ms = 8_500;
      report.runs[1].latency.proactive.total_ms = 8_500;
    },
    (report) => { report.runs[1].latency.role_flip.first_token_ms = -1; },
    (report) => { report.runs[1].latency.role_flip.first_token_ms = 5_000; },
    (report) => {
      report.runs[1] = structuredClone(report.runs[0]);
      report.runs[1].run = 2;
      report.runs[1].backend_session_id = "relabeled-strategist-session";
      report.runs[1].semantic_judge_session_id = "relabeled-judge-session";
      report.runs[1].trace[0].backend_session_id = "relabeled-strategist-session";
      report.runs[1].semantic.judge_receipt.session_id = "relabeled-judge-session";
    },
    (report) => { delete report.requested_effort; },
    (report) => { report.runs[2].requested_effort = "high"; },
    (report) => { report.source_binding.quality_surface_sha256 = "d".repeat(64); },
  ];
  for (const mutate of mutations) {
    const report = passingArtifact();
    mutate(report);
    assert.equal(
      validateSidekickHybridQualityArtifact(
        report,
        Buffer.from("artifact"),
        sourceBinding,
      ).passed,
      false,
    );
  }
});
