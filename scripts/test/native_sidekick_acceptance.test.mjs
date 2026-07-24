import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import {
  canonicalMeridianAcceptance,
  evaluateNativeSidekickAcceptance,
} from "../run_native_sidekick_acceptance.mjs";
import { sidekickSemanticResponseSha256 } from "../lib/sidekick_exact_semantic_gate.mjs";
import { semanticJudgeCriteria } from "../lib/sidekick_semantic_judge.mjs";

const qualitySourceBinding = {
  git_commit: "c".repeat(40),
  quality_surface_sha256: "e".repeat(64),
  fixture_sha256: "f".repeat(64),
};
const qualityProviderExecutable = {
  path: "/opt/homebrew/bin/codex",
  sha256: "9".repeat(64),
  version: "codex-cli 1.0.0",
};

function semanticPassVerdict() {
  const turn = (criteria) => Object.fromEntries([
    ...criteria.map((name) => [name, true]),
    ["reason", "Pass."],
  ]);
  return {
    turn_1: turn(semanticJudgeCriteria.turn_1),
    turn_2: turn(semanticJudgeCriteria.turn_2),
    computed_pass: true,
    overall_pass: true,
    passed: true,
  };
}

function semanticResponses(payload) {
  const candidates = payload.fixture_turns.map((turn) => turn.result.candidate);
  return {
    turn_1: { text: candidates[0].text, evidence_ids: candidates[0].evidence_ids },
    turn_2: { text: candidates[1].text, evidence_ids: candidates[1].evidence_ids },
  };
}

function candidate(
  text,
  evidenceIds,
  firstTokenMs = 900,
  totalMs = 1_800,
  publicationReadyMs = 1_900,
  verifierCorrelation = "1".repeat(64),
) {
  const candidatePayload = {
    decision: "speak",
    kind: "strategy",
    text,
    evidence_ids: evidenceIds,
    visual_evidence_ids: [],
    claims_visual_observation: false,
    confidence: 90,
  };
  return {
    outcome: "published",
    candidate: candidatePayload,
    first_token_ms: firstTokenMs,
    total_ms: totalMs,
    publication_ready_ms: publicationReadyMs,
    evidence_verification: {
      candidate_sha256: createHash("sha256")
        .update(JSON.stringify(candidatePayload))
        .digest("hex"),
      verdict: { decision: "allow", reason_code: "supported" },
      verifier_session_correlation: verifierCorrelation,
    },
  };
}

function passingRuntime(overrides = {}, payload = passingPayload()) {
  const responses = semanticResponses(payload);
  return {
    exit_code: 0,
    wall_ms: 4_000,
    executable_sha256: "b".repeat(64),
    expected_executable_sha256: "b".repeat(64),
    bundle_sha256: "d".repeat(64),
    expected_bundle_sha256: "d".repeat(64),
    expected_build_commit: "c".repeat(40),
    quality_source_binding: qualitySourceBinding,
    quality_provider_executable: qualityProviderExecutable,
    hybrid_quality_gate: {
      schema_version: 1,
      passed: true,
      producer_attested: true,
      fixture_id: "synthetic-meridian-ship-decision",
      run_count: 3,
      requested_model: "gpt-5.6-terra",
      requested_effort: "none",
      requested_verifier_model: "gpt-5.6-terra",
      requested_verifier_effort: "low",
      mechanical_quality_passed: true,
      semantic_quality_passed: true,
      semantic_calibration_passed: true,
      verifier_calibration_passed: true,
      model_matched: true,
      latency_passed: true,
      first_token_p95_ms: 2_000,
      total_median_ms: 4_500,
      service_target_pass_count: 6,
      total_sample_count: 6,
      total_p95_ms: 5_000,
      max_first_token_p95_ms: 4_000,
      max_total_median_ms: 6_000,
      service_target_total_ms: 8_000,
      min_service_target_pass_count: 5,
      max_total_p95_ms: 10_000,
      artifact_sha256: "f".repeat(64),
      producer_artifact_sha256: "f".repeat(64),
      source_binding: qualitySourceBinding,
      provider_executable: qualityProviderExecutable,
    },
    exact_semantic_quality_gate: {
      schema_version: 1,
      provider: "codex-app-server",
      model: "gpt-5.6-terra",
      response_sha256: sidekickSemanticResponseSha256(responses),
      source_binding: qualitySourceBinding,
      provider_executable: qualityProviderExecutable,
      verdict: semanticPassVerdict(),
    },
    stderr: "",
    ...overrides,
  };
}

function passingPayload() {
  const [vendorTurn, procurementTurn] = canonicalMeridianAcceptance.turns;
  const reasoningSessionCorrelation = "a".repeat(64);
  return {
    context_session_id: "sidekick-diagnostic-synthetic-42-1",
    transcript_items: canonicalMeridianAcceptance.transcript_items,
    transcript_source: "embedded_golden",
    prepared_context_source: "embedded_golden",
    screen_source: "none",
    screen_available: false,
    fixture_id: canonicalMeridianAcceptance.fixture_id,
    fixture_trust: "embedded_approved",
    fixture_sha256: canonicalMeridianAcceptance.fixture_sha256,
    provider: "codex-app-server",
    model: "gpt-5.6-terra",
    privacy: "cloud",
    provider_executable_path: qualityProviderExecutable.path,
    provider_executable_sha256: qualityProviderExecutable.sha256,
    provider_version: qualityProviderExecutable.version,
    verifier_provider: "codex-app-server",
    verifier_model: "gpt-5.6-terra",
    verifier_privacy: "cloud",
    build_commit: "c".repeat(40),
    reasoning_session_correlation: reasoningSessionCorrelation,
    reasoning_sessions_started: 1,
    verifier_sessions_started: 2,
    fixture_turns: [
      {
        id: vendorTurn.id,
        prompt: vendorTurn.prompt,
        reasoning_session_correlation: reasoningSessionCorrelation,
        result: candidate(
          "This is $800K per month in contractual liability, not a 90% quality decision. Confidence-gate automation and route uncertain tickets to a human. What confidence distribution and threshold by ticket type bounds the error rate?",
          ["utterance-1", "utterance-3", "utterance-4"],
          900,
          1_800,
          1_900,
          "1".repeat(64),
        ),
      },
      {
        id: procurementTurn.id,
        prompt: procurementTurn.prompt,
        reasoning_session_correlation: reasoningSessionCorrelation,
        result: candidate(
          "For Meridian procurement, require every wrong automated resolution to make the vendor owe Meridian a $200 credit, a written confidence-threshold SLA, audited error reporting, and Meridian's unilateral right to revert affected work to human handling without vendor permission.",
          ["utterance-1", "utterance-3", "utterance-4", "utterance-6"],
          900,
          1_800,
          1_900,
          "2".repeat(64),
        ),
      },
    ],
  };
}

test("installed acceptance requires quality, provenance, and latency together", () => {
  const report = evaluateNativeSidekickAcceptance(passingPayload(), passingRuntime());

  assert.equal(report.passed, true);
  assert.deepEqual(report.quality_score, { numerator: 18, denominator: 18 });
});

test("installed acceptance permits one fail-closed verifier recovery", () => {
  const payload = passingPayload();
  payload.verifier_sessions_started = 3;

  const report = evaluateNativeSidekickAcceptance(payload, passingRuntime({}, payload));

  assert.equal(report.passed, true);
});

test("installed acceptance delegates semantic paraphrases to the calibrated hybrid gate", () => {
  const payload = passingPayload();
  const texts = [
    "At 40,000 monthly tickets and a 10% miss rate, full automation produces 4,000 bad outcomes and $800,000/month in contractual exposure. The 90% headline cannot decide launch. Restrict automation to high-confidence bands; send lower bands to people. Which confidence-band error distribution defines the safe cutoff?",
    "Advise Meridian procurement: each incorrect automated disposition makes the supplier owe Meridian $200. Put the confidence cutoff and observed error ceiling in the SLA; expose underlying records for each case to audit; Meridian alone may immediately return affected work to people, with no supplier approval or delay.",
  ];
  payload.fixture_turns.forEach((turn, index) => {
    turn.result.candidate.text = texts[index];
    turn.result.evidence_verification.candidate_sha256 = createHash("sha256")
      .update(JSON.stringify(turn.result.candidate))
      .digest("hex");
  });

  const report = evaluateNativeSidekickAcceptance(payload, passingRuntime({}, payload));
  assert.equal(report.passed, true);
  assert.equal(report.semantic_diagnostics.some((item) => !item.passed), true);
});

test("a preexisting hybrid artifact without a live producer witness cannot pass", () => {
  const runtime = passingRuntime();
  runtime.hybrid_quality_gate.producer_attested = false;
  const report = evaluateNativeSidekickAcceptance(passingPayload(), runtime);
  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) => item.name === "calibrated_hybrid_quality_artifact").passed,
    false,
  );
});

test("an old good hybrid artifact cannot bless bad current native responses", () => {
  const payload = passingPayload();
  payload.fixture_turns[0].result.candidate.text = "The exposure is $800K/month. Automate everything.";
  payload.fixture_turns[1].result.candidate.text = "Hello.";
  payload.fixture_turns.forEach((turn) => {
    turn.result.evidence_verification.candidate_sha256 = createHash("sha256")
      .update(JSON.stringify(turn.result.candidate))
      .digest("hex");
  });
  const runtime = passingRuntime();

  const report = evaluateNativeSidekickAcceptance(payload, runtime);
  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) =>
      item.name === "exact_native_responses_pass_semantic_judge").passed,
    false,
  );
});

test("an active-meeting fallback cannot impersonate an embedded golden pass", () => {
  const payload = passingPayload();
  payload.transcript_source = "active_transcript";

  const report = evaluateNativeSidekickAcceptance(payload, passingRuntime());

  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) => item.name === "embedded_transcript_only").passed,
    false,
  );
});

test("a slow otherwise-correct response fails the realtime bar", () => {
  const payload = passingPayload();
  payload.fixture_turns[0].result.first_token_ms = 5_001;

  const report = evaluateNativeSidekickAcceptance(payload, passingRuntime());

  assert.equal(report.passed, false);
  assert.equal(report.latency_checks[0].passed, false);
});

test("a slow engine publication fails even when provider streaming starts quickly", () => {
  const payload = passingPayload();
  payload.fixture_turns[0].result.publication_ready_ms = 10_001;

  const report = evaluateNativeSidekickAcceptance(payload, passingRuntime());

  assert.equal(report.passed, false);
  assert.equal(
    report.latency_checks.find((item) => item.name.includes("publication_ready")).passed,
    false,
  );
});

test("a bounded signed tail passes only with a passing fresh latency distribution", () => {
  const payload = passingPayload();
  payload.fixture_turns[0].result.publication_ready_ms = 8_000;

  const passing = evaluateNativeSidekickAcceptance(payload, passingRuntime());
  assert.equal(passing.passed, true);

  const runtime = passingRuntime();
  runtime.hybrid_quality_gate.passed = false;
  runtime.hybrid_quality_gate.latency_passed = false;
  runtime.hybrid_quality_gate.service_target_pass_count = 3;
  const failing = evaluateNativeSidekickAcceptance(payload, runtime);
  assert.equal(failing.passed, false);
  assert.equal(
    failing.source_checks.find((item) =>
      item.name === "calibrated_hybrid_quality_artifact").passed,
    false,
  );
});

test("a stale installed binary cannot inherit a current golden pass", () => {
  const report = evaluateNativeSidekickAcceptance(
    passingPayload(),
    passingRuntime({ executable_sha256: "e".repeat(64) }),
  );

  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) => item.name === "installed_binary_matches_current_signed_build").passed,
    false,
  );
});

test("fixture labels cannot hide a stale compiled golden", () => {
  const payload = passingPayload();
  payload.fixture_sha256 = "0".repeat(64);

  const report = evaluateNativeSidekickAcceptance(payload, passingRuntime());

  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) => item.name === "fixture_digest_matches_checkout").passed,
    false,
  );
});

test("missing or changed canonical prompts cannot earn a persistence pass", () => {
  const payload = passingPayload();
  delete payload.fixture_turns[0].prompt;

  const report = evaluateNativeSidekickAcceptance(payload, passingRuntime());

  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) => item.name === "two_persistent_turns").passed,
    false,
  );
});

test("two turns on different reasoning sessions cannot impersonate persistence", () => {
  const payload = passingPayload();
  payload.fixture_turns[1].reasoning_session_correlation = "b".repeat(64);

  const report = evaluateNativeSidekickAcceptance(payload, passingRuntime());

  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) => item.name === "two_persistent_turns").passed,
    false,
  );
});

test("a missing, reused, or rejected verifier receipt cannot earn acceptance", () => {
  const mutations = [
    (payload) => { payload.verifier_sessions_started = 5; },
    (payload) => {
      payload.fixture_turns[0].result.evidence_verification.candidate_sha256 = "9".repeat(64);
    },
    (payload) => {
      payload.fixture_turns[1].result.evidence_verification.verifier_session_correlation =
        payload.fixture_turns[0].result.evidence_verification.verifier_session_correlation;
    },
    (payload) => {
      payload.fixture_turns[0].result.evidence_verification.verdict.decision = "reject";
    },
  ];
  for (const mutate of mutations) {
    const payload = passingPayload();
    mutate(payload);
    const report = evaluateNativeSidekickAcceptance(payload, passingRuntime());
    assert.equal(report.passed, false);
    assert.equal(
      report.source_checks.find((item) => item.name === "fresh_independent_verifier_per_turn").passed,
      false,
    );
  }
});
