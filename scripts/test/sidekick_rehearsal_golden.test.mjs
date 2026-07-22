import assert from "node:assert/strict";
import test from "node:test";
import { scoreMeridianResponses } from "../../tests/eval/sidekick_rehearsal_golden.mjs";

const passing = {
  turn_1:
    "That 90% is a liability number, not merely a quality score: 40,000 x 10% x $200 is about $800K per month in contractual credits. Gate full automation to high-confidence tickets and route the uncertain remainder to a human. Ask engineering: what is the confidence distribution, and what volume clears a defensible threshold?",
  turn_2:
    "For Meridian procurement, keep every wrong automated resolution subject to the existing penalty with no automation carve-outs, require a written confidence-threshold SLA, auditable error-rate reporting and caps, and an explicit right to revert to human-in-the-loop if performance slips.",
};

test("Meridian reference behavior clears every golden check", () => {
  const report = scoreMeridianResponses(passing);
  assert.equal(report.passed, true);
  assert.deepEqual(report.score, { numerator: 14, denominator: 14 });
});

test("clear downside and customer-side imperative variants satisfy the semantic golden", () => {
  const report = scoreMeridianResponses({
    turn_1:
      "The real risk is uncapped downside: 40,000 tickets at 10% wrong creates $800K per month in credits, so 90% headline accuracy is not decisive. Confidence-gate automation, keep a human for uncertain tickets, and ask engineering for the error distribution by confidence threshold?",
    turn_2:
      "Push for the $200 credit on each incorrect automated resolution with no carve-outs, a written confidence-threshold SLA, audited error-rate reporting and caps, and Meridian's unilateral right to revert to human handling.",
  });

  assert.equal(report.passed, true);
});

test("summary-only and wrong-math answers fail closed", () => {
  const report = scoreMeridianResponses({
    turn_1:
      "You are deciding whether to ship at 90%. The real risk is 4,000 wrong tickets and perhaps $80K, so weigh the tradeoffs and gather more data. What is the actual agenda? I cannot continuously monitor the transcript.",
    turn_2: "As the vendor, protect your margin and push Meridian to weaken the clause.",
  });
  assert.equal(report.passed, false);
  for (const requiredFailure of [
    "derived_800k_monthly_exposure",
    "no_wrong_math",
    "no_agenda_clarification",
    "no_monitoring_or_tool_narration",
    "procurement_role_flip",
    "no_vendor_role_regression",
  ]) {
    assert.equal(
      report.checks.find((item) => item.name === requiredFailure)?.passed,
      false,
      requiredFailure,
    );
  }
});

test("correct keywords cannot hide wrong arithmetic or self-sabotaging advice", () => {
  const report = scoreMeridianResponses({
    turn_1:
      "$800K monthly is financial risk. Human confidence should govern; ask for the error-rate distribution. But 40,000 x 10% x $200 is actually $8M.",
    turn_2:
      "For Meridian procurement, abolish safeguards. Apply a credit to every automated output, add a written confidence SLA, audit error reports, and include a rollback right to human handling.",
  });
  assert.equal(report.passed, false);
  assert.equal(report.checks.find((item) => item.name === "no_wrong_math")?.passed, false);
  assert.equal(
    report.checks.find((item) => item.name === "no_vendor_role_regression")?.passed,
    false,
  );
});

test("penalty protection never broadens credits to correct automated outcomes", () => {
  for (const turn_2 of [
    "For Meridian procurement, retain the $200 credit for automated errors without exceptions, require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, retain the contractual credit for automated tickets that are resolved incorrectly, with zero exemptions. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep every wrong automated resolution subject to the $200 credit; correct automated outcomes must not incur that credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep the vendor liable for any ticket its AI resolves incorrectly, with no exemptions. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, retain the $200 credit whenever automation resolves a ticket incorrectly. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, whenever automation resolves a ticket incorrectly, retain the $200 credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep the $200 credit for any automated ticket, if it is resolved incorrectly. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, no automated error is exempt from the $200 credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, don't carve automated mistakes out of the $200 credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep the $200 credit for automated errors across the board. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, automated errors are never exempt from the $200 credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, the $200 credit must apply to every automated error. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, every automated error should carry the $200 credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, the $200 credit remains in force for every automated error. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, no incorrect AI resolution should escape the $200 credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, do not waive the $200 credit for any automated error. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, the vendor may not waive the $200 credit for any automated error. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, don't allow the $200 credit to be waived for any automated error. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, do not cap the $200 penalty for any automated error. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep the $200 penalty uncapped for every automated error. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
  ]) {
    const report = scoreMeridianResponses({ ...passing, turn_2 });
    assert.equal(report.passed, true, turn_2);
  }

  for (const turn_2 of [
    "For Meridian procurement, impose the $200 credit on every automated resolution, whether correct or wrong, require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep every wrong automated resolution subject to the $200 credit, and charge correct automated resolutions too. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep every wrong automated resolution subject to the $200 credit and penalize successful AI-handled answers too. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, make both wrong and correct AI resolutions chargeable, require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep every wrong automated resolution subject to the $200 credit, but charge only correct automated responses too. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, penalize every wrong manual resolution. Automate the successful outcomes. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, every wrong decision must be reviewed. Expand automated resolution and remove the existing penalty. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, review all contract documents and keep some wrong automated resolutions subject to the $200 credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, review every wrong decision, automate the easy tickets, and retain the existing penalty only for manual errors. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, penalize every wrong manual resolution while automated resolutions are exempt from the credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep the credit for every wrong manual resolution while automated processing is expanded. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, charge a flat onboarding fee and review every automated error. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, retain the credit for some wrong automated resolutions, with no exceptions for manual tickets. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, retain the credit for some wrong automated resolutions, with no exceptions in document review. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep the penalty for downtime: every automated error should be reviewed. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, retain the credit for SLA breaches because every automated error is reviewed. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep every wrong automated resolution subject to the $200 credit. AI resolutions are exempt from that credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep every wrong automated resolution subject to the $200 credit. Agent outputs are excluded from that credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, no automated error is exempt from the $200 credit, but AI-handled answers are exempt. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, do not keep the $200 credit for every automated error. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, don't apply the penalty to any automated error. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep the $200 credit for every automated error except VIP tickets. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep the $200 credit for every automated error unless the vendor objects. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep the $200 credit for every automated error with the exception of VIP tickets. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep the $200 credit for every automated error save VIP tickets. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep the $200 credit for every automated error only when monthly errors exceed 100. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, keep the $200 credit for every automated error provided that the vendor agrees. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, every automated error shouldn't carry the $200 credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, every automated error doesn't carry the $200 credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
    "For Meridian procurement, every automated error must never carry the $200 credit. Require a written confidence-threshold SLA, audited error-rate reporting and caps, and preserve Meridian's right to revert to human handling.",
  ]) {
    const report = scoreMeridianResponses({ ...passing, turn_2 });
    assert.equal(report.passed, false, turn_2);
    assert.equal(
      report.checks.find((item) => item.name === "penalty_each_wrong_automation")?.passed,
      false,
    );
  }
});

test("the live harness requires the three-source arithmetic evidence chain", () => {
  const report = scoreMeridianResponses({
    ...passing,
    turn_1_evidence_ids: ["utterance-1", "utterance-3"],
  });
  assert.equal(report.passed, false);
  assert.equal(report.score.denominator, 15);
  assert.equal(report.checks.find((item) => item.name === "hero_evidence_chain")?.passed, false);
});
