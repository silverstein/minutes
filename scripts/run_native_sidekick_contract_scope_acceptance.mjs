#!/usr/bin/env node

import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import {
  scoreAggregateCappedRemedy,
  scorePerWindowRemedy,
} from "../tests/eval/sidekick_contract_scope_golden.mjs";

const MAX_FIRST_TOKEN_MS = 5_000;
const MAX_TURN_TOTAL_MS = 10_000;
const MAX_WALL_MS = 20_000;
const HARD_TIMEOUT_MS = 35_000;
const MAX_OUTPUT_BYTES = 1024 * 1024;

const fixtureSpecs = Object.freeze([
  Object.freeze({
    name: "per_window_remedy",
    path: fileURLToPath(new URL(
      "../tests/fixtures/sidekick_rehearsal/v1/northstar_uptime_credit.json",
      import.meta.url,
    )),
    expectedId: "synthetic-northstar-uptime-credit",
    expectedTurnId: "per_window_scope",
    requiredEvidenceIds: Object.freeze(["utterance-1", "utterance-2", "utterance-3"]),
    score: scorePerWindowRemedy,
  }),
  Object.freeze({
    name: "aggregate_capped_remedy",
    path: fileURLToPath(new URL(
      "../tests/fixtures/sidekick_rehearsal/v1/harbor_aggregate_rebate.json",
      import.meta.url,
    )),
    expectedId: "synthetic-harbor-aggregate-rebate",
    expectedTurnId: "aggregate_capped_scope",
    requiredEvidenceIds: Object.freeze(["utterance-1", "utterance-2", "utterance-3"]),
    score: scoreAggregateCappedRemedy,
  }),
]);

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

async function bundleManifestSha256(root) {
  const hash = createHash("sha256");
  async function walk(directory, relativeDirectory) {
    const entries = await fs.readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => left.name.localeCompare(right.name, "en"));
    for (const entry of entries) {
      const absolute = path.join(directory, entry.name);
      const relative = path.posix.join(relativeDirectory, entry.name);
      const stat = await fs.lstat(absolute);
      const mode = (stat.mode & 0o777).toString(8);
      if (entry.isDirectory()) {
        hash.update(`directory\0${relative}\0${mode}\0`);
        await walk(absolute, relative);
      } else if (entry.isSymbolicLink()) {
        hash.update(`symlink\0${relative}\0${mode}\0${await fs.readlink(absolute)}\0`);
      } else if (entry.isFile()) {
        hash.update(`file\0${relative}\0${mode}\0${stat.size}\0`);
        hash.update(await fs.readFile(absolute));
        hash.update("\0");
      } else {
        throw new Error(`unsupported bundle entry: ${relative}`);
      }
    }
  }
  await walk(root, "");
  return hash.digest("hex");
}

function check(name, passed, detail) {
  return { name, passed: Boolean(passed), detail };
}

async function loadFixtureSpec(spec) {
  const bytes = await fs.readFile(spec.path);
  const fixture = JSON.parse(bytes.toString("utf8"));
  if (!Array.isArray(fixture.turns) || fixture.turns.length !== 1) {
    throw new Error(`${spec.name} fixture must contain exactly one held-out turn`);
  }
  if (fixture.id !== spec.expectedId || fixture.turns[0].id !== spec.expectedTurnId) {
    throw new Error(`${spec.name} fixture identity does not match its acceptance contract`);
  }
  return {
    ...spec,
    fixture,
    fixtureSha256: sha256(bytes),
  };
}

function candidateFromPayload(payload) {
  const turn = Array.isArray(payload?.fixture_turns) ? payload.fixture_turns[0] : null;
  const result = turn?.result ?? {};
  const candidate = result?.candidate ?? {};
  return {
    turnId: turn?.id ?? null,
    prompt: turn?.prompt ?? null,
    sessionCorrelation: turn?.reasoning_session_correlation ?? null,
    outcome: result?.outcome ?? null,
    text: candidate?.text ?? "",
    evidenceIds: Array.isArray(candidate?.evidence_ids) ? candidate.evidence_ids : [],
    firstTokenMs: result?.first_token_ms ?? null,
    totalMs: result?.total_ms ?? null,
  };
}

export function evaluateContractScopeFixture(payload, runtime, spec) {
  const candidate = candidateFromPayload(payload);
  const quality = spec.score(candidate.text);
  const correlation = payload?.reasoning_session_correlation;
  const sourceChecks = [
    check("binary_exit_zero", runtime.exit_code === 0, `Installed binary exited ${runtime.exit_code}.`),
    check(
      "installed_binary_matches_current_signed_build",
      typeof runtime.executable_sha256 === "string" &&
        /^[a-f0-9]{64}$/.test(runtime.executable_sha256) &&
        runtime.executable_sha256 === spec.expectedExecutableSha256,
      "The installed executable must byte-match the freshly signed bundle from this checkout.",
    ),
    check(
      "installed_bundle_matches_current_signed_build",
      typeof runtime.bundle_sha256 === "string" &&
        /^[a-f0-9]{64}$/.test(runtime.bundle_sha256) &&
        runtime.bundle_sha256 === spec.expectedBundleSha256,
      "Every regular file, symlink, path, and mode in the installed app must match the freshly signed bundle.",
    ),
    check(
      "binary_reports_current_source_commit",
      typeof payload?.build_commit === "string" &&
        /^[a-f0-9]{40,64}$/.test(payload.build_commit) &&
        payload.build_commit === spec.expectedBuildCommit,
      "The installed executable must report the current checkout commit embedded at build time.",
    ),
    check(
      "external_synthetic_fixture_only",
      payload?.evidence_source === "synthetic_fixture" &&
        payload?.transcript_source === "external_user_supplied_fixture" &&
        payload?.prepared_context_source === "external_user_supplied_fixture" &&
        payload?.fixture_trust === "external_user_supplied",
      "The held-out run must use only the explicit synthetic fixture.",
    ),
    check(
      "no_screen_lane",
      payload?.screen_source === "none" && payload?.screen_available === false,
      "The held-out run must not inspect a user screen.",
    ),
    check(
      "fixture_identity_and_digest",
      payload?.fixture_id === spec.expectedId && payload?.fixture_sha256 === spec.fixtureSha256,
      "The installed binary must report the exact held-out fixture bytes scored by this checkout.",
    ),
    check(
      "one_persistent_published_turn",
      payload?.reasoning_sessions_started === 1 &&
        typeof correlation === "string" && /^[a-f0-9]{64}$/.test(correlation) &&
        Array.isArray(payload?.fixture_turns) && payload.fixture_turns.length === 1 &&
        candidate.turnId === spec.expectedTurnId &&
        candidate.prompt === spec.fixture.turns[0].typed_prompt &&
        candidate.sessionCorrelation === correlation &&
        candidate.outcome === "published",
      "The exact held-out prompt must publish through one provider-neutral reasoning session.",
    ),
    check(
      "complete_transcript_ingestion",
      payload?.transcript_items === spec.fixture.transcript.length,
      `The reducer must accept all ${spec.fixture.transcript.length} held-out transcript items.`,
    ),
    check(
      "required_evidence_chain",
      spec.requiredEvidenceIds.every((id) => candidate.evidenceIds.includes(id)),
      `The candidate must cite ${spec.requiredEvidenceIds.join(", ")}.`,
    ),
    check(
      "codex_fast_backend",
      payload?.provider === "codex-app-server" && payload?.model === "codex-fast" && payload?.privacy === "cloud",
      "The installed held-out run must exercise the Codex Fast provider adapter.",
    ),
  ];
  const latencyChecks = [
    check(
      `first_token_under_${MAX_FIRST_TOKEN_MS}ms`,
      Number.isFinite(candidate.firstTokenMs) && candidate.firstTokenMs <= MAX_FIRST_TOKEN_MS,
      `Observed ${candidate.firstTokenMs ?? "null"}ms.`,
    ),
    check(
      `turn_total_under_${MAX_TURN_TOTAL_MS}ms`,
      Number.isFinite(candidate.totalMs) && candidate.totalMs <= MAX_TURN_TOTAL_MS,
      `Observed ${candidate.totalMs ?? "null"}ms.`,
    ),
    check(
      `cold_wall_under_${MAX_WALL_MS}ms`,
      Number.isFinite(runtime.wall_ms) && runtime.wall_ms <= MAX_WALL_MS,
      `Observed ${runtime.wall_ms}ms including provider startup and teardown.`,
    ),
  ];
  const checks = [...sourceChecks, ...quality.checks, ...latencyChecks];
  return {
    name: spec.name,
    fixture_id: spec.expectedId,
    fixture_sha256: spec.fixtureSha256,
    passed: checks.every((item) => item.passed),
    score: { numerator: checks.filter((item) => item.passed).length, denominator: checks.length },
    quality_score: quality.score,
    source_checks: sourceChecks,
    quality_checks: quality.checks,
    latency_checks: latencyChecks,
    runtime,
    turn: candidate,
  };
}

function parseArgs(argv) {
  let app = path.join(os.homedir(), "Applications", "Minutes Dev.app");
  for (let index = 2; index < argv.length; index += 1) {
    if (argv[index] === "--app") app = argv[++index];
    else throw new Error(`unknown argument: ${argv[index]}`);
  }
  if (!app) throw new Error("--app requires a path");
  return { app: path.resolve(app) };
}

async function runInstalledBinary(executable, fixturePath, installedBundleSha256) {
  await fs.access(executable);
  const executableSha256 = sha256(await fs.readFile(executable));
  const startedAt = performance.now();
  const child = spawn(executable, [
    "--diagnose-native-sidekick",
    "--consent-cloud",
    "--diagnose-native-sidekick-fixture",
    fixturePath,
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
  const { app } = parseArgs(argv);
  const repositoryRoot = fileURLToPath(new URL("../", import.meta.url));
  const expectedBuildCommit = execFileSync("git", ["rev-parse", "--verify", "HEAD"], {
    cwd: repositoryRoot,
    encoding: "utf8",
  }).trim().toLowerCase();
  if (!/^[a-f0-9]{40,64}$/.test(expectedBuildCommit)) {
    throw new Error("could not resolve a valid current checkout commit");
  }
  const checkoutStatus = execFileSync(
    "git",
    ["status", "--porcelain=v1", "--untracked-files=all"],
    { cwd: repositoryRoot, encoding: "utf8" },
  ).trim();
  const allowedGeneratedPaths = [
    "tauri/src-tauri/bin/mic_check",
    "tauri/src-tauri/bin/mic_check-aarch64-apple-darwin",
  ];
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
  const signedBuildExecutable = path.join(
    signedBuildApp,
    "Contents",
    "MacOS",
    "minutes-app",
  );
  const executable = path.join(app, "Contents", "MacOS", "minutes-app");
  execFileSync("codesign", ["--verify", "--deep", "--strict", signedBuildApp]);
  execFileSync("codesign", ["--verify", "--deep", "--strict", app]);
  const [expectedExecutableSha256, expectedBundleSha256, installedBundleSha256] = await Promise.all([
    fs.readFile(signedBuildExecutable).then(sha256),
    bundleManifestSha256(signedBuildApp),
    bundleManifestSha256(app),
  ]);
  const specs = (await Promise.all(fixtureSpecs.map(loadFixtureSpec))).map((spec) => ({
    ...spec,
    expectedBuildCommit,
    expectedExecutableSha256,
    expectedBundleSha256,
  }));
  const results = [];
  for (const spec of specs) {
    const { payload, runtime } = await runInstalledBinary(executable, spec.path, installedBundleSha256);
    results.push(evaluateContractScopeFixture(payload, runtime, spec));
  }
  const checks = results.flatMap((result) => [
    check(result.name, result.passed, `${result.score.numerator}/${result.score.denominator}`),
  ]);
  const report = {
    schema_version: 1,
    expected_build_commit: expectedBuildCommit,
    expected_executable_sha256: expectedExecutableSha256,
    expected_bundle_sha256: expectedBundleSha256,
    passed: checks.every((item) => item.passed),
    score: { numerator: checks.filter((item) => item.passed).length, denominator: checks.length },
    results,
  };
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
