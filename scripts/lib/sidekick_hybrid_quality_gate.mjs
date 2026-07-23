import { createHash } from "node:crypto";
import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

import {
  CODEX_REALTIME_EFFORT,
  CODEX_REALTIME_MODEL,
  CODEX_VERIFIER_MODEL,
} from "./sidekick_provider.mjs";
import {
  semanticJudgeCriteria,
} from "./sidekick_semantic_judge.mjs";
import {
  currentSidekickQualitySourceBinding,
  sidekickQualitySourceBindingMatches,
} from "./sidekick_quality_source_binding.mjs";
import {
  attestSidekickProviderExecutable,
  sidekickProviderAttestationMatches,
} from "./sidekick_provider_attestation.mjs";
import { scoreMeridianResponses } from "../../tests/eval/sidekick_rehearsal_golden.mjs";
import { meridianSemanticCalibrationCases } from "../../tests/eval/sidekick_semantic_calibration.mjs";

export const DEFAULT_SIDEKICK_HYBRID_ARTIFACT = "/tmp/sidekick-session-eval.json";
export const MAX_ACCEPTED_FIRST_TOKEN_P95_MS = 4_000;
export const MAX_ACCEPTED_TOTAL_P95_MS = 7_000;

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const execFileAsync = promisify(execFile);
const MECHANICAL_CHECK_NAMES = Object.freeze([
  "derived_800k_monthly_exposure",
  "no_wrong_math",
  "no_agenda_clarification",
  "no_monitoring_or_tool_narration",
  "no_false_visual_claim",
  "background_brevity",
  "foreground_brevity",
  "hero_evidence_chain",
  "procurement_evidence_chain",
]);

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function percentile(values, quantile) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.max(0, Math.ceil(quantile * sorted.length) - 1)];
}

function semanticVerdictPasses(verdict) {
  const conjunction = [
    ...semanticJudgeCriteria.turn_1.map((name) => verdict?.turn_1?.[name]),
    ...semanticJudgeCriteria.turn_2.map((name) => verdict?.turn_2?.[name]),
  ].every((value) => value === true);
  return conjunction &&
    verdict?.computed_pass === conjunction &&
    verdict?.overall_pass === conjunction &&
    verdict?.passed === conjunction;
}

function substantiveRunFingerprint(run) {
  const semantic = run?.semantic ?? {};
  const normalizedText = (value) => String(value ?? "").trim().replace(/\s+/g, " ");
  return sha256(Buffer.from(JSON.stringify({
    responses: {
      proactive_hero_insight: normalizedText(run?.responses?.proactive_hero_insight),
      procurement_role_flip: normalizedText(run?.responses?.procurement_role_flip),
    },
    response_evidence_ids: run?.response_evidence_ids ?? null,
    semantic_verdict: {
      turn_1: Object.fromEntries(
        semanticJudgeCriteria.turn_1.map((name) => [name, semantic?.turn_1?.[name]]),
      ),
      turn_2: Object.fromEntries(
        semanticJudgeCriteria.turn_2.map((name) => [name, semantic?.turn_2?.[name]]),
      ),
      overall_pass: semantic.overall_pass ?? null,
      computed_pass: semantic.computed_pass ?? null,
      passed: semantic.passed ?? null,
      latency: semantic.latency ?? null,
    },
    strategist_latency: run?.latency ?? null,
  })));
}

function runRecomputesCleanly(run, expectedRun) {
  const responses = {
    turn_1: run?.responses?.proactive_hero_insight ?? "",
    turn_2: run?.responses?.procurement_role_flip ?? "",
    turn_1_evidence_ids: run?.response_evidence_ids?.proactive_hero_insight ?? [],
    turn_2_evidence_ids: run?.response_evidence_ids?.procurement_role_flip ?? [],
  };
  const mechanical = scoreMeridianResponses(responses);
  const reportedMechanical = Array.isArray(run?.golden?.mechanical_checks)
    ? run.golden.mechanical_checks
    : [];
  const reportedNames = reportedMechanical.map((item) => item?.name);
  const sessionStartedEvents = Array.isArray(run?.trace)
    ? run.trace.filter((item) => item?.type === "session_started")
    : [];
  const latencySamples = [run?.latency?.proactive, run?.latency?.role_flip];
  return run?.run === expectedRun &&
    run?.provider === "codex-app-server" &&
    run?.requested_model === CODEX_REALTIME_MODEL &&
    run?.requested_effort === CODEX_REALTIME_EFFORT &&
    run?.requested_verifier_model === CODEX_VERIFIER_MODEL &&
    run?.model === CODEX_REALTIME_MODEL &&
    typeof run?.backend_session_id === "string" && run.backend_session_id.trim().length > 0 &&
    typeof run?.semantic_judge_session_id === "string" &&
    run.semantic_judge_session_id.trim().length > 0 &&
    run?.semantic?.judge_receipt?.session_id === run.semantic_judge_session_id &&
    typeof run?.semantic?.judge_receipt?.turn_id === "string" &&
    run.semantic.judge_receipt.turn_id.trim().length > 0 &&
    sessionStartedEvents.length === 1 &&
    sessionStartedEvents[0]?.backend_session_id === run.backend_session_id &&
    sessionStartedEvents[0]?.provider === run.provider &&
    sessionStartedEvents[0]?.model === run.model &&
    run?.semantic_judge_provider === "codex-app-server" &&
    run?.semantic_judge_model === CODEX_REALTIME_MODEL &&
    run?.verifier_provider === "codex-app-server" &&
    run?.verifier_model === CODEX_VERIFIER_MODEL &&
    run?.published_count === 2 &&
    typeof responses.turn_1 === "string" && responses.turn_1.trim().length > 0 &&
    typeof responses.turn_2 === "string" && responses.turn_2.trim().length > 0 &&
    Array.isArray(responses.turn_1_evidence_ids) &&
    Array.isArray(responses.turn_2_evidence_ids) &&
    mechanical.passed === true &&
    reportedNames.length === MECHANICAL_CHECK_NAMES.length &&
    MECHANICAL_CHECK_NAMES.every((name, index) => reportedNames[index] === name) &&
    reportedMechanical.every((item) => item?.passed === true) &&
    semanticVerdictPasses(run?.semantic) &&
    latencySamples.every((sample) =>
      Number.isFinite(sample?.first_token_ms) &&
      Number.isFinite(sample?.total_ms) &&
      sample.first_token_ms >= 0 &&
      sample.total_ms >= 0 &&
      sample.first_token_ms <= sample.total_ms);
}

function calibrationRecomputesCleanly(calibration) {
  const expected = new Map(
    meridianSemanticCalibrationCases.map((item) => [item.id, item.expectedPass]),
  );
  const results = Array.isArray(calibration?.results) ? calibration.results : [];
  return results.length === expected.size &&
    new Set(results.map((item) => item?.id)).size === expected.size &&
    results.every((item) =>
      expected.has(item?.id) &&
      item?.expected_pass === expected.get(item.id) &&
      typeof item?.predicted_pass === "boolean" &&
      item?.correct === (item.predicted_pass === expected.get(item.id))) &&
    results.every((item) => item.correct) &&
    calibration?.passed === true &&
    calibration?.accuracy === 1;
}

/** Recompute the complete calibrated quality artifact; trust no aggregate flag. */
export function validateSidekickHybridQualityArtifact(
  report,
  artifactBytes,
  expectedSourceBinding,
) {
  const runs = Array.isArray(report?.runs) ? report.runs : [];
  const latencySamples = runs.flatMap((run) => [
    run?.latency?.proactive,
    run?.latency?.role_flip,
  ]).filter(Boolean);
  const firstTokenP95 = percentile(latencySamples.map((item) => item.first_token_ms), 0.95);
  const totalP95 = percentile(latencySamples.map((item) => item.total_ms), 0.95);
  const budgets = report?.aggregate?.budgets ?? {};
  const sourceBound = sidekickQualitySourceBindingMatches(
    report?.source_binding,
    expectedSourceBinding,
  );
  const runsPass = runs.length === 3 &&
    runs.every((run, index) => runRecomputesCleanly(run, index + 1));
  const distinctStrategistSessions = new Set(
    runs.map((run) => run?.backend_session_id),
  ).size === 3;
  const distinctSemanticJudgeSessions = new Set(
    runs.map((run) => run?.semantic_judge_session_id),
  ).size === 3;
  const distinctSubstantiveRuns = new Set(
    runs.map((run) => substantiveRunFingerprint(run)),
  ).size === 3;
  const calibrationPass = calibrationRecomputesCleanly(runs[0]?.semantic_calibration);
  const latencyPass =
    latencySamples.length === 6 &&
    firstTokenP95 <= MAX_ACCEPTED_FIRST_TOKEN_P95_MS &&
    totalP95 <= MAX_ACCEPTED_TOTAL_P95_MS &&
    budgets?.max_first_token_p95_ms === MAX_ACCEPTED_FIRST_TOKEN_P95_MS &&
    budgets?.max_total_p95_ms === MAX_ACCEPTED_TOTAL_P95_MS;
  const passed =
    report?.schema_version === 1 &&
    report?.fixture_id === "synthetic-meridian-ship-decision" &&
    report?.benchmark === "persistent-provider-neutral-sidekick" &&
    report?.requested_model === CODEX_REALTIME_MODEL &&
    report?.requested_effort === CODEX_REALTIME_EFFORT &&
    report?.requested_verifier_model === CODEX_VERIFIER_MODEL &&
    sidekickProviderAttestationMatches(
      report?.provider_executable,
      report?.provider_executable,
    ) &&
    sourceBound &&
    runsPass &&
    distinctStrategistSessions &&
    distinctSemanticJudgeSessions &&
    distinctSubstantiveRuns &&
    calibrationPass &&
    latencyPass;

  return {
    schema_version: 1,
    passed,
    fixture_id: report?.fixture_id ?? null,
    run_count: runs.length,
    requested_model: report?.requested_model ?? null,
    requested_effort: report?.requested_effort ?? null,
    requested_verifier_model: report?.requested_verifier_model ?? null,
    provider_executable: report?.provider_executable ?? null,
    mechanical_quality_passed: runsPass,
    semantic_quality_passed: runsPass,
    semantic_calibration_passed: calibrationPass,
    model_matched: runsPass,
    latency_passed: latencyPass,
    first_token_p95_ms: firstTokenP95,
    total_p95_ms: totalP95,
    max_first_token_p95_ms: budgets?.max_first_token_p95_ms ?? null,
    max_total_p95_ms: budgets?.max_total_p95_ms ?? null,
    source_binding: report?.source_binding ?? null,
    artifact_sha256: artifactBytes ? sha256(artifactBytes) : null,
  };
}

export function sidekickHybridQualityReceiptPasses(
  receipt,
  sourceBinding,
  providerExecutable,
) {
  return receipt?.schema_version === 1 &&
    receipt?.passed === true &&
    receipt?.fixture_id === "synthetic-meridian-ship-decision" &&
    receipt?.run_count === 3 &&
    receipt?.requested_model === CODEX_REALTIME_MODEL &&
    receipt?.requested_effort === CODEX_REALTIME_EFFORT &&
    receipt?.requested_verifier_model === CODEX_VERIFIER_MODEL &&
    sidekickProviderAttestationMatches(
      receipt?.provider_executable,
      providerExecutable,
    ) &&
    receipt?.mechanical_quality_passed === true &&
    receipt?.semantic_quality_passed === true &&
    receipt?.semantic_calibration_passed === true &&
    receipt?.model_matched === true &&
    receipt?.latency_passed === true &&
    receipt?.first_token_p95_ms <= MAX_ACCEPTED_FIRST_TOKEN_P95_MS &&
    receipt?.total_p95_ms <= MAX_ACCEPTED_TOTAL_P95_MS &&
    receipt?.max_first_token_p95_ms === MAX_ACCEPTED_FIRST_TOKEN_P95_MS &&
    receipt?.max_total_p95_ms === MAX_ACCEPTED_TOTAL_P95_MS &&
    sidekickQualitySourceBindingMatches(receipt?.source_binding, sourceBinding) &&
    /^[a-f0-9]{64}$/.test(receipt?.artifact_sha256 ?? "");
}

export async function loadSidekickHybridQualityArtifact(
  filePath = process.env.MINUTES_SIDEKICK_HYBRID_ARTIFACT ?? DEFAULT_SIDEKICK_HYBRID_ARTIFACT,
) {
  const [bytes, expectedSourceBinding] = await Promise.all([
    fs.readFile(filePath),
    currentSidekickQualitySourceBinding(repoRoot),
  ]);
  const report = JSON.parse(bytes.toString("utf8"));
  const receipt = validateSidekickHybridQualityArtifact(
    report,
    bytes,
    expectedSourceBinding,
  );
  if (!receipt.passed) {
    throw new Error(`Sidekick hybrid quality artifact failed validation: ${filePath}`);
  }
  return { ...receipt, artifact_path: filePath };
}

/**
 * Production acceptance trust boundary: launch the checked-in evaluator now,
 * receive its artifact digest over a private child-process stdout lane, then
 * validate the exact bytes it produced. A preexisting or relabeled JSON file
 * is never treated as proof of independent executions.
 */
export async function runAndLoadSidekickHybridQualityArtifact({
  filePath = `/tmp/sidekick-session-eval-${process.pid}.json`,
  codexPath,
} = {}) {
  const providerExecutable = await attestSidekickProviderExecutable(codexPath);
  const evaluatorPath = path.join(repoRoot, "scripts/sidekick_session_eval.mjs");
  await fs.rm(filePath, { force: true });
  const { stdout } = await execFileAsync(
    process.execPath,
    [
      evaluatorPath,
      "--repeat",
      "3",
      "--output",
      filePath,
      "--codex",
      providerExecutable.path,
      "--producer-receipt",
    ],
    {
      cwd: repoRoot,
      encoding: "utf8",
      timeout: 300_000,
      maxBuffer: 1024 * 1024,
    },
  );
  const lines = stdout.trim().split(/\r?\n/).filter(Boolean);
  if (lines.length !== 1) {
    throw new Error("Sidekick evaluator producer lane returned unexpected output");
  }
  const producerReceipt = JSON.parse(lines[0]);
  const [bytes, expectedSourceBinding] = await Promise.all([
    fs.readFile(filePath),
    currentSidekickQualitySourceBinding(repoRoot),
  ]);
  const providerExecutableAfter = await attestSidekickProviderExecutable(
    providerExecutable.path,
  );
  if (!sidekickProducerReceiptMatches({
    producerReceipt,
    artifactBytes: bytes,
    expectedSourceBinding,
    expectedProviderExecutable: providerExecutable,
  }) || !sidekickProviderAttestationMatches(providerExecutableAfter, providerExecutable)) {
    throw new Error("Sidekick evaluator producer receipt did not bind three fresh runs");
  }
  const report = JSON.parse(bytes.toString("utf8"));
  const receipt = validateSidekickHybridQualityArtifact(
    report,
    bytes,
    expectedSourceBinding,
  );
  if (!receipt.passed) {
    throw new Error("Fresh Sidekick hybrid quality run failed validation");
  }
  return {
    ...receipt,
    producer_attested: true,
    producer_artifact_sha256: producerReceipt.artifact_sha256,
    provider_executable: providerExecutable,
    artifact_path: filePath,
  };
}

export function sidekickProducerReceiptMatches({
  producerReceipt,
  artifactBytes,
  expectedSourceBinding,
  expectedProviderExecutable,
}) {
  return producerReceipt?.schema_version === 1 &&
    producerReceipt?.artifact_sha256 === sha256(artifactBytes) &&
    sidekickQualitySourceBindingMatches(
      producerReceipt?.source_binding,
      expectedSourceBinding,
    ) &&
    sidekickProviderAttestationMatches(
      producerReceipt?.provider_executable,
      expectedProviderExecutable,
    ) &&
    Array.isArray(producerReceipt?.strategist_session_ids) &&
    producerReceipt.strategist_session_ids.length === 3 &&
    new Set(producerReceipt.strategist_session_ids).size === 3 &&
    Array.isArray(producerReceipt?.semantic_judge_session_ids) &&
    producerReceipt.semantic_judge_session_ids.length === 3 &&
    new Set(producerReceipt.semantic_judge_session_ids).size === 3;
}

export const sidekickHybridMechanicalCheckNames = MECHANICAL_CHECK_NAMES;
