#!/usr/bin/env node

import fs from "node:fs/promises";
import { execFileSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  CodexAppServerClient,
  configuredMcpDisableArgs,
} from "./lib/codex_app_server.mjs";
import {
  CodexAppServerBackend,
  CODEX_REALTIME_EFFORT,
  CODEX_REALTIME_MODEL,
  CODEX_VERIFIER_EFFORT,
  CODEX_VERIFIER_MODEL,
} from "./lib/sidekick_provider.mjs";
import { BackendEvidenceVerifier } from "./lib/sidekick_evidence_verifier.mjs";
import { SidekickSession } from "./lib/sidekick_session.mjs";
import {
  disclosedSidekickContext,
  loadSidekickSotaFixtures,
  scoreSidekickSotaResponses,
} from "./lib/sidekick_sota_fixture.mjs";
import { SidekickSotaJudge } from "./lib/sidekick_sota_judge.mjs";
import {
  attestSidekickProviderExecutable,
  sidekickProviderAttestationMatches,
} from "./lib/sidekick_provider_attestation.mjs";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDirectory, "..");
const defaultFixtureDirectory = path.join(
  repoRoot,
  "tests/fixtures/sidekick_sota/v1",
);

function parseArgs(argv) {
  const options = {
    fixtureDirectory: defaultFixtureDirectory,
    scenario: null,
    codex: "codex",
    model: CODEX_REALTIME_MODEL,
    effort: CODEX_REALTIME_EFFORT,
    verifierModel: CODEX_VERIFIER_MODEL,
    verifierEffort: CODEX_VERIFIER_EFFORT,
    judgeModel: "gpt-5.6-sol",
    judgeEffort: "medium",
    maxFirstTokenMs: 5_000,
    maxTotalMs: 8_000,
    output: null,
    list: false,
    allowPartial: false,
  };
  for (let index = 2; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--fixture-dir") options.fixtureDirectory = argv[++index];
    else if (argument === "--scenario") options.scenario = argv[++index];
    else if (argument === "--codex") options.codex = argv[++index];
    else if (argument === "--model") options.model = argv[++index];
    else if (argument === "--effort") options.effort = argv[++index];
    else if (argument === "--verifier-model") options.verifierModel = argv[++index];
    else if (argument === "--verifier-effort") options.verifierEffort = argv[++index];
    else if (argument === "--judge-model") options.judgeModel = argv[++index];
    else if (argument === "--judge-effort") options.judgeEffort = argv[++index];
    else if (argument === "--max-first-token-ms") {
      options.maxFirstTokenMs = Number(argv[++index]);
    } else if (argument === "--max-total-ms") {
      options.maxTotalMs = Number(argv[++index]);
    }
    else if (argument === "--output") options.output = argv[++index];
    else if (argument === "--list") options.list = true;
    else if (argument === "--allow-partial") options.allowPartial = true;
    else throw new Error(`unknown argument: ${argument}`);
  }
  return options;
}

export function assertDistinctSotaJudgeModels({
  strategistModel,
  judgeModel,
  source = "requested",
}) {
  const strategist = String(strategistModel ?? "").trim();
  const judge = String(judgeModel ?? "").trim();
  if (!strategist || !judge) {
    throw new Error(`${source} strategist and judge model identities are required`);
  }
  if (strategist.toLowerCase() === judge.toLowerCase()) {
    throw new Error(`${source} strategist and judge models must be distinct`);
  }
}

export function sidekickSotaExitCode(aggregate, { allowPartial = false } = {}) {
  if (aggregate.full_corpus_passed) return 0;
  if (allowPartial && aggregate.behavioral_path_all_passed) return 0;
  return 1;
}

function percentile(values, quantile) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.max(0, Math.ceil(sorted.length * quantile) - 1)];
}

export function scoreSidekickSotaLatency({
  fixture,
  latencies,
  maxFirstTokenMs = 5_000,
  maxTotalMs = 8_000,
}) {
  const samples = fixture.turns
    .filter((turn) => turn.mode === "foreground")
    .map((turn) => ({
      turn_id: turn.id,
      first_token_ms: latencies?.[turn.id]?.first_token_ms ?? null,
      total_ms: latencies?.[turn.id]?.total_ms ?? null,
    }));
  const checks = samples.map((sample) => ({
    ...sample,
    passed:
      Number.isFinite(sample.first_token_ms) &&
      Number.isFinite(sample.total_ms) &&
      sample.first_token_ms <= maxFirstTokenMs &&
      sample.total_ms <= maxTotalMs,
  }));
  const firstTokenP95 = percentile(
    samples.map(({ first_token_ms }) => first_token_ms).filter(Number.isFinite),
    0.95,
  );
  const totalP95 = percentile(
    samples.map(({ total_ms }) => total_ms).filter(Number.isFinite),
    0.95,
  );
  return {
    passed: checks.every(({ passed }) => passed),
    max_first_token_ms: maxFirstTokenMs,
    max_total_ms: maxTotalMs,
    first_token_p95_ms: firstTokenP95,
    total_p95_ms: totalP95,
    checks,
  };
}

export function buildSidekickSotaEvalPlan(
  loadedFixtures,
  { scenario = null } = {},
) {
  const matched = scenario
    ? loadedFixtures.filter(({ fixture }) => fixture.id === scenario)
    : loadedFixtures;
  if (scenario && matched.length === 0) {
    throw new Error(`unknown Sidekick SOTA scenario: ${scenario}`);
  }
  const runnable = matched.filter(
    ({ fixture }) => fixture.execution.status === "executable",
  );
  const skipped = matched
    .filter(({ fixture }) => !runnable.some(({ fixture: item }) => item.id === fixture.id))
    .map(({ fixture }) => ({
      id: fixture.id,
      status: fixture.execution.status,
      reason: fixture.execution.reason,
    }));
  return {
    runnable,
    skipped,
    counts: {
      total: loadedFixtures.length,
      matched: matched.length,
      runnable: runnable.length,
      skipped: skipped.length,
    },
  };
}

function clientArguments(effort, mcpDisableArgs) {
  return [
    "--disable",
    "apps",
    "--disable",
    "plugins",
    "--config",
    'service_tier="fast"',
    "--config",
    `model_reasoning_effort=${JSON.stringify(effort)}`,
    ...mcpDisableArgs,
    "--enable",
    "fast_mode",
    "app-server",
  ];
}

function responseFromCompletion(completion) {
  const decision = completion?.publication?.decision ?? completion?.decision;
  if (!decision) return null;
  return {
    decision: decision.decision,
    text: decision.text,
    evidence_ids: decision.evidence_ids ?? [],
    visual_evidence_ids: decision.visual_evidence_ids ?? [],
    claims_visual_observation: decision.claims_visual_observation ?? false,
    confidence: decision.confidence ?? null,
  };
}

async function runScenario({ fixture, providerExecutable, options, mcpDisableArgs }) {
  const cwd = await fs.mkdtemp(path.join(os.tmpdir(), "minutes-sidekick-sota-"));
  const makeClient = (name, title, effort = options.effort) =>
    new CodexAppServerClient({
      command: providerExecutable.path,
      args: clientArguments(effort, mcpDisableArgs),
      cwd,
      requestTimeoutMs: 60_000,
      experimentalApi: true,
      clientInfo: { name, title, version: "0.2.0" },
    });
  const strategistClient = makeClient(
    `minutes-sidekick-sota-${fixture.id}`,
    "Minutes Sidekick SOTA Strategist",
  );
  const judgeClient = makeClient(
    `minutes-sidekick-sota-judge-${fixture.id}`,
    "Minutes Sidekick SOTA Judge",
    options.judgeEffort,
  );
  let verifierIndex = 0;
  const evidenceVerifier = new BackendEvidenceVerifier({
    backendFactory: () => {
      verifierIndex += 1;
      return new CodexAppServerBackend(
        makeClient(
          `minutes-sidekick-sota-verifier-${fixture.id}-${verifierIndex}`,
          "Minutes Sidekick SOTA Evidence Verifier",
          options.verifierEffort,
        ),
        {
          model: options.verifierModel,
          reasoningEffort: options.verifierEffort,
        },
      );
    },
  });
  const captureSessionId = `synthetic-sota-${fixture.id}`;
  const session = new SidekickSession({
    backend: new CodexAppServerBackend(strategistClient, {
      model: options.model,
      reasoningEffort: options.effort,
    }),
    evidenceVerifier,
    captureSessionId,
    brief: disclosedSidekickContext(fixture),
  });
  const judge = new SidekickSotaJudge({
    backend: new CodexAppServerBackend(judgeClient, {
      model: options.judgeModel,
      reasoningEffort: options.judgeEffort,
    }),
  });

  try {
    const [strategistBackend, judgeBackend, verifierBackend] = await Promise.all([
      session.start({ cwd }),
      judge.start({ cwd }),
      evidenceVerifier.start({ cwd }),
    ]);
    assertDistinctSotaJudgeModels({
      strategistModel: strategistBackend.model,
      judgeModel: judgeBackend.model,
      source: "provider-reported",
    });
    const responses = {};
    const latencies = {};
    let observedTranscriptIndex = 0;
    for (const turn of fixture.turns) {
      const transcriptCutoff = fixture.transcript.findIndex(
        (item) => item.id === turn.transcript_through_id,
      );
      while (observedTranscriptIndex <= transcriptCutoff) {
        const item = fixture.transcript[observedTranscriptIndex];
        session.observeTranscript({
          id: item.id,
          captureSessionId,
          speaker: item.speaker,
          text: item.text,
        });
        observedTranscriptIndex += 1;
      }
      const completion =
        turn.mode === "background"
          ? await session.evaluateProactive()
          : await session.sendUser(turn.typed_prompt);
      responses[turn.id] = responseFromCompletion(completion);
      latencies[turn.id] = completion?.publication?.latency ?? null;
    }
    const mechanical = scoreSidekickSotaResponses({ fixture, responses });
    const semantic = await judge.grade({ fixture, responses });
    const latency = scoreSidekickSotaLatency({
      fixture,
      latencies,
      maxFirstTokenMs: options.maxFirstTokenMs,
      maxTotalMs: options.maxTotalMs,
    });
    return {
      fixture_id: fixture.id,
      execution_status: fixture.execution.status,
      passed: mechanical.passed && semantic.passed && latency.passed,
      quality_passed: mechanical.passed && semantic.passed,
      coverage: {
        persistent_reasoning_path: true,
        projected_transcript_ingestion: true,
        live_capture_pipeline: false,
        diarization: false,
        restricted_context_filter: false,
        declared_capture_mode: fixture.capture.mode,
      },
      strategist: {
        provider: strategistBackend.provider,
        model: strategistBackend.model,
        session_id: strategistBackend.sessionId,
      },
      verifier: {
        provider: verifierBackend.provider,
        model: verifierBackend.model,
        session_ids: evidenceVerifier.verificationReceipts.map(
          ({ session_id }) => session_id,
        ),
      },
      judge: {
        provider: judgeBackend.provider,
        model: judgeBackend.model,
        session_id: judgeBackend.sessionId,
      },
      responses,
      latencies,
      latency,
      mechanical,
      semantic,
      trace: session.trace,
    };
  } finally {
    judge.close();
    await evidenceVerifier.close();
    await session.stop();
    await fs.rm(cwd, { recursive: true, force: true });
  }
}

export async function main(argv = process.argv) {
  const options = parseArgs(argv);
  const loadedFixtures = await loadSidekickSotaFixtures(
    path.resolve(options.fixtureDirectory),
  );
  const plan = buildSidekickSotaEvalPlan(loadedFixtures, options);
  if (options.list) {
    const listing = {
      schema_version: 1,
      counts: plan.counts,
      runnable: plan.runnable.map(({ fixture }) => ({
        id: fixture.id,
        domain: fixture.domain,
        status: fixture.execution.status,
      })),
      skipped: plan.skipped,
    };
    process.stdout.write(`${JSON.stringify(listing, null, 2)}\n`);
    return 0;
  }
  if (plan.runnable.length === 0) {
    throw new Error("no executable Sidekick SOTA scenarios selected");
  }
  assertDistinctSotaJudgeModels({
    strategistModel: options.model,
    judgeModel: options.judgeModel,
  });

  const configuredProviderPath = path.isAbsolute(options.codex)
    ? options.codex
    : execFileSync("/usr/bin/which", [options.codex], { encoding: "utf8" }).trim();
  const providerExecutable = await attestSidekickProviderExecutable(
    configuredProviderPath,
  );
  const mcpDisableArgs = await configuredMcpDisableArgs();
  const results = [];
  for (const { fixture } of plan.runnable) {
    try {
      results.push(
        await runScenario({
          fixture,
          providerExecutable,
          options,
          mcpDisableArgs,
        }),
      );
    } catch (error) {
      results.push({
        fixture_id: fixture.id,
        execution_status: fixture.execution.status,
        passed: false,
        error: String(error),
      });
    }
  }
  const providerExecutableAfter = await attestSidekickProviderExecutable(
    providerExecutable.path,
  );
  if (!sidekickProviderAttestationMatches(providerExecutableAfter, providerExecutable)) {
    throw new Error("Codex provider executable changed during Sidekick SOTA evaluation");
  }
  const insightPassed = results.reduce(
    (total, result) => total + (result.semantic?.insights?.passed ?? 0),
    0,
  );
  const insightTotal = results.reduce(
    (total, result) => total + (result.semantic?.insights?.total ?? 0),
    0,
  );
  const requiredInsightRate = insightTotal > 0 ? insightPassed / insightTotal : 0;
  const qualityPassed =
    results.length > 0 &&
    results.every((result) => result.quality_passed) &&
    requiredInsightRate >= 0.9;
  const latencyPassed =
    results.length > 0 && results.every((result) => result.latency?.passed);
  const behavioralPathAllPassed =
    qualityPassed &&
    latencyPassed &&
    results.length > 0 &&
    results.every((result) => result.passed);
  const report = {
    schema_version: 1,
    benchmark: "sidekick-sota-adversarial-corpus",
    provider_executable: providerExecutableAfter,
    requested_model: options.model,
    requested_effort: options.effort,
    requested_verifier_model: options.verifierModel,
    requested_verifier_effort: options.verifierEffort,
    requested_judge_model: options.judgeModel,
    requested_judge_effort: options.judgeEffort,
    max_first_token_ms: options.maxFirstTokenMs,
    max_total_ms: options.maxTotalMs,
    skipped: plan.skipped,
    results,
    aggregate: {
      attempted: results.length,
      passed: results.filter((result) => result.passed).length,
      failed: results.filter((result) => !result.passed).length,
      deferred: plan.skipped.length,
      quality_passed: qualityPassed,
      latency_passed: latencyPassed,
      required_insights_found: insightPassed,
      required_insights_total: insightTotal,
      required_insight_rate: requiredInsightRate,
      first_token_p95_ms: percentile(
        results
          .flatMap((result) => result.latency?.checks ?? [])
          .map(({ first_token_ms }) => first_token_ms)
          .filter(Number.isFinite),
        0.95,
      ),
      total_p95_ms: percentile(
        results
          .flatMap((result) => result.latency?.checks ?? [])
          .map(({ total_ms }) => total_ms)
          .filter(Number.isFinite),
        0.95,
      ),
      behavioral_path_all_passed: behavioralPathAllPassed,
      full_corpus_passed:
        plan.counts.matched === plan.counts.total &&
        plan.skipped.length === 0 &&
        results.length === plan.counts.matched &&
        behavioralPathAllPassed,
      release_ready: false,
      partial_success_allowed: options.allowPartial,
      release_blockers: [
        "capture and diarization are not exercised by this behavioral replay",
        ...(requiredInsightRate < 0.9
          ? ["fewer than 90% of rubric insights were found"]
          : []),
        ...(plan.skipped.length > 0
          ? ["projection scenarios remain deferred until their product evidence lanes exist"]
          : []),
      ],
    },
  };
  const serialized = `${JSON.stringify(report, null, 2)}\n`;
  if (options.output) {
    await fs.writeFile(path.resolve(repoRoot, options.output), serialized);
  } else {
    process.stdout.write(serialized);
  }
  return sidekickSotaExitCode(report.aggregate, {
    allowPartial: options.allowPartial,
  });
}

if (
  process.argv[1] &&
  pathToFileURL(path.resolve(process.argv[1])).href === import.meta.url
) {
  process.exitCode = await main();
}
