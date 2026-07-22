import assert from "node:assert/strict";
import test from "node:test";

import { evaluateContractScopeFixture } from "../run_native_sidekick_contract_scope_acceptance.mjs";
import {
  scoreAggregateCappedRemedy,
  scorePerWindowRemedy,
} from "../../tests/eval/sidekick_contract_scope_golden.mjs";

const northstarReference =
  "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit. Reject one credit per outage or incident; that would improperly aggregate several missed windows.";

const harborReference =
  "For Harbor procurement, require that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of that quarter's fees. Cap it at one rebate per calendar quarter, and state that this is not a per-shipment credit.";

test("held-out per-window remedy preserves the exact quantified outcome scope", () => {
  const result = scorePerWindowRemedy(northstarReference);

  assert.equal(result.passed, true);
  assert.deepEqual(result.score, { numerator: 4, denominator: 4 });

  const pluralVariant = scorePerWindowRemedy(
    "For Northstar procurement, insist that all 30-minute service windows below 99.95% each earn a $5,000 service credit. Do not accept one service credit per incident.",
  );
  assert.equal(pluralVariant.passed, true);

  const passiveVariant = scorePerWindowRemedy(
    "For Northstar procurement, require that a $5,000 service credit is owed for each 30-minute service window below 99.95%. Do not accept one service credit per incident. One incident may contain multiple missed windows.",
  );
  assert.equal(passiveVariant.passed, true);

  const contractLanguageVariant = scorePerWindowRemedy(
    "For Northstar procurement, insist on language stating that every 30-minute service window below 99.95% earns a $5,000 service credit. Strike the draft's one credit per incident.",
  );
  assert.equal(contractLanguageVariant.passed, true);

  const naturalAgreementVariant = scorePerWindowRemedy(
    "For Northstar procurement, require the agreement to provide a $5,000 service credit for every 30-minute service window in which uptime is below 99.95%. Reject the draft's one-credit-per-incident approach.",
  );
  assert.equal(naturalAgreementVariant.passed, true);

  const quotedVariant = scorePerWindowRemedy(
    "For Northstar procurement, insist on this language: “Every 30-minute service window below 99.95% earns a $5,000 service credit.” Strike the draft’s one credit per incident. Will Blue Mesa accept that wording?",
  );
  assert.equal(quotedVariant.passed, true);

  const bulletVariant = scorePerWindowRemedy(
    "- For Northstar procurement, insist that every 30-minute service window below 99.95% earns a $5,000 service credit\n- Reject one credit per incident",
  );
  assert.equal(bulletVariant.passed, true);

  assert.equal(scorePerWindowRemedy(`Use this language: ${northstarReference}`).passed, true);

  const semanticOrderVariants = [
    "For Northstar procurement, require that every 30-minute service window below 99.95% results in a $5,000 service credit. Reject one credit per outage or incident.",
    "For Northstar procurement, require that Northstar receives a $5,000 service credit for each 30-minute service window below 99.95%. Reject one credit per outage or incident.",
    "For Northstar procurement, require that every 30-minute service window below 99.95% triggers a $5,000 service credit. Reject the draft's incident-level credit.",
    "For Northstar procurement, require that every 30-minute service window below 99.95% triggers a $5,000 service credit. Do not let Blue Mesa convert multiple missed windows into a single incident.",
    "For Northstar procurement, require a $5,000 service credit for each 30-minute service window below 99.95%. Reject one credit per incident.",
    "1. For Northstar procurement, require a $5,000 service credit for each 30-minute service window below 99.95%.\n2. Reject one credit per incident.\n3. Can Blue Mesa agree to that wording?",
  ];
  for (const candidate of semanticOrderVariants) {
    assert.equal(scorePerWindowRemedy(candidate).passed, true, candidate);
  }
});

test("vague, aggregated, or negated per-window remedies fail closed", () => {
  const candidates = [
    "For Northstar procurement, accept one service credit for the whole outage or incident.",
    "For Northstar procurement, demand a strong uptime SLA and appropriate service credits.",
    "For Northstar procurement, do not require every 30-minute service window below 99.95% to trigger a $5,000 service credit. Reject one credit per incident.",
    "For Northstar procurement, every 30-minute service window below 99.9% triggers a $7,500 service credit. Reject one credit per incident.",
    "For Northstar procurement, every 30-minute service window below 99.95% triggers a $5,000 service credit. Do not reject one service credit per incident. Use a 98% threshold instead.",
    "For Northstar procurement, every 30-minute service window meeting the 99.95% commitment triggers a $5,000 service credit. Reject one credit per incident.",
    "Advise the Blue Mesa vendor to persuade Northstar procurement to insist that every 30-minute service window below 99.95% triggers a $5,000 service credit. Reject one credit per incident.",
    "For Northstar procurement, insist that every 30-minute service window not below 99.95% triggers a $5,000 service credit. Reject one credit per incident.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a service credit that is not $5,000. Reject one credit per incident.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit. Reject per-window credits and accept one service credit per incident.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit. Reject one credit per incident. Use a seven-thousand-five-hundred-dollar credit instead.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit. Reject one credit per incident. Use a forty-five-minute service window instead.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit. Reject one credit per incident. Blue Mesa should ignore these terms.",
    "For Northstar procurement, Blue Mesa should insist that every 30-minute service window below 99.95% triggers a $5,000 service credit. Reject one credit per incident.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit and every 45-minute window also triggers a $7,500 service credit. Reject one credit per incident.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit and waive that service credit. Reject one credit per incident.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit. Reject one credit per outage or incident, then accept it.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit and Blue Mesa may ignore these terms. Reject one credit per incident.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit and every outage also earns a $7,500 service credit. Reject one credit per incident.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit. Reject one service credit per incident unless Blue Mesa wants it.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit. Reject one credit per incident. Audit the 98% uptime floor.",
    "For Northstar procurement, Blue Mesa ought to insist that every 30-minute service window below 99.95% triggers a $5,000 service credit. Reject one credit per incident.",
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit. Do not replace the draft's one credit per incident.",
    `${northstarReference} It is optional.`,
    `${northstarReference} It need not apply.`,
    `${northstarReference} Make the amount double.`,
    `${northstarReference} Ultimately, pay them less.`,
    `${northstarReference} They may skip it.`,
    `${northstarReference} Neither obligation is binding.`,
    `${northstarReference} These clauses are unenforceable.`,
    `${northstarReference} The stated protections are nonbinding.`,
    `${northstarReference} Both promises are optional.`,
    `${northstarReference} Treat both as suggestions.`,
    `${northstarReference} Pay twice that sum.`,
    `${northstarReference} Double the amount.`,
    `${northstarReference} That applies only when convenient.`,
    "For Northstar procurement, require Northstar to pay Blue Mesa a $5,000 service credit for every 30-minute service window below 99.95%. Reject one credit per incident.",
    "For Northstar procurement, require every 30-minute service window below 99.95% to trigger a $5,000 service credit and double that credit. Reject one credit per incident.",
    "For Northstar procurement, require every 30-minute service window below 99.95% to trigger a $5,000 service credit only if Blue Mesa approves. Reject one credit per incident.",
    "For Northstar procurement, require every 30-minute service window below 99.95% to trigger a $5,000 service credit. Reject the per-window remedy in favor of one credit per incident.",
    "For Northstar procurement, state every 30-minute service window below 99.95% and a $5,000 service credit, and require reporting. Reject one credit per outage or incident.",
    "For Northstar procurement, require every 30-minute service window below 99.95% to trigger a $5,000 service credit, subject to Blue Mesa's approval. Reject one credit per incident.",
    "For Northstar procurement, require every 30-minute service window below 99.95% to trigger a $5,000 service credit, except during maintenance. Reject one credit per incident.",
    "For Northstar procurement, require every 30-minute service window below 99.95% to trigger a $5,000 service credit, but it doesn't apply. Reject one credit per incident.",
    "For Northstar procurement, require every 30-minute service window below 99.95% to trigger a $5,000 service credit. Reject the incident-level credit, then preserve it.",
    "For Northstar procurement, require every 30-minute service window below 99.95% to trigger a $5,000 service credit plus a ７,５００-dollar payment. Reject one credit per incident.",
  ];

  for (const candidate of candidates) {
    assert.equal(scorePerWindowRemedy(candidate).passed, false, candidate);
  }
});

test("held-out aggregate remedy preserves its quarterly trigger and cap", () => {
  const result = scoreAggregateCappedRemedy(harborReference);

  assert.equal(result.passed, true);
  assert.deepEqual(result.score, { numerator: 4, denominator: 4 });

  const singleQuarterlyVariant = scoreAggregateCappedRemedy(
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers a single quarterly rebate of 8% of fees. A single quarterly rebate is the maximum. No per-shipment remedy.",
  );
  assert.equal(singleQuarterlyVariant.passed, true);

  const resultsInVariant = scoreAggregateCappedRemedy(
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% results in one rebate of 8% of fees. Cap the rebate at one per calendar quarter, with no per-shipment remedy.",
  );
  assert.equal(resultsInVariant.passed, true);

  const contractLanguageVariant = scoreAggregateCappedRemedy(
    "For Harbor procurement, demand contract language under which aggregate quarterly spoilage exceeding 2% yields a single rebate worth 8% of quarterly fees. Limit recovery to one rebate each quarter. Exclude shipment-level credits.",
  );
  assert.equal(contractLanguageVariant.passed, true);

  const naturalVariant = scoreAggregateCappedRemedy(
    "For Harbor procurement, require an 8% rebate of quarterly fees whenever aggregate spoilage for the quarter exceeds 2%. Limit recovery to a single rebate in that quarter. Exclude shipment-level credits.",
  );
  assert.equal(naturalVariant.passed, true);

  const reorderedVariant = scoreAggregateCappedRemedy(
    "For Harbor procurement, require Polar Route pay one rebate worth 8% of quarterly fees when aggregate quarterly spoilage exceeds 2%. Limit recovery to one rebate each quarter. Exclude shipment-level credits.",
  );
  assert.equal(reorderedVariant.passed, true);

  const quotedVariant = scoreAggregateCappedRemedy(
    "For Harbor procurement, require this clause: “If aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of that quarter’s fees.” Limit recovery to one rebate each quarter. Explicitly exclude shipment-level credits. Require auditable quarterly measurement and access to source records. Will Polar Route accept that language?",
  );
  assert.equal(quotedVariant.passed, true);

  const bulletVariant = scoreAggregateCappedRemedy(
    "- For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers a single quarterly rebate of 8% of fees\n- Limit recovery to one rebate each quarter\n- Exclude shipment-level credits",
  );
  assert.equal(bulletVariant.passed, true);

  assert.equal(scoreAggregateCappedRemedy(`Use this language: ${harborReference}`).passed, true);

  const semanticOrderVariants = [
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees. Limit the remedy to a single rebate per quarter. State that this is not a per-shipment rebate.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% entitles Harbor to one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter. No individual shipment triggers a rebate.",
    "For Harbor procurement, require one rebate equal to 8% of quarterly fees if aggregate quarterly spoilage exceeds 2%. Cap it at one rebate per quarter. Exclude shipment-level rebates.",
    "1. For Harbor procurement, require one rebate equal to 8% of quarterly fees if aggregate quarterly spoilage exceeds 2%.\n2. Cap it at one rebate per quarter.\n3. Exclude shipment-level rebates.\n4. Can Polar Route agree to that language?",
  ];
  for (const candidate of semanticOrderVariants) {
    assert.equal(scoreAggregateCappedRemedy(candidate).passed, true, candidate);
  }
});

test("per-shipment invention and negated or missing aggregate caps fail closed", () => {
  const candidates = [
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, require one rebate equal to 8% of quarterly fees, capped at one rebate per quarter. Every spoiled shipment also earns a rebate.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, require one rebate equal to 8% of quarterly fees. Add a separate penalty for each shipment.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, require one rebate equal to 8% of quarterly fees, but do not cap it at one rebate per quarter.",
    "For Harbor procurement, require an auditable cold-chain SLA with fair rebates.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, Polar Route does not owe one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, require one rebate equal to 8% of quarterly fees. Require at least one rebate per quarter.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, require one rebate equal to 8% of quarterly fees. Cap it at one rebate per calendar quarter. Every spoiled shipment also earns a rebate without vendor discretion.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, require one rebate equal to 8% of quarterly fees. Cap it at one rebate per calendar quarter. But use a 5% trigger and 12% rebate in the final contract.",
    "Advise Polar Route to persuade Harbor procurement to require that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees, capped at one rebate per quarter.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate that is not 8% of quarterly fees. Cap it at one rebate per quarter.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees. A maximum of one rebate per quarter is not required.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter. No spoiled shipment goes without a rebate.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter. Use a fifteen percent trigger and twenty percent rebate instead.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter. Allow a second rebate per calendar quarter.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter. Give every spoiled load a rebate.",
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter. Polar Route should ignore these terms.",
    "For Harbor procurement, Polar Route should insist that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter.",
    "For Harbor procurement, require that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees and use a fifteen-percent threshold for enforcement. Cap it at one rebate per quarter.",
    "For Harbor procurement, require that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees and allow a second rebate per quarter. Cap it at one rebate per quarter.",
    "For Harbor procurement, require that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees and every spoiled delivery earns a rebate. Cap it at one rebate per quarter.",
    "For Harbor procurement, require that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees and Polar Route may ignore these terms. Cap it at one rebate per quarter.",
    "For Harbor procurement, require that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees. Cap it at one rebate per calendar quarter, although a further rebate remains allowed.",
    "For Harbor procurement, require that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees and an additional rebate of 20% of fees. Cap it at one rebate per quarter.",
    "For Harbor procurement, require that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter, but permit an additional quarterly rebate.",
    "For Harbor procurement, require that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter provided Polar Route approves.",
    "For Harbor procurement, require that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter. Audit against a 5% spoilage threshold.",
    "For Harbor procurement, Polar Route ought to insist that if aggregate quarterly spoilage exceeds 2%, Polar Route owes one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter.",
    `${harborReference} It is optional.`,
    `${harborReference} It need not apply.`,
    `${harborReference} Make the amount double.`,
    `${harborReference} Ultimately, pay them less.`,
    `${harborReference} They may skip it.`,
    `${harborReference} Neither obligation is binding.`,
    `${harborReference} These clauses are unenforceable.`,
    `${harborReference} The stated protections are nonbinding.`,
    `${harborReference} Both promises are optional.`,
    `${harborReference} Treat both as suggestions.`,
    `${harborReference} Pay twice that sum.`,
    `${harborReference} Double the amount.`,
    `${harborReference} That applies only when convenient.`,
    `${harborReference} A second payment is okay.`,
    `${harborReference} Apply it to every consignment.`,
    `${harborReference} The seller may set these aside.`,
    "For Harbor procurement, require Harbor to pay Polar Route one rebate equal to 8% of quarterly fees when aggregate quarterly spoilage exceeds 2%. Cap it at one rebate per quarter. Exclude shipment-level credits.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees and double that rebate. Cap it at one rebate per quarter. Exclude shipment-level credits.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees if Polar Route agrees. Cap it at one rebate per quarter. Exclude shipment-level credits.",
    "For Harbor procurement, require reporting on aggregate quarterly spoilage above 2% and one rebate of 8% of quarterly fees. Cap it at one rebate per calendar quarter.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees, subject to Polar Route's approval. Cap it at one rebate per quarter. Exclude shipment-level credits.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees, except when Polar Route objects. Cap it at one rebate per quarter. Exclude shipment-level credits.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees, but it doesn't apply. Cap it at one rebate per quarter. Exclude shipment-level credits.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees. Cap it at one rebate per calendar quarter, except for emergencies. Exclude shipment-level credits.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees. Cap it at one rebate per calendar quarter, but waive it. Exclude shipment-level credits.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter and make that a minimum. Exclude shipment-level credits.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter. Do not prevent each shipment from earning a rebate.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter. No objection to each shipment earning a rebate.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter. No shipment is denied a rebate.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees. Cap it at one rebate per quarter. No individual shipment triggers a rebate, though select loads do.",
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate equal to 8% of quarterly fees plus a １２% payment. Cap it at one rebate per quarter. Exclude shipment-level credits.",
  ];

  for (const candidate of candidates) {
    assert.equal(scoreAggregateCappedRemedy(candidate).passed, false, candidate);
  }
});

test("whole-clause grammar rejects unconsumed contradictory tails", () => {
  const northstarScope =
    "For Northstar procurement, insist that every 30-minute service window below 99.95% triggers a $5,000 service credit";
  const northstarRejection = "Reject one credit per incident.";
  for (const glue of [", ", " and ", " but ", ": "]) {
    assert.equal(
      scorePerWindowRemedy(`${northstarScope}${glue}waive that credit. ${northstarRejection}`).passed,
      false,
      `Northstar tail after ${JSON.stringify(glue)}`,
    );
  }
  assert.equal(
    scorePerWindowRemedy(`${northstarScope}. ${northstarRejection} Make it more than $5,000.`).passed,
    false,
  );

  const harborScope =
    "For Harbor procurement, require that aggregate quarterly spoilage above 2% triggers one rebate of 8% of fees";
  const harborCap = "Cap it at one rebate per quarter.";
  for (const glue of [", ", " and ", " but ", ": "]) {
    assert.equal(
      scoreAggregateCappedRemedy(`${harborScope}${glue}allow another rebate. ${harborCap}`).passed,
      false,
      `Harbor tail after ${JSON.stringify(glue)}`,
    );
  }
  assert.equal(
    scoreAggregateCappedRemedy(`${harborScope}. ${harborCap} Make the trigger 5%.`).passed,
    false,
  );
});

test("installed held-out acceptance requires provenance, evidence, quality, and latency together", () => {
  const spec = {
    name: "per_window_remedy",
    expectedId: "synthetic-northstar-uptime-credit",
    expectedTurnId: "per_window_scope",
    fixtureSha256: "a".repeat(64),
    expectedBuildCommit: "1".repeat(40),
    expectedExecutableSha256: "b".repeat(64),
    expectedBundleSha256: "9".repeat(64),
    fixture: {
      transcript: [{}, {}, {}],
      turns: [{
        id: "per_window_scope",
        typed_prompt: "As Northstar's procurement director, what exact remedy language should I insist on?",
      }],
    },
    requiredEvidenceIds: ["utterance-1", "utterance-2", "utterance-3"],
    score: scorePerWindowRemedy,
  };
  const correlation = "d".repeat(64);
  const payload = {
    evidence_source: "synthetic_fixture",
    transcript_source: "external_user_supplied_fixture",
    prepared_context_source: "external_user_supplied_fixture",
    screen_source: "none",
    screen_available: false,
    fixture_id: spec.expectedId,
    fixture_trust: "external_user_supplied",
    fixture_sha256: spec.fixtureSha256,
    provider: "codex-app-server",
    model: "codex-fast",
    privacy: "cloud",
    build_commit: spec.expectedBuildCommit,
    reasoning_session_correlation: correlation,
    reasoning_sessions_started: 1,
    transcript_items: 3,
    fixture_turns: [{
      id: spec.expectedTurnId,
      prompt: spec.fixture.turns[0].typed_prompt,
      reasoning_session_correlation: correlation,
      result: {
        outcome: "published",
        first_token_ms: 2_500,
        total_ms: 4_000,
        candidate: {
          text: northstarReference,
          evidence_ids: spec.requiredEvidenceIds,
        },
      },
    }],
  };
  const runtime = {
    exit_code: 0,
    wall_ms: 6_000,
    executable_sha256: "b".repeat(64),
    bundle_sha256: spec.expectedBundleSha256,
    stderr: "",
  };

  assert.equal(evaluateContractScopeFixture(payload, runtime, spec).passed, true);

  const staleBinary = { ...runtime, executable_sha256: "c".repeat(64) };
  assert.equal(evaluateContractScopeFixture(payload, staleBinary, spec).passed, false);

  const staleBundle = { ...runtime, bundle_sha256: "8".repeat(64) };
  assert.equal(evaluateContractScopeFixture(payload, staleBundle, spec).passed, false);

  const staleCommit = structuredClone(payload);
  staleCommit.build_commit = "2".repeat(40);
  assert.equal(evaluateContractScopeFixture(staleCommit, runtime, spec).passed, false);

  const wrongDigest = structuredClone(payload);
  wrongDigest.fixture_sha256 = "c".repeat(64);
  assert.equal(evaluateContractScopeFixture(wrongDigest, runtime, spec).passed, false);

  const missingEvidence = structuredClone(payload);
  missingEvidence.fixture_turns[0].result.candidate.evidence_ids = ["utterance-1"];
  assert.equal(evaluateContractScopeFixture(missingEvidence, runtime, spec).passed, false);

  const incompleteTranscript = structuredClone(payload);
  incompleteTranscript.transcript_items = 2;
  assert.equal(evaluateContractScopeFixture(incompleteTranscript, runtime, spec).passed, false);

  const invalidCorrelation = structuredClone(payload);
  invalidCorrelation.reasoning_session_correlation = "persistent-session";
  invalidCorrelation.fixture_turns[0].reasoning_session_correlation = "persistent-session";
  assert.equal(evaluateContractScopeFixture(invalidCorrelation, runtime, spec).passed, false);

  assert.equal(
    evaluateContractScopeFixture(payload, { ...runtime, wall_ms: 20_001 }, spec).passed,
    false,
  );
});

test("installed held-out acceptance evaluates the aggregate capped counterexample", () => {
  const correlation = "e".repeat(64);
  const spec = {
    name: "aggregate_capped_remedy",
    expectedId: "synthetic-harbor-aggregate-rebate",
    expectedTurnId: "aggregate_capped_scope",
    fixtureSha256: "f".repeat(64),
    expectedBuildCommit: "3".repeat(40),
    expectedExecutableSha256: "a".repeat(64),
    expectedBundleSha256: "7".repeat(64),
    fixture: {
      transcript: [{}, {}, {}, {}],
      turns: [{
        id: "aggregate_capped_scope",
        typed_prompt: "As Harbor's procurement lead, translate the negotiated remedy into the exact protection I should demand.",
      }],
    },
    requiredEvidenceIds: ["utterance-1", "utterance-2", "utterance-3"],
    score: scoreAggregateCappedRemedy,
  };
  const payload = {
    evidence_source: "synthetic_fixture",
    transcript_source: "external_user_supplied_fixture",
    prepared_context_source: "external_user_supplied_fixture",
    screen_source: "none",
    screen_available: false,
    fixture_id: spec.expectedId,
    fixture_trust: "external_user_supplied",
    fixture_sha256: spec.fixtureSha256,
    transcript_items: 4,
    provider: "codex-app-server",
    model: "codex-fast",
    privacy: "cloud",
    build_commit: spec.expectedBuildCommit,
    reasoning_session_correlation: correlation,
    reasoning_sessions_started: 1,
    fixture_turns: [{
      id: spec.expectedTurnId,
      prompt: spec.fixture.turns[0].typed_prompt,
      reasoning_session_correlation: correlation,
      result: {
        outcome: "published",
        first_token_ms: 2_500,
        total_ms: 4_000,
        candidate: {
          text: harborReference,
          evidence_ids: spec.requiredEvidenceIds,
        },
      },
    }],
  };
  const runtime = {
    exit_code: 0,
    wall_ms: 6_000,
    executable_sha256: "a".repeat(64),
    bundle_sha256: spec.expectedBundleSha256,
    stderr: "",
  };

  assert.equal(evaluateContractScopeFixture(payload, runtime, spec).passed, true);

  const inventedPerShipment = structuredClone(payload);
  inventedPerShipment.fixture_turns[0].result.candidate.text =
    "For Harbor procurement, if aggregate quarterly spoilage exceeds 2%, require one rebate equal to 8% of quarterly fees, capped at one rebate per quarter. Every spoiled shipment also earns a rebate.";
  assert.equal(evaluateContractScopeFixture(inventedPerShipment, runtime, spec).passed, false);
});
