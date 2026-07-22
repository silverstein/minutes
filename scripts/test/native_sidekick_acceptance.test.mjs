import assert from "node:assert/strict";
import test from "node:test";

import {
  canonicalMeridianAcceptance,
  evaluateNativeSidekickAcceptance,
} from "../run_native_sidekick_acceptance.mjs";

function candidate(text, evidenceIds, firstTokenMs = 900, totalMs = 1_800) {
  return {
    outcome: "published",
    candidate: {
      text,
      evidence_ids: evidenceIds,
      visual_evidence_ids: [],
      claims_visual_observation: false,
    },
    first_token_ms: firstTokenMs,
    total_ms: totalMs,
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
    model: "codex-fast",
    privacy: "cloud",
    reasoning_session_correlation: reasoningSessionCorrelation,
    reasoning_sessions_started: 1,
    fixture_turns: [
      {
        id: vendorTurn.id,
        prompt: vendorTurn.prompt,
        reasoning_session_correlation: reasoningSessionCorrelation,
        result: candidate(
          "This is $800K per month in contractual liability, not a 90% quality decision. Confidence-gate automation and route uncertain tickets to a human. What confidence distribution and threshold by ticket type bounds the error rate?",
          ["utterance-1", "utterance-3", "utterance-4"],
        ),
      },
      {
        id: procurementTurn.id,
        prompt: procurementTurn.prompt,
        reasoning_session_correlation: reasoningSessionCorrelation,
        result: candidate(
          "For Meridian procurement: keep every wrong automated resolution subject to the $200 credit with no automation carve-outs, require a written confidence-threshold SLA, demand audited error reporting and caps, and preserve Meridian's right to revert to human handling.",
          ["utterance-1", "utterance-3", "utterance-4"],
        ),
      },
    ],
  };
}

test("installed acceptance requires quality, provenance, and latency together", () => {
  const report = evaluateNativeSidekickAcceptance(passingPayload(), {
    exit_code: 0,
    wall_ms: 4_000,
    stderr: "",
  });

  assert.equal(report.passed, true);
  assert.deepEqual(report.quality_score, { numerator: 15, denominator: 15 });
});

test("an active-meeting fallback cannot impersonate an embedded golden pass", () => {
  const payload = passingPayload();
  payload.transcript_source = "active_transcript";

  const report = evaluateNativeSidekickAcceptance(payload, {
    exit_code: 0,
    wall_ms: 4_000,
    stderr: "",
  });

  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) => item.name === "embedded_transcript_only").passed,
    false,
  );
});

test("a slow otherwise-correct response fails the realtime bar", () => {
  const payload = passingPayload();
  payload.fixture_turns[0].result.first_token_ms = 5_001;

  const report = evaluateNativeSidekickAcceptance(payload, {
    exit_code: 0,
    wall_ms: 4_000,
    stderr: "",
  });

  assert.equal(report.passed, false);
  assert.equal(report.latency_checks[0].passed, false);
});

test("fixture labels cannot hide a stale compiled golden", () => {
  const payload = passingPayload();
  payload.fixture_sha256 = "0".repeat(64);

  const report = evaluateNativeSidekickAcceptance(payload, {
    exit_code: 0,
    wall_ms: 4_000,
    stderr: "",
  });

  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) => item.name === "fixture_digest_matches_checkout").passed,
    false,
  );
});

test("missing or changed canonical prompts cannot earn a persistence pass", () => {
  const payload = passingPayload();
  delete payload.fixture_turns[0].prompt;

  const report = evaluateNativeSidekickAcceptance(payload, {
    exit_code: 0,
    wall_ms: 4_000,
    stderr: "",
  });

  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) => item.name === "two_persistent_turns").passed,
    false,
  );
});

test("two turns on different reasoning sessions cannot impersonate persistence", () => {
  const payload = passingPayload();
  payload.fixture_turns[1].reasoning_session_correlation = "b".repeat(64);

  const report = evaluateNativeSidekickAcceptance(payload, {
    exit_code: 0,
    wall_ms: 4_000,
    stderr: "",
  });

  assert.equal(report.passed, false);
  assert.equal(
    report.source_checks.find((item) => item.name === "two_persistent_turns").passed,
    false,
  );
});
