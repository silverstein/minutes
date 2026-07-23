#!/usr/bin/env node

import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { scoreMeridianResponses } from "../tests/eval/sidekick_rehearsal_golden.mjs";
import { CODEX_REALTIME_MODEL, CODEX_VERIFIER_MODEL } from "./lib/sidekick_provider.mjs";
import {
  runAndLoadSidekickHybridQualityArtifact,
  sidekickHybridQualityReceiptPasses,
} from "./lib/sidekick_hybrid_quality_gate.mjs";
import {
  runSidekickExactSemanticGate,
  sidekickExactSemanticReceiptPasses,
} from "./lib/sidekick_exact_semantic_gate.mjs";
import { currentSidekickQualitySourceBinding } from "./lib/sidekick_quality_source_binding.mjs";
import {
  attestSidekickProviderExecutable,
  sidekickProviderAttestationMatches,
} from "./lib/sidekick_provider_attestation.mjs";
import {
  bundleManifestSha256,
  canonicalInstalledAppPath,
  currentGeneratedBuildHelperPaths,
  validateCanonicalInstalledApp,
} from "./run_native_sidekick_contract_scope_acceptance.mjs";

const MAX_FIRST_TOKEN_MS = 5_000;
const MAX_TURN_TOTAL_MS = 10_000;
const MAX_PUBLICATION_READY_MS = 5_000;
const MAX_WALL_MS = 30_000;
const HARD_TIMEOUT_MS = 45_000;
const MAX_OUTPUT_BYTES = 1024 * 1024;
const CANONICAL_FIXTURE_PATH = fileURLToPath(
  new URL("../tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json", import.meta.url),
);

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

const canonicalFixtureBytes = await fs.readFile(CANONICAL_FIXTURE_PATH);
const canonicalFixture = JSON.parse(canonicalFixtureBytes.toString("utf8"));
if (!Array.isArray(canonicalFixture.turns) || canonicalFixture.turns.length !== 2) {
  throw new Error("canonical Meridian fixture must contain exactly two turns");
}
export const canonicalMeridianAcceptance = Object.freeze({
  fixture_id: canonicalFixture.id,
  fixture_sha256: sha256(canonicalFixtureBytes),
  transcript_items: canonicalFixture.transcript?.length ?? 0,
  turns: Object.freeze(
    canonicalFixture.turns.map((turn) =>
      Object.freeze({ id: turn.id, prompt: turn.typed_prompt }),
    ),
  ),
});

function semanticResponsesFromPayload(payload) {
  const turns = Array.isArray(payload?.fixture_turns) ? payload.fixture_turns : [];
  const candidate = (index) => turns[index]?.result?.candidate ?? {};
  return {
    turn_1: {
      text: String(candidate(0).text ?? ""),
      evidence_ids: Array.isArray(candidate(0).evidence_ids) ? candidate(0).evidence_ids : [],
    },
    turn_2: {
      text: String(candidate(1).text ?? ""),
      evidence_ids: Array.isArray(candidate(1).evidence_ids) ? candidate(1).evidence_ids : [],
    },
  };
}

function check(name, passed, detail) {
  return { name, passed: Boolean(passed), detail };
}

function candidateFromTurn(turn) {
  const result = turn?.result ?? {};
  const candidate = result?.candidate ?? {};
  return {
    id: turn?.id ?? null,
    prompt: turn?.prompt ?? null,
    outcome: result?.outcome ?? null,
    text: candidate?.text ?? "",
    evidence_ids: Array.isArray(candidate?.evidence_ids) ? candidate.evidence_ids : [],
    first_token_ms: result?.first_token_ms ?? null,
    total_ms: result?.total_ms ?? null,
    publication_ready_ms: result?.publication_ready_ms ?? null,
    reasoning_session_correlation: turn?.reasoning_session_correlation ?? null,
    evidence_verification: result?.evidence_verification ?? null,
    candidate_sha256: sha256(Buffer.from(JSON.stringify({
      decision: candidate?.decision ?? null,
      kind: candidate?.kind ?? null,
      text: candidate?.text ?? null,
      evidence_ids: Array.isArray(candidate?.evidence_ids) ? candidate.evidence_ids : [],
      visual_evidence_ids: Array.isArray(candidate?.visual_evidence_ids)
        ? candidate.visual_evidence_ids
        : [],
      claims_visual_observation: candidate?.claims_visual_observation ?? null,
      confidence: candidate?.confidence ?? null,
    }))),
  };
}

export function evaluateNativeSidekickAcceptance(payload, runtime) {
  const fixtureTurns = Array.isArray(payload?.fixture_turns) ? payload.fixture_turns : [];
  const turns = fixtureTurns.map(candidateFromTurn);
  const turn1 = turns[0] ?? candidateFromTurn(null);
  const turn2 = turns[1] ?? candidateFromTurn(null);
  const semanticResponses = semanticResponsesFromPayload(payload);
  const quality = scoreMeridianResponses({
    turn_1: turn1.text,
    turn_2: turn2.text,
    turn_1_evidence_ids: turn1.evidence_ids,
    turn_2_evidence_ids: turn2.evidence_ids,
  });
  const canonicalTurns = canonicalMeridianAcceptance.turns;
  const persistentCorrelation = payload?.reasoning_session_correlation;
  const turnsMatchCanonicalPrompts =
    turns.length === canonicalTurns.length &&
    turns.every(
      (turn, index) =>
        turn.id === canonicalTurns[index].id && turn.prompt === canonicalTurns[index].prompt,
    );
  const onePersistentSession =
    payload?.reasoning_sessions_started === 1 &&
    typeof persistentCorrelation === "string" &&
    /^[a-f0-9]{64}$/.test(persistentCorrelation) &&
    turns.every(
      (turn) => turn.reasoning_session_correlation === persistentCorrelation,
    );
  const freshVerifierPerTurn =
    payload?.verifier_sessions_started === turns.length &&
    payload?.verifier_provider === "codex-app-server" &&
    payload?.verifier_model === CODEX_VERIFIER_MODEL &&
    payload?.verifier_privacy === "cloud" &&
    turns.length === canonicalTurns.length &&
    turns.every((turn) =>
      /^[a-f0-9]{64}$/.test(turn.evidence_verification?.candidate_sha256 ?? "") &&
      turn.evidence_verification.candidate_sha256 === turn.candidate_sha256 &&
      turn.evidence_verification?.verdict?.decision === "allow" &&
      turn.evidence_verification?.verdict?.reason_code === "supported" &&
      /^[a-f0-9]{64}$/.test(
        turn.evidence_verification?.verifier_session_correlation ?? "",
      )) &&
    new Set(turns.map((turn) =>
      turn.evidence_verification.verifier_session_correlation)).size === turns.length;
  const sourceChecks = [
    check(
      "calibrated_hybrid_quality_artifact",
      sidekickHybridQualityReceiptPasses(
        runtime.hybrid_quality_gate,
        runtime.quality_source_binding,
        runtime.quality_provider_executable,
      ) && runtime.hybrid_quality_gate?.producer_attested === true,
      "Semantic quality requires a fresh three-run evaluator witnessed by this acceptance process; this native boundary also owns exact source, mechanical, provenance, and latency checks.",
    ),
    check(
      "exact_native_responses_pass_semantic_judge",
      sidekickExactSemanticReceiptPasses(
        runtime.exact_semantic_quality_gate,
        semanticResponses,
        runtime.quality_source_binding,
        runtime.quality_provider_executable,
      ),
      "The calibrated semantic judge must grade these exact native candidate bytes, not unrelated prior responses.",
    ),
    check(
      "quality_source_matches_current_build",
      runtime.quality_source_binding?.git_commit === runtime.expected_build_commit,
      "Quality prompts, evaluator, fixture, and engine must be bound to the installed build commit.",
    ),
    check(
      "one_attested_provider_for_product_and_quality_gates",
      payload?.provider_executable_path === runtime.quality_provider_executable?.path &&
        payload?.provider_executable_sha256 === runtime.quality_provider_executable?.sha256 &&
        payload?.provider_version === runtime.quality_provider_executable?.version &&
        sidekickProviderAttestationMatches(
          runtime.hybrid_quality_gate?.provider_executable,
          runtime.quality_provider_executable,
        ) &&
        sidekickProviderAttestationMatches(
          runtime.exact_semantic_quality_gate?.provider_executable,
          runtime.quality_provider_executable,
        ),
      "The installed product, fresh three-run evaluator, verifier, and exact semantic judge must use the same canonical provider executable bytes.",
    ),
    check("binary_exit_zero", runtime.exit_code === 0, `Installed binary exited ${runtime.exit_code}.`),
    check(
      "installed_binary_matches_current_signed_build",
      typeof runtime.executable_sha256 === "string" &&
        /^[a-f0-9]{64}$/.test(runtime.executable_sha256) &&
        runtime.executable_sha256 === runtime.expected_executable_sha256,
      "The installed executable must byte-match the freshly signed bundle from this checkout.",
    ),
    check(
      "installed_bundle_matches_current_signed_build",
      typeof runtime.bundle_sha256 === "string" &&
        /^[a-f0-9]{64}$/.test(runtime.bundle_sha256) &&
        runtime.bundle_sha256 === runtime.expected_bundle_sha256,
      "Every path, file, symlink, and mode in the installed app must match the freshly signed bundle.",
    ),
    check(
      "binary_reports_current_source_commit",
      typeof payload?.build_commit === "string" &&
        /^[a-f0-9]{40,64}$/.test(payload.build_commit) &&
        payload.build_commit === runtime.expected_build_commit,
      "The installed executable must report the current checkout commit embedded at build time.",
    ),
    check("embedded_transcript_only", payload?.transcript_source === "embedded_golden", "Transcript must come from compiled golden bytes."),
    check("embedded_prepared_context_only", payload?.prepared_context_source === "embedded_golden", "Prepared context must come from compiled golden bytes."),
    check("no_screen_lane", payload?.screen_source === "none" && payload?.screen_available === false, "Golden run must never inspect a user screen."),
    check("approved_fixture", payload?.fixture_trust === "embedded_approved" && payload?.fixture_id === canonicalMeridianAcceptance.fixture_id, "Fixture must be the approved embedded Meridian golden."),
    check("fixture_digest_matches_checkout", payload?.fixture_sha256 === canonicalMeridianAcceptance.fixture_sha256, "Installed binary's compiled fixture must byte-match the checkout golden."),
    check("synthetic_capture_identity", /^sidekick-diagnostic-synthetic-/.test(payload?.context_session_id ?? ""), "Capture identity must be synthetic and isolated."),
    check("canonical_transcript_items", payload?.transcript_items === canonicalMeridianAcceptance.transcript_items, "Every canonical fixture utterance must reach the reducer."),
    check("codex_fast_backend", payload?.provider === "codex-app-server" && payload?.model === CODEX_REALTIME_MODEL && payload?.privacy === "cloud", "Installed acceptance must exercise the configured Codex Fast cloud model."),
    check(
      "two_persistent_turns",
      turnsMatchCanonicalPrompts &&
        turns.every((turn) => turn.outcome === "published") &&
        onePersistentSession,
      "Both exact canonical prompts must publish through one provider-neutral persistent reasoning session.",
    ),
    check(
      "fresh_independent_verifier_per_turn",
      freshVerifierPerTurn,
      "Each published candidate must carry an allow/supported receipt from its own one-time verifier, with no synchronous future-session prewarm blocking publication.",
    ),
  ];
  const latencyChecks = [
    ...turns.flatMap((turn, index) => [
      check(
        `turn_${index + 1}_first_token_under_${MAX_FIRST_TOKEN_MS}ms`,
        Number.isFinite(turn.first_token_ms) && turn.first_token_ms <= MAX_FIRST_TOKEN_MS,
        `Observed ${turn.first_token_ms ?? "null"}ms.`,
      ),
      check(
        `turn_${index + 1}_total_under_${MAX_TURN_TOTAL_MS}ms`,
        Number.isFinite(turn.total_ms) && turn.total_ms <= MAX_TURN_TOTAL_MS,
        `Observed ${turn.total_ms ?? "null"}ms.`,
      ),
      check(
        `turn_${index + 1}_publication_ready_under_${MAX_PUBLICATION_READY_MS}ms`,
        Number.isFinite(turn.publication_ready_ms) &&
          turn.publication_ready_ms <= MAX_PUBLICATION_READY_MS,
        `Observed ${turn.publication_ready_ms ?? "null"}ms from typed request through publication readiness.`,
      ),
    ]),
    check(
      `cold_two_turn_wall_under_${MAX_WALL_MS}ms`,
      Number.isFinite(runtime.wall_ms) && runtime.wall_ms <= MAX_WALL_MS,
      `Observed ${runtime.wall_ms}ms including provider startup and teardown.`,
    ),
  ];
  const allChecks = [...sourceChecks, ...quality.mechanical_checks, ...latencyChecks];
  return {
    schema_version: 1,
    fixture_id: canonicalMeridianAcceptance.fixture_id,
    fixture_sha256: canonicalMeridianAcceptance.fixture_sha256,
    passed: allChecks.every((item) => item.passed),
    score: {
      numerator: allChecks.filter((item) => item.passed).length,
      denominator: allChecks.length,
    },
    quality_score: quality.score,
    source_checks: sourceChecks,
    quality_checks: quality.mechanical_checks,
    semantic_diagnostics: quality.checks.filter((item) =>
      !quality.mechanical_checks.some((mechanical) => mechanical.name === item.name)),
    latency_checks: latencyChecks,
    runtime,
    turns,
  };
}

async function runInstalledBinary(executable, installedBundleSha256) {
  await fs.access(executable);
  const executableSha256 = sha256(await fs.readFile(executable));
  const startedAt = performance.now();
  const child = spawn(executable, [
    "--diagnose-native-sidekick",
    "--consent-cloud",
    "--diagnose-native-sidekick-golden",
    "meridian",
  ], { stdio: ["ignore", "pipe", "pipe"] });
  let stdout = "";
  let stderr = "";
  let timedOut = false;
  let forceKill = null;
  const timeout = setTimeout(() => {
    timedOut = true;
    child.kill("SIGTERM");
    forceKill = setTimeout(() => child.kill("SIGKILL"), 2_000);
  }, HARD_TIMEOUT_MS);
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    stdout += chunk;
    if (Buffer.byteLength(stdout) > MAX_OUTPUT_BYTES) child.kill("SIGTERM");
  });
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
    if (Buffer.byteLength(stderr) > MAX_OUTPUT_BYTES) child.kill("SIGTERM");
  });
  const exitCode = await new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code, signal) => resolve(code ?? (signal ? 128 : 1)));
  });
  clearTimeout(timeout);
  if (forceKill) clearTimeout(forceKill);
  const wallMs = Math.round(performance.now() - startedAt);
  if (timedOut) throw new Error(`installed Sidekick diagnostic exceeded ${HARD_TIMEOUT_MS}ms`);
  let payload;
  try {
    payload = JSON.parse(stdout);
  } catch (error) {
    throw new Error(`installed Sidekick diagnostic did not return JSON: ${error.message}\n${stderr.slice(0, 2_000)}`);
  }
  return {
    payload,
    runtime: {
      exit_code: exitCode,
      wall_ms: wallMs,
      executable_sha256: executableSha256,
      bundle_sha256: installedBundleSha256,
      stderr: stderr.trim().slice(0, 2_000),
    },
  };
}

async function main(argv) {
  const canonicalApp = canonicalInstalledAppPath();
  if (argv.length > 2) {
    throw new Error(`installed Meridian acceptance only runs against ${canonicalApp}`);
  }
  const app = canonicalApp;
  await validateCanonicalInstalledApp(app, canonicalApp);
  const repositoryRoot = fileURLToPath(new URL("../", import.meta.url));
  const expectedBuildCommit = execFileSync("git", ["rev-parse", "--verify", "HEAD"], {
    cwd: repositoryRoot,
    encoding: "utf8",
  }).trim().toLowerCase();
  const checkoutStatus = execFileSync(
    "git",
    ["status", "--porcelain=v1", "--untracked-files=all"],
    { cwd: repositoryRoot, encoding: "utf8" },
  ).trim();
  const allowedGeneratedPaths = currentGeneratedBuildHelperPaths(repositoryRoot);
  const relevantDirtyLines = checkoutStatus.split("\n").filter(Boolean).filter((line) =>
    !allowedGeneratedPaths.some((generatedPath) => line.slice(3) === generatedPath),
  );
  if (relevantDirtyLines.length > 0) {
    throw new Error(`acceptance requires committed application and harness source; dirty paths:\n${relevantDirtyLines.join("\n")}`);
  }
  const signedBuildApp = path.join(
    repositoryRoot,
    "target",
    "release",
    "bundle",
    "macos",
    "Minutes Dev.app",
  );
  const signedBuildExecutable = path.join(signedBuildApp, "Contents", "MacOS", "minutes-app");
  const executable = path.join(app, "Contents", "MacOS", "minutes-app");
  execFileSync("codesign", ["--verify", "--deep", "--strict", signedBuildApp]);
  execFileSync("codesign", ["--verify", "--deep", "--strict", app]);
  const [expectedExecutableSha256, expectedBundleSha256, installedBundleSha256] = await Promise.all([
    fs.readFile(signedBuildExecutable).then(sha256),
    bundleManifestSha256(signedBuildApp),
    bundleManifestSha256(app),
  ]);
  const { payload, runtime } = await runInstalledBinary(executable, installedBundleSha256);
  const qualitySourceBinding = await currentSidekickQualitySourceBinding(repositoryRoot);
  const qualityProviderExecutable = await attestSidekickProviderExecutable(
    payload.provider_executable_path,
  );
  if (
    payload.provider_executable_sha256 !== qualityProviderExecutable.sha256 ||
    payload.provider_version !== qualityProviderExecutable.version
  ) {
    throw new Error("installed Sidekick provider attestation did not match the current executable");
  }
  const semanticResponses = semanticResponsesFromPayload(payload);
  Object.assign(runtime, {
    expected_build_commit: expectedBuildCommit,
    expected_executable_sha256: expectedExecutableSha256,
    expected_bundle_sha256: expectedBundleSha256,
    quality_source_binding: qualitySourceBinding,
    quality_provider_executable: qualityProviderExecutable,
    hybrid_quality_gate: await runAndLoadSidekickHybridQualityArtifact({
      codexPath: qualityProviderExecutable.path,
    }),
    exact_semantic_quality_gate: await runSidekickExactSemanticGate({
      fixture: canonicalFixture,
      responses: semanticResponses,
      sourceBinding: qualitySourceBinding,
      codex: qualityProviderExecutable.path,
    }),
  });
  const qualityProviderExecutableAfter = await attestSidekickProviderExecutable(
    qualityProviderExecutable.path,
  );
  if (!sidekickProviderAttestationMatches(
    qualityProviderExecutableAfter,
    qualityProviderExecutable,
  )) {
    throw new Error("Sidekick provider executable changed across native acceptance");
  }
  const report = evaluateNativeSidekickAcceptance(payload, runtime);
  report.tested_app_path = app;
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  return report.passed ? 0 : 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = await main(process.argv);
  } catch (error) {
    process.stderr.write(`${error.stack || error}\n`);
    process.exitCode = 1;
  }
}
