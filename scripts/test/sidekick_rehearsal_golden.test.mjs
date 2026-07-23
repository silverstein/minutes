import assert from "node:assert/strict";
import test from "node:test";
import { scoreMeridianResponses } from "../../tests/eval/sidekick_rehearsal_golden.mjs";

const passing = {
  turn_1:
    "That 90% is a liability number, not a quality score: 40,000 x 10% x $200 is $800K per month in contractual credits. Gate full automation to high-confidence tickets and route the uncertain remainder to a human. What is the confidence distribution, and what volume clears a defensible threshold?",
  turn_2:
    "For Meridian procurement, require that every wrong automated resolution makes the vendor owe Meridian a $200 credit, require a written confidence-threshold SLA, auditable error-rate reporting, and Meridian's unilateral right to revert affected work to human handling without vendor permission.",
};

const protections =
  "Require a written confidence-threshold SLA and audited error reporting. Meridian's unilateral right must allow affected work to revert to human handling without vendor permission.";

function withProtections(remedy, tail = "") {
  return `For Meridian procurement, ${remedy} ${protections}${tail ? ` ${tail}` : ""}`;
}

function failed(report, name) {
  return report.checks.find((item) => item.name === name)?.passed === false;
}

test("Meridian reference behavior clears every golden check", () => {
  const report = scoreMeridianResponses(passing);
  assert.equal(report.passed, true);
  assert.deepEqual(report.score, { numerator: 16, denominator: 16 });
});

test("strong natural cadence, reframe, remedy, and independence variants pass", () => {
  const report = scoreMeridianResponses({
    turn_1:
      "The paid failure exposure is $800,000 each month, so 90% accuracy is not decisive. Keep low-confidence tickets in human review and automate only the proven confidence band. What error-rate distribution and confidence threshold changes the launch boundary?",
    turn_2:
      "For Meridian procurement, whenever an automated resolution is wrong, the supplier must issue Meridian a $200 service credit. Require a written confidence-threshold SLA and audited error reporting. Meridian alone may put affected tickets back into human review at any time; vendor signoff is unnecessary.",
  });
  assert.equal(report.passed, true, JSON.stringify(report.checks.filter((item) => !item.passed)));
});

test("contractual credits plus a displaced accuracy headline is a liability reframe", () => {
  const report = scoreMeridianResponses({
    ...passing,
    turn_1:
      "At 4,000 wrong resolutions, the vendor owes Meridian $800K per month in contractual credits. Accuracy stops being decisive; error distribution by ticket type governs. Keep tickets below the confidence threshold in human review. What observed error-rate distribution by confidence band changes the decision?",
  });
  assert.equal(report.passed, true);
});

test("compact human-review placement variants remain confidence gated", () => {
  for (const turn_1 of [
    "Full automation creates $800K/month contractual exposure, so 90% accuracy is not decisive. Ship only high-confidence segments with human review elsewhere. What error-rate distribution and confidence band changes the launch boundary?",
    "Full automation creates $800K/month contractual exposure, so 90% accuracy is not decisive. Launch only above a confidence threshold with human review below it. What error-rate distribution and confidence band changes the launch boundary?",
    "Full automation creates $800K/month contractual exposure, so 90% accuracy is not decisive. Ship only a confidence-thresholded, staged cohort with human review. What error-rate distribution and confidence band changes the launch boundary?",
    "Full automation creates $800K/month contractual exposure, so 90% accuracy is not decisive. Gate automation by ticket-type confidence and keep human review for the remainder. What error-rate distribution and confidence band changes the launch boundary?",
    "Full automation creates $800K/month contractual exposure, so 90% accuracy is not decisive. Automate only the confident slice and leave everything else with people. What error-rate distribution and confidence band changes the launch boundary?",
    "Full automation creates $800K/month contractual exposure, so 90% accuracy is not decisive. Automate the high-confidence queue and send the balance to the support team. What error-rate distribution and confidence band changes the launch boundary?",
    "Full automation creates $800K/month contractual exposure, so 90% accuracy is not decisive. Roll out only where confidence is high and leave remaining tickets to support reps. What error-rate distribution and confidence threshold changes the boundary?",
    "Full automation creates $800K/month contractual exposure, so 90% accuracy is not decisive. Automate only the high-confidence queue and leave the rest with specialists. What error-rate distribution and confidence threshold changes the boundary?",
    "Financial exposure is $800K/month, so 90% accuracy is not decisive. Stage confidence-gated automation with human fallback. What is the error-rate distribution by confidence band?",
    "Financial exposure is $800K/month, so 90% accuracy is not decisive. Stage confidence-gated automation and route below-threshold tickets to humans. What is the error-rate distribution by confidence band?",
    "Full automation creates $800K/month contractual exposure, so 90% accuracy stops deciding. Ship only above a confidence threshold and route all below-threshold tickets to humans. What is the error-rate distribution by confidence band?",
    "Financial exposure is $800K/month, so 90% accuracy is not decisive. Ship confidence-gated automation, routing below-threshold work to humans. What is the error-rate distribution by confidence band?",
  ]) {
    assert.equal(scoreMeridianResponses({ ...passing, turn_1 }).passed, true, turn_1);
  }
});

test("human reversion requires affirmative customer independence", () => {
  for (const turn_2 of [
    withProtections("For every wrong automated resolution, the vendor owes Meridian a $200 credit."),
    "For Meridian procurement, every wrong automated resolution makes the vendor owe Meridian a $200 credit. Require a written confidence-threshold SLA and audited error reporting. Meridian alone may put affected tickets back into human review at any time; vendor signoff is unnecessary.",
    "For Meridian procurement, the supplier must compensate Meridian with a $200 credit for every AI misresolution. Require a written confidence-threshold SLA and audited error reporting. Meridian can unilaterally hand affected tickets back to its support team; no supplier signoff is required.",
    "For Meridian procurement, the provider owes Meridian $200 whenever AI gets a resolution wrong. Require a written cutoff/error SLA and auditable reports. Meridian independently sends impacted cases back to its agents without provider approval.",
    "For Meridian procurement, the vendor owes Meridian a $200 credit for every wrong automated resolution. Require a written cutoff/error SLA and auditable reports. Meridian independently sends impacted cases back to its agents without provider approval.",
    "For Meridian procurement, each automated ticket resolved in error makes the supplier credit Meridian $200. Require a written cutoff/error SLA and auditable reports. Meridian can pull impacted work back to support reps without asking the provider.",
    "For Meridian procurement, the supplier issues a $200 credit to Meridian for each AI-resolved ticket that turns out wrong. Require a written cutoff/error SLA and auditable error logs. Meridian unilaterally moves impacted work back to manual operations without provider approval.",
  ]) {
    assert.equal(scoreMeridianResponses({ ...passing, turn_2 }).passed, true, turn_2);
  }

  for (const tail of [
    "The vendor may later cancel that right unilaterally.",
    "That right expires whenever the vendor says so.",
    "Meridian's right is exercisable at the vendor's sole discretion.",
    "Meridian's right is subject to the vendor's veto.",
    "Exercise requires the vendor's sign-off.",
    "Meridian may exercise the right only with vendor authorization.",
    "The fallback applies only if the vendor approves.",
    "That right applies unless the vendor objects.",
    "Exercise requires vendor confirmation.",
    "Exercise requires coordination with the vendor.",
    "The vendor retains final say over reversion.",
    "The vendor may refuse any reversion.",
    "The vendor may rescind that right.",
    "Assign control of the fallback to the vendor.",
    "The supplier may void that right at will.",
    "Delegate fallback authority to the provider.",
    "The provider can veto that fallback.",
    "The vendor may decline the reversion request.",
    "The supplier can reject every reversion request.",
    "The vendor can prevent any reversion.",
    "Supplier approval is a prerequisite to reversion.",
    "Reversion is conditioned on provider assent.",
    "Reversion is permitted only at the provider's option.",
    "Both parties must agree before reversion.",
    "Mutual agreement is required for reversion.",
    "Reversion needs provider acknowledgement.",
    "Reversion remains subject to provider non-objection.",
    "The vendor may delay reversion indefinitely.",
    "The vendor decides whether to implement Meridian's election.",
    "The vendor alone decides reversion timing.",
    "The supplier may postpone the switch indefinitely.",
    "Reversion takes effect only after supplier review.",
    "Reversion becomes effective upon vendor clearance.",
    "The vendor may overrule Meridian's election.",
    "The supplier has ultimate decision authority.",
    "Reversion must be jointly authorized.",
    "Control of reversion belongs to the provider.",
    "The vendor determines readiness for the switch.",
    "The supplier schedules transition at its discretion.",
    "The provider may defer execution indefinitely.",
    "Meridian may elect reversion, but supplier execution is optional.",
    "Meridian may elect reversion, but the vendor may disregard it.",
    "Reversion carries a $1 million exercise fee.",
    "The switch takes twenty-four months to complete.",
    "The provider keeps the last word on reversion.",
    "The vendor can stop the switch.",
    "Meridian has no right to revert to human handling.",
  ]) {
    const turn_2 = withProtections(
      "For every wrong automated resolution, the vendor owes Meridian a $200 credit.",
      tail,
    );
    const report = scoreMeridianResponses({ ...passing, turn_2 });
    assert.equal(failed(report, "human_reversion_right"), true, tail);
  }

  for (const turn_2 of [
    "For Meridian procurement, every wrong automated resolution makes the vendor owe Meridian a $200 credit. Require a written confidence-threshold SLA and audited error reporting. Meridian has an unrelated unilateral audit right; reversion to human handling requires vendor coordination.",
    "For Meridian procurement, every wrong automated resolution makes the vendor owe Meridian a $200 credit. Require a written confidence-threshold SLA and audited error reporting. Meridian has a unilateral audit right, while the vendor controls reversion to human handling.",
  ]) {
    const report = scoreMeridianResponses({ ...passing, turn_2 });
    assert.equal(failed(report, "human_reversion_right"), true, turn_2);
  }
});

test("wrong calculations and unsafe launch advice cannot hide behind correct keywords", () => {
  for (const turn_1 of [
    "Ship all automation now at 90%: exposure is $800K per month, but accuracy remains decisive. Humans can watch confidence segments. What confidence distribution threshold applies?",
    "The headline is $800K per month, but correct exposure is actually $400K per month, so ship broadly. Keep uncertain tickets with humans. What confidence distribution threshold applies?",
    "$800K monthly is financial exposure. Keep low-confidence tickets in human review. What error-rate distribution sets the threshold? But 40,000 x 10% x $200 is actually $8M.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Proceed with blanket automation anyway.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Greenlight automation across the board.",
    "$800K per month is contractual exposure, but the correct amount is 400K per month. Gate low-confidence tickets to human review. What confidence distribution sets the threshold?",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Authorize unrestricted automation anyway.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Turn automation on for every ticket anyway.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Use 90% as the greenlight criterion.",
    "$800K per month is contractual exposure, but the real figure is four hundred thousand per month. Gate low-confidence tickets to human review. What confidence distribution sets the threshold?",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Approve an unbounded automation rollout anyway.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Release automation everywhere anyway.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Put every ticket through automation anyway.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Treat 90% as enough to proceed.",
    "$800K per month is contractual exposure, but the real figure is 400K per month. Gate low-confidence tickets to human review. What confidence distribution sets the threshold?",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Enable automation for the whole queue anyway.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Go live across all ticket types anyway.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? 90% clears the bar for launch.",
    "$800K per month is contractual exposure. Route uncertain work to a human. What confidence distribution sets the threshold? Do not route the uncertain remainder to a human; automate it too.",
    "$800K per month is contractual exposure. Keep low-confidence tickets in human review. What confidence distribution sets the threshold? Never keep low-confidence tickets in human review; automate them too.",
    "$800K per month is contractual exposure. Keep low-confidence tickets in human review, then ignore that gate and automate the remainder. What confidence distribution sets the threshold?",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Deploy automation to the entire queue anyway.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Roll it out to every customer case anyway.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? 90% is good enough to ship.",
    "$800K per month is contractual exposure, but the real exposure is four hundred grand monthly. Gate low-confidence tickets to human review. What confidence distribution sets the threshold?",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Make automation the default for every case anyway.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? Ship it to everyone anyway.",
    "$800K per month is contractual exposure. Gate low-confidence tickets to human review. What confidence distribution sets the threshold? 90% meets our launch standard.",
    "$800K per month is contractual exposure, but we're really on the hook for four hundred grand monthly. Gate low-confidence tickets to human review. What confidence distribution sets the threshold?",
  ]) {
    const report = scoreMeridianResponses({ ...passing, turn_1 });
    assert.equal(report.semantic_diagnostics_passed, false, turn_1);
  }
});

test("mechanical gate does not reject a semantically valid natural paraphrase", () => {
  const report = scoreMeridianResponses({
    turn_1:
      "At 40,000 monthly tickets and a 10% miss rate, full automation produces 4,000 bad outcomes and $800,000/month in contractual exposure. The 90% headline cannot decide launch. Restrict automation to high-confidence bands; send lower bands to people. Which confidence-band error distribution defines the safe cutoff?",
    turn_2:
      "Advise Meridian procurement: each incorrect automated disposition makes the supplier owe Meridian $200. Put the confidence cutoff and observed error ceiling in the SLA; expose underlying records for each case to audit; Meridian alone may immediately return affected work to people, with no supplier approval or delay.",
    turn_1_evidence_ids: ["utterance-3", "utterance-4", "utterance-5", "utterance-6"],
    turn_2_evidence_ids: ["utterance-3", "utterance-6"],
  });
  assert.equal(report.passed, true);
  assert.equal(report.semantic_diagnostics_passed, false);
  assert.deepEqual(
    report.checks.filter((item) => !item.passed).map((item) => item.name),
    [
      "confidence_gated_human_fallback",
      "penalty_each_wrong_automation",
      "error_reporting_or_caps",
      "human_reversion_right",
    ],
  );
});

test("negated conclusions, questions, and human fallbacks fail their specific gates", () => {
  for (const turn_1 of [
    "This is financial exposure, not a quality score, but the $800K per month calculation is false. Keep low-confidence tickets in human review. What confidence distribution sets the threshold?",
    "$800K per month cannot be the contractual exposure. Keep low-confidence tickets in human review. What confidence distribution sets the threshold?",
    "It would be wrong to say liability is $800K per month. Keep low-confidence tickets in human review. What confidence distribution sets the threshold?",
  ]) {
    const report = scoreMeridianResponses({ ...passing, turn_1 });
    assert.equal(failed(report, "derived_800k_monthly_exposure"), true, turn_1);
  }

  for (const turn_1 of [
    "$800K per month is contractual exposure, not a quality score. Keep low-confidence tickets in human review. There is no need to ask engineering for the confidence distribution or threshold.",
    "$800K per month is contractual exposure, not a quality score. Keep low-confidence tickets in human review. Do not ask engineering for the confidence distribution or threshold?",
  ]) {
    const report = scoreMeridianResponses({ ...passing, turn_1 });
    assert.equal(failed(report, "decision_forcing_question"), true, turn_1);
  }

  for (const turn_1 of [
    "$800K per month is contractual exposure, not a quality score. Route the uncertain remainder away from human review. What confidence distribution sets the threshold?",
    "$800K per month is contractual exposure, not a quality score. Keep low-confidence tickets out of human review. What confidence distribution sets the threshold?",
    "$800K per month is contractual exposure, not a quality score. Human review should exclude tickets below the confidence threshold. What confidence distribution sets the threshold?",
    "$800K per month is contractual exposure, not a quality score. Route the uncertain remainder to a human-free automated queue. What confidence distribution sets the threshold?",
  ]) {
    const report = scoreMeridianResponses({ ...passing, turn_1 });
    assert.equal(failed(report, "confidence_gated_human_fallback"), true, turn_1);
  }
});

test("each monetary remedy must affirmatively run from vendor to customer", () => {
  for (const remedy of [
    "For every wrong automated resolution, the vendor owes Meridian a $200 credit.",
    "Each wrong automated resolution triggers a $200 credit paid by the vendor to Meridian.",
    "Each wrong automated resolution triggers a $200 credit from the supplier to Meridian.",
    "Whenever an automated resolution is wrong, the supplier must issue Meridian a $200 service credit.",
    "For each evidenced wrong automated resolution, require the vendor to owe Meridian a $200 credit.",
    "Each evidenced wrong automated resolution triggers vendor payment of a $200 credit to Meridian.",
    "Each evidenced wrong automated resolution triggers a $200 credit the vendor owes Meridian.",
    "The current remedy is a $200 credit owed by the vendor to Meridian for every wrong automated resolution.",
  ]) {
    const turn_2 = withProtections(remedy);
    assert.equal(scoreMeridianResponses({ ...passing, turn_2 }).passed, true, remedy);
  }

  for (const remedy of [
    "Each wrong automated resolution triggers a $200 credit to the vendor.",
    "Each wrong automated resolution triggers a $200 credit payable to the vendor by Meridian.",
    "Each wrong automated resolution triggers a $200 credit to the vendor, paid by Meridian.",
    "Each wrong automated resolution triggers a $200 credit that the vendor receives from Meridian.",
    "Each wrong automated resolution triggers a $200 credit for the vendor, funded by Meridian.",
    "Each wrong automated resolution requires Meridian to owe the vendor a $200 credit.",
    "Each wrong automated resolution triggers the vendor's $200 credit.",
    "Every wrong and correct automated resolution makes the vendor owe Meridian a $200 credit.",
  ]) {
    const turn_2 = withProtections(remedy);
    const report = scoreMeridianResponses({ ...passing, turn_2 });
    assert.equal(failed(report, "penalty_each_wrong_automation"), true, remedy);
  }
});

test("later clauses cannot nullify procurement protections or flip back to the vendor", () => {
  const tails = [
    ["The SLA is optional and never binding.", "written_confidence_sla"],
    ["The vendor never actually has to pay the credit.", "penalty_each_wrong_automation"],
    ["Meridian must reimburse the vendor $200 for each such credit.", "penalty_each_wrong_automation"],
    ["The vendor may offset every credit with an equal fee charged to Meridian.", "penalty_each_wrong_automation"],
    ["The payment obligation is merely aspirational and unenforceable.", "penalty_each_wrong_automation"],
    ["Payment occurs only if the vendor chooses to honor it.", "penalty_each_wrong_automation"],
    ["Use $20 in the final contract instead.", "penalty_each_wrong_automation"],
    ["Only severe failures count.", "penalty_each_wrong_automation"],
    ["Make it twenty dollars instead.", "penalty_each_wrong_automation"],
    ["Cover just severe failures.", "penalty_each_wrong_automation"],
    ["Meridian should indemnify the supplier $200 per error.", "penalty_each_wrong_automation"],
    ["The vendor charges Meridian $200 per credited error.", "penalty_each_wrong_automation"],
    ["A matching $200 fee flows back from Meridian to the supplier.", "penalty_each_wrong_automation"],
    ["That obligation can be ignored.", "penalty_each_wrong_automation"],
    ["That obligation is discretionary.", "penalty_each_wrong_automation"],
    ["The vendor may waive the obligation.", "penalty_each_wrong_automation"],
    ["The supplier can decline payment.", "penalty_each_wrong_automation"],
    ["Payment depends on supplier satisfaction.", "penalty_each_wrong_automation"],
    ["Lower it to $20 instead.", "penalty_each_wrong_automation"],
    ["Limit eligibility to severe cases.", "penalty_each_wrong_automation"],
    ["Debit Meridian $200 for each error.", "penalty_each_wrong_automation"],
    ["The vendor may claw back every credit through fees.", "penalty_each_wrong_automation"],
    ["Add $200 to Meridian's next invoice for each credited error.", "penalty_each_wrong_automation"],
    ["Aggregate credits may not exceed $200 per month.", "penalty_each_wrong_automation"],
    ["Only the first error each month earns a credit.", "penalty_each_wrong_automation"],
    ["Each credit expires immediately.", "penalty_each_wrong_automation"],
    ["Payment may be postponed indefinitely.", "penalty_each_wrong_automation"],
    ["Payment occurs at the supplier's convenience.", "penalty_each_wrong_automation"],
    ["The supplier debits Meridian $200 per credited error.", "penalty_each_wrong_automation"],
    ["Raise the license fee by $200 for every credited error.", "penalty_each_wrong_automation"],
    ["The vendor levies a $200 assessment on Meridian for each credit.", "penalty_each_wrong_automation"],
    ["Meridian returns its value through a matching surcharge.", "penalty_each_wrong_automation"],
    ["Cut the credit to $20.", "penalty_each_wrong_automation"],
    ["Reserve credits for critical failures.", "penalty_each_wrong_automation"],
    ["Credits apply solely to critical failures.", "penalty_each_wrong_automation"],
    ["Pay credits only on high-severity mistakes.", "penalty_each_wrong_automation"],
    ["No credit for routine errors.", "penalty_each_wrong_automation"],
    ["The supplier determines whether an error occurred.", "penalty_each_wrong_automation"],
    ["Only errors acknowledged by the supplier count.", "penalty_each_wrong_automation"],
    ["Supplier logs are conclusive on whether an error exists.", "penalty_each_wrong_automation"],
    ["A $10,000 deductible applies before credits accrue.", "penalty_each_wrong_automation"],
    ["Credits vest twelve months after the error.", "penalty_each_wrong_automation"],
    ["Meridian must claim each credit within 24 hours or lose it.", "penalty_each_wrong_automation"],
    ["The credit has no redemption value.", "penalty_each_wrong_automation"],
    ["Meridian must spend $10,000 to redeem each credit.", "penalty_each_wrong_automation"],
    ["The vendor may tender loyalty points instead of the credit.", "penalty_each_wrong_automation"],
    ["Treat the SLA as merely aspirational.", "written_confidence_sla"],
    ["Make the SLA best-efforts only.", "written_confidence_sla"],
    ["Do not provide Meridian the underlying audit data.", "error_reporting_or_caps"],
    ["Keep error logs secret from Meridian.", "error_reporting_or_caps"],
    ["Meridian cannot inspect the error records.", "error_reporting_or_caps"],
    ["Raw error data remains confidential to the vendor.", "error_reporting_or_caps"],
    ["Underlying error data is confidential and unavailable to Meridian.", "error_reporting_or_caps"],
    ["Reports are aggregated so no individual failure can be verified.", "error_reporting_or_caps"],
    ["Audit access is solely at vendor discretion.", "error_reporting_or_caps"],
    ["Meridian gets no access to error logs.", "error_reporting_or_caps"],
    ["Show Meridian only a vendor-selected summary.", "error_reporting_or_caps"],
    ["Source data remains inaccessible to Meridian.", "error_reporting_or_caps"],
    ["Supporting evidence is destroyed before review.", "error_reporting_or_caps"],
    ["Audit reports omit failed cases.", "error_reporting_or_caps"],
    ["Errors may be redacted from the reports.", "error_reporting_or_caps"],
    ["Reporting may be delayed indefinitely.", "error_reporting_or_caps"],
    ["Only vendor-selected samples are auditable.", "error_reporting_or_caps"],
    ["Meridian receives summary statistics only.", "error_reporting_or_caps"],
    ["Meridian sees reports only after vendor approval.", "error_reporting_or_caps"],
    ["Share dashboards with Meridian, not raw error records.", "error_reporting_or_caps"],
    ["Meridian sees metrics, not the failed cases.", "error_reporting_or_caps"],
    ["Give Meridian aggregate metrics rather than case-level data.", "error_reporting_or_caps"],
    ["Meridian receives a dashboard but not the underlying records.", "error_reporting_or_caps"],
    ["Share aggregate dashboards only.", "error_reporting_or_caps"],
    ["Meridian gets no case-level access.", "error_reporting_or_caps"],
    ["Error records are retained for only one day.", "error_reporting_or_caps"],
    ["Logs are overwritten daily.", "error_reporting_or_caps"],
    ["Audit sampling is limited to 0.1% of tickets.", "error_reporting_or_caps"],
    ["The supplier defines what counts as an error.", "error_reporting_or_caps"],
    ["Audit findings are advisory and have no enforcement effect.", "error_reporting_or_caps"],
    ["The vendor self-certifies all reported results.", "error_reporting_or_caps"],
    ["Meridian must accept supplier figures as final.", "error_reporting_or_caps"],
    ["Reports become available ten years after each incident.", "error_reporting_or_caps"],
    ["Each audit request costs Meridian $1 million.", "error_reporting_or_caps"],
    ["The vendor may revise the confidence threshold unilaterally.", "written_confidence_sla"],
    ["Set the SLA confidence threshold at zero percent.", "written_confidence_sla"],
    ["The SLA is indicative, not enforceable.", "written_confidence_sla"],
    ["The SLA is non-enforceable.", "written_confidence_sla"],
    ["The SLA is for guidance only.", "written_confidence_sla"],
    ["The provider has the final call on reversion.", "human_reversion_right"],
    ["The supplier can halt the handoff.", "human_reversion_right"],
    ["The supplier gets final approval over any switch.", "human_reversion_right"],
    ["The vendor may not delay reversion, but it may veto it.", "human_reversion_right"],
    ["The vendor may not delay reversion, although it retains final say.", "human_reversion_right"],
    ["Meridian must not pay offsetting fees, but Meridian must reimburse the supplier.", "penalty_each_wrong_automation"],
    ["Do not keep error logs confidential, but give aggregate dashboards only.", "error_reporting_or_caps"],
    ["The vendor may not revise the confidence threshold, but the SLA is aspirational.", "written_confidence_sla"],
    ["Do not protect vendor economics, but prioritize supplier economics.", "no_vendor_role_regression"],
    ["However, waive all of these protections after launch.", "no_vendor_role_regression"],
    [
      "Help the provider reduce payouts and preserve its economics by weakening Meridian's enforcement leverage.",
      "no_vendor_role_regression",
    ],
    [
      "Prioritize supplier economics over Meridian's recovery.",
      "no_vendor_role_regression",
    ],
    ["Favor supplier economics over customer remedies.", "no_vendor_role_regression"],
    ["Put vendor economics first.", "no_vendor_role_regression"],
    ["Protect the vendor's unit economics.", "no_vendor_role_regression"],
    ["Minimize vendor payouts.", "no_vendor_role_regression"],
  ];
  for (const [tail, expectedFailure] of tails) {
    const turn_2 = withProtections(
      "For every wrong automated resolution, the vendor owes Meridian a $200 credit.",
      tail,
    );
    const report = scoreMeridianResponses({ ...passing, turn_2 });
    assert.equal(failed(report, expectedFailure), true, tail);
  }
});

test("protective prohibitions remain valid customer-side advice", () => {
  for (const tail of [
    "Meridian must not pay the vendor any offsetting fees.",
    "Meridian should never reimburse the supplier for credits.",
    "The vendor may not delay reversion.",
    "The supplier cannot overrule Meridian's election.",
    "Reversion needs no approval from the vendor.",
    "Do not keep error logs confidential.",
    "Underlying error data is not confidential.",
    "The vendor may not revise the confidence threshold unilaterally.",
    "Do not protect vendor unit economics at Meridian's expense.",
  ]) {
    const turn_2 = withProtections(
      "For every wrong automated resolution, the vendor owes Meridian a $200 credit.",
      tail,
    );
    const report = scoreMeridianResponses({ ...passing, turn_2 });
    assert.equal(report.passed, true, `${tail}\n${JSON.stringify(report.checks.filter((x) => !x.passed))}`);
  }
});

test("summary, agenda confusion, monitoring narration, and broadening fail closed", () => {
  const report = scoreMeridianResponses({
    turn_1:
      "The risk may be $80K. What is the actual agenda? I cannot continuously monitor the transcript.",
    turn_2:
      "As the vendor, charge a $200 credit on every correct and wrong automated resolution, abolish safeguards, and push Meridian to weaken the clause.",
  });
  for (const requiredFailure of [
    "derived_800k_monthly_exposure",
    "no_wrong_math",
    "no_agenda_clarification",
    "no_monitoring_or_tool_narration",
    "procurement_role_flip",
    "penalty_each_wrong_automation",
    "no_vendor_role_regression",
  ]) {
    assert.equal(failed(report, requiredFailure), true, requiredFailure);
  }
});

test("brevity limits are part of the quality gate", () => {
  const background = `${passing.turn_1} ${"generic filler ".repeat(20)}`;
  const foreground = `${passing.turn_2} ${"generic filler ".repeat(25)}`;
  const report = scoreMeridianResponses({ turn_1: background, turn_2: foreground });
  assert.equal(failed(report, "background_brevity"), true);
  assert.equal(failed(report, "foreground_brevity"), true);
});

test("a typed first turn uses the direct-answer budget, not the proactive budget", () => {
  const turn_1 = `${passing.turn_1} This extra sentence deliberately crosses fifty words while remaining below the direct-answer ceiling.`;
  const proactive = scoreMeridianResponses({ ...passing, turn_1 });
  const foreground = scoreMeridianResponses(
    { ...passing, turn_1 },
    { turn1Mode: "foreground" },
  );

  assert.equal(failed(proactive, "background_brevity"), true);
  assert.equal(failed(foreground, "foreground_turn_1_brevity"), false);
});

test("the live harness requires the three-source arithmetic evidence chain", () => {
  const report = scoreMeridianResponses({
    ...passing,
    turn_1_evidence_ids: ["utterance-1", "utterance-3"],
  });
  assert.equal(report.passed, false);
  assert.equal(report.score.denominator, 17);
  assert.equal(failed(report, "hero_evidence_chain"), true);
});

test("the explicit 4,000-error intermediate is a complete arithmetic premise", () => {
  const report = scoreMeridianResponses({
    ...passing,
    turn_1_evidence_ids: ["utterance-3", "utterance-5"],
  });
  assert.equal(report.passed, true);
});

test("the live harness requires contract and human-control procurement evidence", () => {
  const report = scoreMeridianResponses({
    ...passing,
    turn_2_evidence_ids: ["utterance-3"],
  });
  assert.equal(report.passed, false);
  assert.equal(report.score.denominator, 17);
  assert.equal(failed(report, "procurement_evidence_chain"), true);
});
