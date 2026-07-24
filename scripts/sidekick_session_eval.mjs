#!/usr/bin/env node

import fs from "node:fs/promises";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  CodexAppServerClient,
  configuredMcpDisableArgs,
} from "./lib/codex_app_server.mjs";
import {
  CODEX_REALTIME_EFFORT,
  CODEX_REALTIME_MODEL,
  CODEX_VERIFIER_ADJUDICATION_EFFORT,
  CODEX_VERIFIER_EFFORT,
  CODEX_VERIFIER_MODEL,
  CodexAppServerBackend,
} from "./lib/sidekick_provider.mjs";
import { SidekickSession } from "./lib/sidekick_session.mjs";
import { SidekickSemanticJudge } from "./lib/sidekick_semantic_judge.mjs";
import { BackendEvidenceVerifier } from "./lib/sidekick_evidence_verifier.mjs";
import {
  defaultFixturePath,
  scoreMeridianResponses,
} from "../tests/eval/sidekick_rehearsal_golden.mjs";
import { meridianSemanticCalibrationCases } from "../tests/eval/sidekick_semantic_calibration.mjs";
import { sidekickVerifierCalibrationCases } from "../tests/eval/sidekick_verifier_calibration.mjs";
import { currentSidekickQualitySourceBinding } from "./lib/sidekick_quality_source_binding.mjs";
import {
  attestSidekickProviderExecutable,
  sidekickProviderAttestationMatches,
} from "./lib/sidekick_provider_attestation.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");

function parseArgs(argv) {
  const options = {
    fixture: defaultFixturePath,
    codex: "codex",
    output: null,
    repeat: 3,
    model: CODEX_REALTIME_MODEL,
    effort: CODEX_REALTIME_EFFORT,
    verifierModel: CODEX_VERIFIER_MODEL,
    verifierEffort: CODEX_VERIFIER_EFFORT,
    maxFirstTokenMs: 4_000,
    maxMedianTotalMs: 6_000,
    serviceTargetTotalMs: 8_000,
    minServiceTargetSamples: 5,
    maxTotalMs: 10_000,
    producerReceipt: false,
  };
  for (let index = 2; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--fixture") options.fixture = argv[++index];
    else if (arg === "--codex") options.codex = argv[++index];
    else if (arg === "--output") options.output = argv[++index];
    else if (arg === "--repeat") options.repeat = Number(argv[++index]);
    else if (arg === "--model") options.model = argv[++index];
    else if (arg === "--effort") options.effort = argv[++index];
    else if (arg === "--verifier-model") options.verifierModel = argv[++index];
    else if (arg === "--verifier-effort") options.verifierEffort = argv[++index];
    else if (arg === "--max-first-token-ms") options.maxFirstTokenMs = Number(argv[++index]);
    else if (arg === "--max-median-total-ms") options.maxMedianTotalMs = Number(argv[++index]);
    else if (arg === "--service-target-total-ms") options.serviceTargetTotalMs = Number(argv[++index]);
    else if (arg === "--max-total-ms") options.maxTotalMs = Number(argv[++index]);
    else if (arg === "--producer-receipt") options.producerReceipt = true;
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

function median(values) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((left, right) => left - right);
  const midpoint = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[midpoint - 1] + sorted[midpoint]) / 2
    : sorted[midpoint];
}

function preparedBrief(fixture) {
  return {
    user_role: fixture.prepared_context.user_role,
    posture: fixture.prepared_context.posture,
    goal: fixture.prepared_context.demo_goal,
    known_facts: fixture.prepared_context.known_facts,
  };
}

async function runVerifierCalibration({ codex, verifierModel, verifierEffort }) {
  const cwd = await fs.mkdtemp(path.join(os.tmpdir(), "minutes-sidekick-verifier-calibration-"));
  const mcpDisableArgs = await configuredMcpDisableArgs();
  let session = 0;
  const verifier = new BackendEvidenceVerifier({
    backendFactory: () => {
      session += 1;
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
          `model_reasoning_effort=${JSON.stringify(verifierEffort)}`,
          ...mcpDisableArgs,
          "--enable",
          "fast_mode",
          "app-server",
        ],
        cwd,
        requestTimeoutMs: 60_000,
        experimentalApi: true,
        clientInfo: {
          name: `minutes-sidekick-verifier-calibration-${session}`,
          title: "Minutes Sidekick Verifier Calibration",
          version: "0.2.0",
        },
      });
      return new CodexAppServerBackend(client, {
        model: verifierModel,
        reasoningEffort: verifierEffort,
        deliberateReasoningEffort: CODEX_VERIFIER_ADJUDICATION_EFFORT,
      });
    },
  });
  const results = [];
  try {
    await verifier.start({ cwd });
    for (const item of sidekickVerifierCalibrationCases) {
      const screenEvidence = item.screen_evidence
        ? {
            id: item.screen_evidence.id,
            path: path.resolve(repoRoot, item.screen_evidence.path),
          }
        : null;
      const verdict = await verifier.verify({
        candidate: item.candidate,
        transcriptEvidence: item.transcript_evidence,
        screenEvidence,
        authoritativeContext: item.authoritative_context ?? { role: "meeting strategist" },
      });
      results.push({
        id: item.id,
        expected_allowed: item.expected_allowed,
        expected_reason_code: item.expected_reason_code,
        allowed: verdict.allowed,
        decision: verdict.decision,
        reason_code: verdict.reason_code,
        latency: verdict.latency,
        passed:
          verdict.allowed === item.expected_allowed &&
          (!item.expected_reason_code ||
            verdict.reason_code === item.expected_reason_code),
      });
    }
    return {
      model: verifierModel,
      effort: verifierEffort,
      passed: results.every((item) => item.passed),
      results,
      session_ids: verifier.verificationReceipts.map((receipt) => receipt.session_id),
    };
  } finally {
    await verifier.close();
    await fs.rm(cwd, { recursive: true, force: true });
  }
}

async function runOnce({ fixture, codex, model, verifierModel, effort, verifierEffort, run }) {
  const cwd = await fs.mkdtemp(path.join(os.tmpdir(), "minutes-sidekick-session-eval-"));
  const mcpDisableArgs = await configuredMcpDisableArgs();
  const clientOptions = (name, title, backendEffort = effort) => ({
    command: codex,
    args: [
      "--disable",
      "apps",
      "--disable",
      "plugins",
      "--config",
      'service_tier="fast"',
      "--config",
      `model_reasoning_effort=${JSON.stringify(backendEffort)}`,
      ...mcpDisableArgs,
      "--enable",
      "fast_mode",
      "app-server",
    ],
    cwd,
    requestTimeoutMs: 60_000,
    experimentalApi: true,
    clientInfo: {
      name,
      title,
      version: "0.2.0",
    },
  });
  const client = new CodexAppServerClient(
    clientOptions("minutes-sidekick-eval", "Minutes Sidekick Eval"),
  );
  const judgeClient = new CodexAppServerClient(
    clientOptions("minutes-sidekick-semantic-judge", "Minutes Sidekick Semantic Judge"),
  );
  const publications = [];
  let verifierSession = 0;
  const evidenceVerifier = new BackendEvidenceVerifier({
    backendFactory: () => {
      verifierSession += 1;
      const verifierClient = new CodexAppServerClient(
        clientOptions(
          `minutes-sidekick-evidence-verifier-${verifierSession}`,
          "Minutes Sidekick Evidence Verifier",
          verifierEffort,
        ),
      );
      return new CodexAppServerBackend(verifierClient, {
        model: verifierModel,
        reasoningEffort: verifierEffort,
        deliberateReasoningEffort: CODEX_VERIFIER_ADJUDICATION_EFFORT,
      });
    },
  });
  const session = new SidekickSession({
    backend: new CodexAppServerBackend(client, { model, reasoningEffort: effort }),
    evidenceVerifier,
    captureSessionId: `synthetic-meridian-${run}`,
    brief: preparedBrief(fixture),
    onPublish: (publication) => publications.push(publication),
  });
  const semanticJudge = new SidekickSemanticJudge({
    backend: new CodexAppServerBackend(judgeClient, { model, reasoningEffort: effort }),
  });

  try {
    const [backend, semanticBackend, verifierBackend] = await Promise.all([
      session.start({ cwd }),
      semanticJudge.start({ cwd }),
      evidenceVerifier.start({ cwd }),
    ]);
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
      turn_2_evidence_ids: roleFlip?.publication?.decision?.evidence_ids ?? [],
    });
    const semantic = await semanticJudge.grade({
      fixture,
      responses: {
        turn_1: {
          text: proactiveText,
          evidence_ids: proactive?.publication?.decision?.evidence_ids ?? [],
        },
        turn_2: {
          text: roleFlipText,
          evidence_ids: roleFlip?.publication?.decision?.evidence_ids ?? [],
        },
      },
    });
    const semanticCalibration = run === 1
      ? await semanticJudge.calibrate({
          fixture,
          examples: meridianSemanticCalibrationCases,
        })
      : null;
    return {
      run,
      provider: backend.provider,
      requested_model: model,
      requested_effort: effort,
      requested_verifier_model: verifierModel,
      requested_verifier_effort: verifierEffort,
      model: backend.model,
      backend_session_id: backend.sessionId,
      semantic_judge_provider: semanticBackend.provider,
      semantic_judge_model: semanticBackend.model,
      semantic_judge_session_id: semanticBackend.sessionId,
      verifier_provider: verifierBackend.provider,
      verifier_model: verifierBackend.model,
      service_tier: backend.serviceTier,
      latency: {
        proactive: proactive?.publication?.latency ?? null,
        role_flip: roleFlip?.publication?.latency ?? null,
      },
      responses: {
        proactive_hero_insight: proactiveText,
        procurement_role_flip: roleFlipText,
      },
      response_evidence_ids: {
        proactive_hero_insight: proactive?.publication?.decision?.evidence_ids ?? [],
        procurement_role_flip: roleFlip?.publication?.decision?.evidence_ids ?? [],
      },
      published_count: publications.length,
      golden,
      semantic,
      semantic_calibration: semanticCalibration,
      trace: session.trace,
    };
  } finally {
    semanticJudge.close();
    await evidenceVerifier.close();
    await session.stop();
    await fs.rm(cwd, { recursive: true, force: true });
  }
}

async function main() {
  const options = parseArgs(process.argv);
  const configuredProviderPath = path.isAbsolute(options.codex)
    ? options.codex
    : execFileSync("/usr/bin/which", [options.codex], { encoding: "utf8" }).trim();
  const providerExecutable = await attestSidekickProviderExecutable(
    configuredProviderPath,
  );
  const fixture = JSON.parse(await fs.readFile(options.fixture, "utf8"));
  if (fixture.schema_version !== 1 || fixture.content_origin !== "synthetic") {
    throw new Error("Sidekick session eval accepts only schema-v1 synthetic fixtures");
  }

  const runs = [];
  for (let run = 1; run <= options.repeat; run += 1) {
    runs.push(await runOnce({
      fixture,
      codex: providerExecutable.path,
      model: options.model,
      effort: options.effort,
      verifierModel: options.verifierModel,
      verifierEffort: options.verifierEffort,
      run,
    }));
  }
  const verifierCalibration = await runVerifierCalibration({
    codex: providerExecutable.path,
    verifierModel: options.verifierModel,
    verifierEffort: options.verifierEffort,
  });
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
  const totalMedian = median(
    latencySamples.map((sample) => sample.total_ms).filter(Number.isFinite),
  );
  const serviceTargetPassCount = latencySamples.filter(
    (sample) => Number.isFinite(sample.total_ms) &&
      sample.total_ms <= options.serviceTargetTotalMs,
  ).length;
  const qualityPassed = runs.every((run) => run.golden.passed);
  const semanticQualityPassed = runs.every((run) => run.semantic.passed);
  const semanticCalibrationPassed = runs[0]?.semantic_calibration?.passed === true;
  const verifierCalibrationPassed = verifierCalibration.passed === true;
  const modelMatched = runs.every((run) => run.model === options.model);
  const latencyPassed =
    firstTokenP95 !== null &&
    totalMedian !== null &&
    totalP95 !== null &&
    firstTokenP95 <= options.maxFirstTokenMs &&
    totalMedian <= options.maxMedianTotalMs &&
    serviceTargetPassCount >= options.minServiceTargetSamples &&
    totalP95 <= options.maxTotalMs;
  const providerExecutableAfter = await attestSidekickProviderExecutable(
    providerExecutable.path,
  );
  if (!sidekickProviderAttestationMatches(providerExecutableAfter, providerExecutable)) {
    throw new Error("Codex provider executable changed during Sidekick evaluation");
  }
  const report = {
    schema_version: 1,
    fixture_id: fixture.id,
    benchmark: "persistent-provider-neutral-sidekick",
    requested_model: options.model,
    requested_effort: options.effort,
    requested_verifier_model: options.verifierModel,
    requested_verifier_effort: options.verifierEffort,
    provider_executable: providerExecutable,
    source_binding: await currentSidekickQualitySourceBinding(repoRoot),
    runs,
    verifier_calibration: verifierCalibration,
    aggregate: {
      quality_passed: qualityPassed,
      semantic_quality_passed: semanticQualityPassed,
      semantic_calibration_passed: semanticCalibrationPassed,
      verifier_calibration_passed: verifierCalibrationPassed,
      model_matched: modelMatched,
      latency_passed: latencyPassed,
      passed:
        qualityPassed &&
        semanticQualityPassed &&
        semanticCalibrationPassed &&
        verifierCalibrationPassed &&
        modelMatched &&
        latencyPassed,
      first_token_p95_ms: firstTokenP95,
      total_median_ms: totalMedian,
      service_target_pass_count: serviceTargetPassCount,
      total_sample_count: latencySamples.length,
      total_p95_ms: totalP95,
      budgets: {
        max_first_token_p95_ms: options.maxFirstTokenMs,
        max_total_median_ms: options.maxMedianTotalMs,
        service_target_total_ms: options.serviceTargetTotalMs,
        min_service_target_pass_count: options.minServiceTargetSamples,
        max_total_p95_ms: options.maxTotalMs,
      },
    },
  };
  const serialized = `${JSON.stringify(report, null, 2)}\n`;
  if (options.output) await fs.writeFile(path.resolve(repoRoot, options.output), serialized);
  else process.stdout.write(serialized);
  if (options.producerReceipt) {
    if (!options.output) {
      throw new Error("--producer-receipt requires --output so stdout remains a receipt-only lane");
    }
    process.stdout.write(`${JSON.stringify({
      schema_version: 1,
      artifact_sha256: createHash("sha256").update(serialized).digest("hex"),
      source_binding: report.source_binding,
      strategist_session_ids: runs.map((run) => run.backend_session_id),
      semantic_judge_session_ids: runs.map((run) => run.semantic_judge_session_id),
      verifier_calibration_session_ids: verifierCalibration.session_ids,
      provider_executable: providerExecutableAfter,
    })}\n`);
  }
  return report.aggregate.passed ? 0 : 1;
}

process.exitCode = await main();
