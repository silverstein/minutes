import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

const QUALITY_SURFACE_PATHS = Object.freeze([
  "crates/core/src/context_card.rs",
  "crates/core/src/live_sidekick/engine.rs",
  "crates/core/src/live_sidekick/provider.rs",
  "crates/core/src/live_sidekick/session.rs",
  "tauri/src-tauri/src/codex_reasoning_backend.rs",
  "resources/live_sidekick/base_instructions.txt",
  "resources/live_sidekick/developer_instructions.txt",
  "resources/live_sidekick/intervention_contract.json",
  "resources/live_sidekick/verifier_base_instructions.txt",
  "resources/live_sidekick/verifier_developer_instructions.txt",
  "resources/live_sidekick/codex_realtime_model.txt",
  "resources/live_sidekick/codex_realtime_effort.txt",
  "resources/live_sidekick/codex_verifier_model.txt",
  "resources/live_sidekick/codex_verifier_effort.txt",
  "resources/live_sidekick/codex_verifier_adjudication_effort.txt",
  "tauri/src/assets/app-icon.png",
  "scripts/lib/sidekick_provider.mjs",
  "scripts/lib/sidekick_provider_attestation.mjs",
  "scripts/lib/sidekick_session.mjs",
  "scripts/lib/sidekick_evidence_verifier.mjs",
  "scripts/lib/sidekick_semantic_judge.mjs",
  "scripts/sidekick_session_eval.mjs",
  "tests/eval/sidekick_rehearsal_golden.mjs",
  "tests/eval/sidekick_semantic_calibration.mjs",
  "tests/eval/sidekick_verifier_calibration.mjs",
  "tests/fixtures/sidekick_compaction/v1/cases.json",
  "tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json",
]);

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

export async function currentSidekickQualitySourceBinding(repoRoot) {
  const digest = createHash("sha256");
  for (const relativePath of QUALITY_SURFACE_PATHS) {
    const bytes = await fs.readFile(path.join(repoRoot, relativePath));
    digest.update(relativePath);
    digest.update("\0");
    digest.update(bytes);
    digest.update("\0");
  }
  const fixtureBytes = await fs.readFile(path.join(
    repoRoot,
    "tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json",
  ));
  return {
    git_commit: execFileSync("git", ["rev-parse", "HEAD"], {
      cwd: repoRoot,
      encoding: "utf8",
    }).trim(),
    quality_surface_sha256: digest.digest("hex"),
    fixture_sha256: sha256(fixtureBytes),
  };
}

export function sidekickQualitySourceBindingMatches(actual, expected) {
  return /^[a-f0-9]{40,64}$/.test(actual?.git_commit ?? "") &&
    actual?.git_commit === expected?.git_commit &&
    /^[a-f0-9]{64}$/.test(actual?.quality_surface_sha256 ?? "") &&
    actual?.quality_surface_sha256 === expected?.quality_surface_sha256 &&
    /^[a-f0-9]{64}$/.test(actual?.fixture_sha256 ?? "") &&
    actual?.fixture_sha256 === expected?.fixture_sha256;
}

export const sidekickQualitySurfacePaths = QUALITY_SURFACE_PATHS;
