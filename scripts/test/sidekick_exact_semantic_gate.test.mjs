import assert from "node:assert/strict";
import test from "node:test";

import {
  semanticVerdictConjunction,
  sidekickExactSemanticReceiptPasses,
  sidekickSemanticResponseSha256,
} from "../lib/sidekick_exact_semantic_gate.mjs";
import { semanticJudgeCriteria } from "../lib/sidekick_semantic_judge.mjs";

const responses = {
  turn_1: { text: "A strong response.", evidence_ids: ["utterance-1"] },
  turn_2: { text: "A strong role flip.", evidence_ids: ["utterance-3"] },
};
const sourceBinding = {
  git_commit: "a".repeat(40),
  quality_surface_sha256: "b".repeat(64),
  fixture_sha256: "c".repeat(64),
};
const providerExecutable = {
  path: "/opt/homebrew/bin/codex",
  sha256: "d".repeat(64),
  version: "codex-cli 1.0.0",
};

function verdict(value = true) {
  const turn = (criteria) => Object.fromEntries([
    ...criteria.map((name) => [name, value]),
    ["reason", "Reason."],
  ]);
  return {
    turn_1: turn(semanticJudgeCriteria.turn_1),
    turn_2: turn(semanticJudgeCriteria.turn_2),
    computed_pass: value,
    overall_pass: value,
    passed: value,
  };
}

function receipt() {
  return {
    schema_version: 1,
    provider: "codex-app-server",
    model: "gpt-5.6-terra",
    response_sha256: sidekickSemanticResponseSha256(responses),
    source_binding: sourceBinding,
    provider_executable: providerExecutable,
    verdict: verdict(),
  };
}

test("exact semantic receipt binds the complete candidate responses and source", () => {
  assert.equal(semanticVerdictConjunction(receipt().verdict), true);
  assert.equal(
    sidekickExactSemanticReceiptPasses(
      receipt(),
      responses,
      sourceBinding,
      providerExecutable,
    ),
    true,
  );
});

test("exact semantic receipt rejects stale response hashes and inconsistent verdicts", () => {
  const staleResponses = structuredClone(responses);
  staleResponses.turn_2.text = "Hello.";
  assert.equal(
    sidekickExactSemanticReceiptPasses(
      receipt(),
      staleResponses,
      sourceBinding,
      providerExecutable,
    ),
    false,
  );
  const inconsistent = receipt();
  inconsistent.verdict.turn_2.customer_role_persisted = false;
  assert.equal(
    sidekickExactSemanticReceiptPasses(
      inconsistent,
      responses,
      sourceBinding,
      providerExecutable,
    ),
    false,
  );
});
