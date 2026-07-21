#!/usr/bin/env node

import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  CodexAppServerClient,
  configuredMcpDisableArgs,
} from "./lib/codex_app_server.mjs";
import { CodexAppServerBackend } from "./lib/sidekick_provider.mjs";
import { SidekickSession } from "./lib/sidekick_session.mjs";
import {
  defaultFixturePath,
  scoreMeridianResponses,
} from "../tests/eval/sidekick_rehearsal_golden.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");

function parseArgs(argv) {
  const options = {
    fixture: defaultFixturePath,
    codex: "codex",
    output: null,
    repeat: 1,
    maxFirstTokenMs: 4_000,
    maxTotalMs: 10_000,
  };
  for (let index = 2; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--fixture") options.fixture = argv[++index];
    else if (arg === "--codex") options.codex = argv[++index];
    else if (arg === "--output") options.output = argv[++index];
    else if (arg === "--repeat") options.repeat = Number(argv[++index]);
    else if (arg === "--max-first-token-ms") options.maxFirstTokenMs = Number(argv[++index]);
    else if (arg === "--max-total-ms") options.maxTotalMs = Number(argv[++index]);
    else throw new Error(`unknown argument: ${arg}`);
  }
  if (!Number.isInteger(options.repeat) || options.repeat < 1 || options.repeat > 20) {
    throw new Error("--repeat must be an integer from 1 to 20");
  }
  return options;
}

function percentile(values, quantile) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((left, right) => left - right);
  const index = Math.max(0, Math.ceil(quantile * sorted.length) - 1);
  return sorted[index];
}

function preparedBrief(fixture) {
  return {
    user_role: fixture.prepared_context.user_role,
    posture: fixture.prepared_context.posture,
    goal: fixture.prepared_context.demo_goal,
    known_facts: fixture.prepared_context.known_facts,
  };
}

async function runOnce({ fixture, codex, run }) {
  const cwd = await fs.mkdtemp(path.join(os.tmpdir(), "minutes-sidekick-session-eval-"));
  const mcpDisableArgs = await configuredMcpDisableArgs();
  const client = new CodexAppServerClient({
    command: codex,
    args: [
      "--disable",
      "apps",
      "--disable",
      "plugins",
      "--config",
      'service_tier="fast"',
      "--config",
      'model_reasoning_effort="low"',
      ...mcpDisableArgs,
      "--enable",
      "fast_mode",
      "app-server",
    ],
    cwd,
    requestTimeoutMs: 60_000,
    experimentalApi: true,
    clientInfo: {
      name: "minutes-sidekick-eval",
      title: "Minutes Sidekick Eval",
      version: "0.2.0",
    },
  });
  const publications = [];
  const session = new SidekickSession({
    backend: new CodexAppServerBackend(client),
    captureSessionId: `synthetic-meridian-${run}`,
    brief: preparedBrief(fixture),
    onPublish: (publication) => publications.push(publication),
  });

  try {
    const backend = await session.start({ cwd });
    for (const item of fixture.transcript) {
      session.observeTranscript({
        id: `utterance-${item.sequence}`,
        captureSessionId: `synthetic-meridian-${run}`,
        speaker: item.speaker,
        text: item.text,
      });
    }

    // The hero insight must emerge proactively, without the user asking the
    // model to do the arithmetic or telling it what to notice.
    const proactive = await session.evaluateProactive();
    const roleFlip = await session.sendUser(fixture.turns[1].typed_prompt);
    const proactiveText = proactive?.publication?.decision?.text ?? "";
    const roleFlipText = roleFlip?.publication?.decision?.text ?? "";
    const golden = scoreMeridianResponses({
      turn_1: proactiveText,
      turn_2: roleFlipText,
      turn_1_evidence_ids: proactive?.publication?.decision?.evidence_ids ?? [],
    });
    return {
      run,
      provider: backend.provider,
      model: backend.model,
      service_tier: backend.serviceTier,
      latency: {
        proactive: proactive?.publication?.latency ?? null,
        role_flip: roleFlip?.publication?.latency ?? null,
      },
      responses: {
        proactive_hero_insight: proactiveText,
        procurement_role_flip: roleFlipText,
      },
      published_count: publications.length,
      golden,
      trace: session.trace,
    };
  } finally {
    await session.stop();
    await fs.rm(cwd, { recursive: true, force: true });
  }
}

async function main() {
  const options = parseArgs(process.argv);
  const fixture = JSON.parse(await fs.readFile(options.fixture, "utf8"));
  if (fixture.schema_version !== 1 || fixture.content_origin !== "synthetic") {
    throw new Error("Sidekick session eval accepts only schema-v1 synthetic fixtures");
  }

  const runs = [];
  for (let run = 1; run <= options.repeat; run += 1) {
    runs.push(await runOnce({ fixture, codex: options.codex, run }));
  }
  const latencySamples = runs.flatMap((run) =>
    [run.latency.proactive, run.latency.role_flip].filter(Boolean),
  );
  const firstTokenP95 = percentile(
    latencySamples.map((sample) => sample.first_token_ms).filter(Number.isFinite),
    0.95,
  );
  const totalP95 = percentile(
    latencySamples.map((sample) => sample.total_ms).filter(Number.isFinite),
    0.95,
  );
  const qualityPassed = runs.every((run) => run.golden.passed);
  const latencyPassed =
    firstTokenP95 !== null &&
    totalP95 !== null &&
    firstTokenP95 <= options.maxFirstTokenMs &&
    totalP95 <= options.maxTotalMs;
  const report = {
    schema_version: 1,
    fixture_id: fixture.id,
    benchmark: "persistent-provider-neutral-sidekick",
    runs,
    aggregate: {
      quality_passed: qualityPassed,
      latency_passed: latencyPassed,
      passed: qualityPassed && latencyPassed,
      first_token_p95_ms: firstTokenP95,
      total_p95_ms: totalP95,
      budgets: {
        max_first_token_p95_ms: options.maxFirstTokenMs,
        max_total_p95_ms: options.maxTotalMs,
      },
    },
  };
  const serialized = `${JSON.stringify(report, null, 2)}\n`;
  if (options.output) await fs.writeFile(path.resolve(repoRoot, options.output), serialized);
  else process.stdout.write(serialized);
  return report.aggregate.passed ? 0 : 1;
}

process.exitCode = await main();
