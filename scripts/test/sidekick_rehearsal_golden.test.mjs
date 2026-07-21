import assert from "node:assert/strict";
import test from "node:test";
import { scoreMeridianResponses } from "../../tests/eval/sidekick_rehearsal_golden.mjs";

const passing = {
  turn_1:
    "That 90% is a liability number, not merely a quality score: 40,000 x 10% x $200 is about $800K per month in contractual credits. Gate full automation to high-confidence tickets and route the uncertain remainder to a human. Ask engineering: what is the confidence distribution, and what volume clears a defensible threshold?",
  turn_2:
    "For Meridian procurement, require the penalty to apply to all automated resolutions, a written confidence-threshold SLA, auditable error-rate reporting and caps, and an explicit right to revert to human-in-the-loop if performance slips.",
};

test("Meridian reference behavior clears every golden check", () => {
  const report = scoreMeridianResponses(passing);
  assert.equal(report.passed, true);
  assert.deepEqual(report.score, { numerator: 14, denominator: 14 });
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

