#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "../..");
export const defaultFixturePath = path.join(
  repoRoot,
  "tests/fixtures/sidekick_rehearsal/v1/meridian_ship_decision.json",
);

function check(name, passed, detail) {
  return { name, passed: Boolean(passed), detail };
}

function normalized(text) {
  return String(text ?? "")
    .replace(/[’‘]/g, "'")
    .replace(/[–—]/g, "-")
    .toLowerCase();
}

export function scoreMeridianResponses(responses) {
  const turn1 = normalized(responses.turn_1);
  const turn2 = normalized(responses.turn_2);

  const turn1Checks = [
    check(
      "derived_800k_monthly_exposure",
      /\$?\s*800(?:,?000|k)\b/.test(turn1) &&
        /(\/\s*(?:mo|month)|per month|monthly|a month)/.test(turn1),
      "Must state approximately $800K per month, not only 4,000 errors.",
    ),
    check(
      "liability_reframe",
      /(liabil|contractual exposure|contractual risk|financial exposure|financial risk|penalty credits?|cost decision|uncapped[- ]loss)/.test(
        turn1,
      ),
      "Must reframe 90% accuracy as contractual or financial exposure.",
    ),
    check(
      "confidence_gated_human_fallback",
      /confidence/.test(turn1) && /human/.test(turn1),
      "Must confidence-gate automation and route uncertain work to a human.",
    ),
    check(
      "decision_forcing_question",
      /\?/.test(turn1) &&
        /confidence/.test(turn1) &&
        /(distribution|threshold|score|calibrat|percentile)/.test(turn1),
      "Must ask engineering for the confidence distribution or threshold.",
    ),
    check(
      "no_wrong_math",
      !/(?:90%|ninety percent|current exposure|this exposure).{0,80}\$\s*(?:8\s*k|8,?000|80\s*k|80,?000|8\s*m|8,?000,?000)\b/.test(
        turn1,
      ),
      "$8K, $80K, and $8M are automatic failures.",
    ),
    check(
      "no_agenda_clarification",
      !/(what(?:'s| is) the agenda|clarif(?:y|ication).{0,30}(agenda|topic)|actual agenda)/.test(
        turn1,
      ),
      "A confirmed meeting topic must never trigger another agenda question.",
    ),
    check(
      "no_monitoring_or_tool_narration",
      !/(cannot|can't|unable to).{0,35}(monitor|poll|wake)|continuous monitoring|on demand|transcript command|screen context command|tool call/.test(
        turn1,
      ),
      "The visible answer must not narrate host limitations or routine reads.",
    ),
    check(
      "no_false_visual_claim",
      !/(i can see|i see on (?:the|your) screen|the (?:screen|slide) shows|visible on your screen)/.test(
        turn1,
      ),
      "The synthetic screen carries no material evidence.",
    ),
  ];

  const procurementProtections = {
    penalty_all_automation:
      /(penalt|credit).{0,60}(?:all|every|each).{0,45}automat|(?:all|every|each).{0,45}automat.{0,60}(penalt|credit)|preserve the \$200 credit[\s\S]{0,400}credits apply automatically/.test(
        turn2,
      ),
    written_confidence_sla:
      /(written.{0,35}confidence|confidence.{0,35}(written|sla)|sla.{0,35}confidence)/.test(
        turn2,
      ) ||
      (/demand these terms/.test(turn2) &&
        /confidence (?:band|threshold)/.test(turn2) &&
        /(predefined acceptance gates|quality thresholds|correctness gate)/.test(turn2)),
    error_reporting_or_caps: /(error.{0,30}(report|cap)|report.{0,30}error|audit)/.test(turn2),
    human_reversion_right:
      /(right.{0,35}(revert|human)|revert.{0,35}human|human-in-the-loop fallback|fallback.{0,25}human)/.test(
        turn2,
      ),
  };

  const turn2Checks = [
    check(
      "procurement_role_flip",
      /(for meridian|meridian(?:'s)? procurement|as (?:meridian|the customer)|procurement lead|customer-side|meridian (?:traffic|contract))/.test(
        turn2,
      ) ||
        (/meridian.{0,60}(?:approves?|can pause|right|authorize)/.test(turn2) &&
          /(not the vendor|vendor consent|vendor-caused)/.test(turn2)),
      "Turn two must persistently advise Meridian procurement, not the vendor.",
    ),
    ...Object.entries(procurementProtections).map(([name, passed]) =>
      check(name, passed, `Missing procurement protection: ${name.replaceAll("_", " ")}.`),
    ),
    check(
      "no_vendor_role_regression",
      !/(as the vendor|protect your margin|limit your liability to meridian|push meridian)/.test(turn2),
      "The answer must not slip back into vendor-side advice.",
    ),
  ];

  const checks = [...turn1Checks, ...turn2Checks];
  return {
    schema_version: 1,
    fixture_id: "synthetic-meridian-ship-decision",
    passed: checks.every((item) => item.passed),
    score: {
      numerator: checks.filter((item) => item.passed).length,
      denominator: checks.length,
    },
    checks,
  };
}

async function main(argv) {
  let fixturePath = defaultFixturePath;
  let responsesPath = null;
  for (let index = 2; index < argv.length; index += 1) {
    if (argv[index] === "--fixture") fixturePath = argv[++index];
    else if (argv[index] === "--responses") responsesPath = argv[++index];
    else throw new Error(`unknown argument: ${argv[index]}`);
  }
  if (!responsesPath) throw new Error("--responses PATH is required");

  const fixture = JSON.parse(await fs.readFile(fixturePath, "utf8"));
  if (fixture.schema_version !== 1 || fixture.content_origin !== "synthetic") {
    throw new Error("rehearsal fixture must be schema v1 and explicitly synthetic");
  }
  const responses = JSON.parse(await fs.readFile(responsesPath, "utf8"));
  const report = scoreMeridianResponses(responses);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  return report.passed ? 0 : 1;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = await main(process.argv);
}
