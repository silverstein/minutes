#!/usr/bin/env node

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { scoreMeridianResponses } from "../tests/eval/sidekick_rehearsal_golden.mjs";

const MAX_FIRST_TOKEN_MS = 5_000;
const MAX_TURN_TOTAL_MS = 10_000;
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
    reasoning_session_correlation: turn?.reasoning_session_correlation ?? null,
  };
}

export function evaluateNativeSidekickAcceptance(payload, runtime) {
  const fixtureTurns = Array.isArray(payload?.fixture_turns) ? payload.fixture_turns : [];
  const turns = fixtureTurns.map(candidateFromTurn);
  const turn1 = turns[0] ?? candidateFromTurn(null);
  const turn2 = turns[1] ?? candidateFromTurn(null);
  const quality = scoreMeridianResponses({
    turn_1: turn1.text,
    turn_2: turn2.text,
    turn_1_evidence_ids: turn1.evidence_ids,
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
  const sourceChecks = [
    check("binary_exit_zero", runtime.exit_code === 0, `Installed binary exited ${runtime.exit_code}.`),
    check("embedded_transcript_only", payload?.transcript_source === "embedded_golden", "Transcript must come from compiled golden bytes."),
    check("embedded_prepared_context_only", payload?.prepared_context_source === "embedded_golden", "Prepared context must come from compiled golden bytes."),
    check("no_screen_lane", payload?.screen_source === "none" && payload?.screen_available === false, "Golden run must never inspect a user screen."),
    check("approved_fixture", payload?.fixture_trust === "embedded_approved" && payload?.fixture_id === canonicalMeridianAcceptance.fixture_id, "Fixture must be the approved embedded Meridian golden."),
    check("fixture_digest_matches_checkout", payload?.fixture_sha256 === canonicalMeridianAcceptance.fixture_sha256, "Installed binary's compiled fixture must byte-match the checkout golden."),
    check("synthetic_capture_identity", /^sidekick-diagnostic-synthetic-/.test(payload?.context_session_id ?? ""), "Capture identity must be synthetic and isolated."),
    check("canonical_transcript_items", payload?.transcript_items === canonicalMeridianAcceptance.transcript_items, "Every canonical fixture utterance must reach the reducer."),
    check("codex_fast_backend", payload?.provider === "codex-app-server" && payload?.model === "codex-fast" && payload?.privacy === "cloud", "Installed acceptance must exercise the Codex Fast cloud adapter."),
    check(
      "two_persistent_turns",
      turnsMatchCanonicalPrompts &&
        turns.every((turn) => turn.outcome === "published") &&
        onePersistentSession,
      "Both exact canonical prompts must publish through one provider-neutral persistent reasoning session.",
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
    ]),
    check(
      `cold_two_turn_wall_under_${MAX_WALL_MS}ms`,
      Number.isFinite(runtime.wall_ms) && runtime.wall_ms <= MAX_WALL_MS,
      `Observed ${runtime.wall_ms}ms including provider startup and teardown.`,
    ),
  ];
  const allChecks = [...sourceChecks, ...quality.checks, ...latencyChecks];
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
    quality_checks: quality.checks,
    latency_checks: latencyChecks,
    runtime,
    turns,
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

async function runInstalledBinary(executable) {
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
      stderr: stderr.trim().slice(0, 2_000),
    },
  };
}

async function main(argv) {
  const { app } = parseArgs(argv);
  const executable = path.join(app, "Contents", "MacOS", "minutes-app");
  const { payload, runtime } = await runInstalledBinary(executable);
  const report = evaluateNativeSidekickAcceptance(payload, runtime);
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
