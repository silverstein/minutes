import { createHash } from "node:crypto";
import os from "node:os";
import fs from "node:fs/promises";
import path from "node:path";

import {
  CodexAppServerClient,
  configuredMcpDisableArgs,
} from "./codex_app_server.mjs";
import {
  CODEX_REALTIME_EFFORT,
  CODEX_REALTIME_MODEL,
  CodexAppServerBackend,
} from "./sidekick_provider.mjs";
import {
  semanticJudgeCriteria,
  SidekickSemanticJudge,
} from "./sidekick_semantic_judge.mjs";
import { sidekickQualitySourceBindingMatches } from "./sidekick_quality_source_binding.mjs";
import {
  attestSidekickProviderExecutable,
  sidekickProviderAttestationMatches,
} from "./sidekick_provider_attestation.mjs";

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

export function sidekickSemanticResponseSha256(responses) {
  return sha256(Buffer.from(JSON.stringify({
    turn_1: {
      text: String(responses?.turn_1?.text ?? ""),
      evidence_ids: Array.isArray(responses?.turn_1?.evidence_ids)
        ? responses.turn_1.evidence_ids
        : [],
    },
    turn_2: {
      text: String(responses?.turn_2?.text ?? ""),
      evidence_ids: Array.isArray(responses?.turn_2?.evidence_ids)
        ? responses.turn_2.evidence_ids
        : [],
    },
  })));
}

export function semanticVerdictConjunction(verdict) {
  return [
    ...semanticJudgeCriteria.turn_1.map((criterion) => verdict?.turn_1?.[criterion]),
    ...semanticJudgeCriteria.turn_2.map((criterion) => verdict?.turn_2?.[criterion]),
  ].every((value) => value === true);
}

export function sidekickExactSemanticReceiptPasses(
  receipt,
  responses,
  sourceBinding,
  providerExecutable,
) {
  const conjunction = semanticVerdictConjunction(receipt?.verdict);
  return receipt?.schema_version === 1 &&
    receipt?.provider === "codex-app-server" &&
    receipt?.model === CODEX_REALTIME_MODEL &&
    sidekickProviderAttestationMatches(
      receipt?.provider_executable,
      providerExecutable,
    ) &&
    receipt?.response_sha256 === sidekickSemanticResponseSha256(responses) &&
    receipt?.verdict?.computed_pass === conjunction &&
    receipt?.verdict?.overall_pass === conjunction &&
    receipt?.verdict?.passed === conjunction &&
    conjunction &&
    sidekickQualitySourceBindingMatches(receipt?.source_binding, sourceBinding);
}

export async function runSidekickExactSemanticGate({
  fixture,
  responses,
  sourceBinding,
  codex,
}) {
  const providerExecutable = await attestSidekickProviderExecutable(codex);
  const cwd = await fs.mkdtemp(path.join(os.tmpdir(), "minutes-sidekick-exact-semantic-"));
  const mcpDisableArgs = await configuredMcpDisableArgs();
  const client = new CodexAppServerClient({
    command: providerExecutable.path,
    args: [
      "--disable", "apps",
      "--disable", "plugins",
      "--config", 'service_tier="fast"',
      "--config", `model_reasoning_effort=${JSON.stringify(CODEX_REALTIME_EFFORT)}`,
      ...mcpDisableArgs,
      "--enable", "fast_mode",
      "app-server",
    ],
    cwd,
    requestTimeoutMs: 60_000,
    experimentalApi: true,
    clientInfo: {
      name: "minutes-sidekick-exact-semantic-gate",
      title: "Minutes Sidekick Exact Semantic Gate",
      version: "0.2.0",
    },
  });
  const judge = new SidekickSemanticJudge({
    backend: new CodexAppServerBackend(client, {
      model: CODEX_REALTIME_MODEL,
      reasoningEffort: CODEX_REALTIME_EFFORT,
    }),
  });
  try {
    const backend = await judge.start({ cwd });
    const verdict = await judge.grade({ fixture, responses });
    const providerExecutableAfter = await attestSidekickProviderExecutable(
      providerExecutable.path,
    );
    if (!sidekickProviderAttestationMatches(providerExecutableAfter, providerExecutable)) {
      throw new Error("Codex provider executable changed during exact semantic evaluation");
    }
    return {
      schema_version: 1,
      provider: backend.provider,
      model: backend.model,
      provider_executable: providerExecutable,
      response_sha256: sidekickSemanticResponseSha256(responses),
      source_binding: sourceBinding,
      verdict,
    };
  } finally {
    judge.close();
    await fs.rm(cwd, { recursive: true, force: true });
  }
}
