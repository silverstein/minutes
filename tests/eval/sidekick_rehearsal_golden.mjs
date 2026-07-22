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

  const penaltyTerm =
    /\b(?:penalt(?:y|ies)|penali[sz]\w*|credits?|charg(?:e|ed|es|ing|eable)|liab(?:le|ility)|remed(?:y|ies))\b/;
  const automationFragment = String.raw`(?:automat\w*|ai|agent|model|machine|ai[- ]handled|ai[- ]resolved|agent[- ]handled|agent[- ]resolved|model[- ]handled|model[- ]resolved)`;
  const wrongFragment = String.raw`(?:wrong(?:ly)?|incorrect(?:ly)?|erroneous(?:ly)?|errors?|failed|failures?|mistakes?|mistaken|bad\s+(?:resolutions?|outcomes?|outputs?|responses?|answers?|tickets?))`;
  const relationBridge = String.raw`(?:(?!\b(?:manual|human|and|but|while|whereas|some|selected)\b)[^,;.!?]){0,60}`;
  const directAutomatedError = String.raw`(?:\b${automationFragment}\b${relationBridge}\b${wrongFragment}\b|\b${wrongFragment}\b${relationBridge}\b${automationFragment}\b)`;
  const conditionalAutomatedError = String.raw`\b${automationFragment}\b[^;.!?]{0,55},\s*(?:if|when)\b[^;.!?]{0,45}\b${wrongFragment}\b`;
  const automatedError = String.raw`(?:${directAutomatedError}|${conditionalAutomatedError})`;
  const quantifiedAutomatedError = String.raw`\b(?:every|each|all|any|per|whenever)\b${relationBridge}${automatedError}`;
  const exceptionScopedAutomatedError = String.raw`${automatedError}(?:\s+without\s+(?:any\s+)?(?:exceptions?|exemptions?)|\s+across\s+the\s+board|\s*,?\s*(?:with\s+)?(?:no|zero)\s+(?:automation\s+)?(?:exceptions?|exemptions?|carve[- ]?outs?)(?!\s+(?:for|in|to)\b))`;
  const scopedAutomatedError = String.raw`(?:${quantifiedAutomatedError}|${exceptionScopedAutomatedError})`;
  const remedyFragment = String.raw`(?:the\s+)?(?:(?:existing|contractual|current)\s+)*(?:\$\s*200\s+|two\s+hundred\s+dollar\s+)?(?:penalt(?:y|ies)|credits?|remed(?:y|ies)|liability)`;
  const preserveVerb = String.raw`(?:keep|retain|preserve|maintain|enforce|apply|demand|require|push\s+for)`;
  const scopedRemedyPatterns = [
    new RegExp(
      String.raw`\b${preserveVerb}\b[^;.!?]{0,25}${remedyFragment}\s+(?:(?:on|for|to|against|when|if)\s+${scopedAutomatedError}|whenever\s+${automatedError})`,
    ),
    new RegExp(
      String.raw`${remedyFragment}\s+(?:(?:must|should|shall)\s+)?(?:still\s+)?(?:appl(?:y|ies)|covers?|attaches?)\s+(?:to|for|on)\s+${scopedAutomatedError}`,
    ),
    new RegExp(
      String.raw`${remedyFragment}\s+(?:(?:must|should|shall)\s+)?(?:remains?|stays?)\s+(?:in\s+force|intact)\s+(?:for|on)\s+${scopedAutomatedError}`,
    ),
    new RegExp(
      String.raw`${scopedAutomatedError}[^;.!?]{0,30}\b(?:subject\s+to|covered\s+by|under)\s+${remedyFragment}`,
    ),
    new RegExp(
      String.raw`${scopedAutomatedError}(?:(?!\b(?:not|never)\b|n't)[^;.!?]){0,25}\b(?:triggers?|incurs?|receives?|gets?|carr(?:y|ies))\s+${remedyFragment}`,
    ),
    new RegExp(
      String.raw`${scopedAutomatedError}\s*,\s*${preserveVerb}\b[^;.!?]{0,25}${remedyFragment}`,
    ),
    new RegExp(
      String.raw`\b(?:penali[sz]e|charge)\s+(?:\$\s*200\s+(?:for|on)\s+)?${scopedAutomatedError}`,
    ),
    new RegExp(
      String.raw`\b(?:hold\s+)?(?:the\s+)?(?:vendor|provider|company)?\s*(?:liable|responsible)\s+for\s+${scopedAutomatedError}`,
    ),
    new RegExp(
      String.raw`\b(?:do\s+not|don't|never)\s+(?:carve|exclude|exempt)\s+${automatedError}\s+(?:out\s+of|from)\s+${remedyFragment}`,
    ),
    new RegExp(
      String.raw`\bno\b[^;.!?]{0,20}${automatedError}[^;.!?]{0,25}\b(?:is|are|be)\s+exempt\s+from\s+${remedyFragment}`,
    ),
    new RegExp(
      String.raw`${automatedError}[^;.!?]{0,25}\b(?:is|are|be)\s+(?:not|never)\s+(?:exempt|excluded)\s+from\s+${remedyFragment}`,
    ),
    new RegExp(
      String.raw`\bno\b[^;.!?]{0,20}${automatedError}[^;.!?]{0,30}\b(?:(?:should|must|can)\s+)?(?:escape|avoid)\s+${remedyFragment}`,
    ),
    new RegExp(
      String.raw`\b(?:do\s+not|don't|never)\s+(?:waive|remove|drop|eliminate|suspend|cancel|cap)\w*\s+${remedyFragment}\s+(?:on|for|to)\s+${scopedAutomatedError}`,
    ),
    new RegExp(
      String.raw`\b(?:(?:may|must|shall|can)\s+not|cannot|can't)\s+(?:waive|remove|drop|eliminate|suspend|cancel|cap)\w*\s+${remedyFragment}\s+(?:on|for|to)\s+${scopedAutomatedError}`,
    ),
    new RegExp(
      String.raw`\b(?:do\s+not|don't|never)\s+allow\s+${remedyFragment}\s+to\s+be\s+(?:waived|removed|dropped|eliminated|suspended|cancelled|canceled|capped)\s+(?:on|for|to)\s+${scopedAutomatedError}`,
    ),
    new RegExp(
      String.raw`\b${preserveVerb}\b[^;.!?]{0,25}${remedyFragment}\s+uncapped\s+(?:on|for|to)\s+${scopedAutomatedError}`,
    ),
    new RegExp(
      String.raw`${remedyFragment}\s+(?:remains?|stays?)\s+uncapped\s+(?:on|for|to)\s+${scopedAutomatedError}`,
    ),
  ];
  const overbroadAllAutomation =
    /\b(?:every|each|all)\s+(?:single\s+)?(?:automat\w*|ai[- ]handled|agent[- ]handled|model[- ]handled)\s+(?:resolutions?|outcomes?|outputs?|responses?|answers?|tickets?)\b|\b(?:whether|regardless of whether)\s+(?:it is |they are )?(?:correct|successful|accurate)\s+(?:or|and)\s+(?:wrong|incorrect)|\b(?:regardless|irrespective)\s+of\s+(?:correctness|accuracy|outcome)\b/;
  const manualOnlyRemedy =
    /\b(?:only|solely)\s+(?:for\s+)?(?:wrong\s+)?manual\b|\bmanual\b[^.;!?\n]{0,45}\b(?:only|solely)\b/;
  const negatesScopedRemedy = new RegExp(
    String.raw`\b(?:do\s+not|don't|never)\s+(?:keep|retain|preserve|maintain|enforce|apply|demand|require|push\s+for)\b[^;.!?\n]{0,100}\b(?:penalt(?:y|ies)|credits?|remed(?:y|ies)|liability)\b|\b(?:penalt(?:y|ies)|credits?|remed(?:y|ies)|liability)\b[^;.!?\n]{0,45}\b(?:must|should|shall|does|do|is|are)?\s*(?:not|never)\s+(?:apply|attach|cover|remain|stay)\w*\b|${automatedError}[^;.!?\n]{0,35}\b(?:(?:must|should|shall|does|do|is|are)\s+)?(?:not|never)\s+(?:trigger|incur|receive|get|carry|be\s+subject)\w*\b|${automatedError}[^;.!?\n]{0,35}\b(?:shouldn't|doesn't|mustn't|isn't|aren't|won't|can't)\s+(?:trigger|incur|receive|get|carry|be\s+subject)\w*\b`,
  );
  const explicitScopedCarveout = new RegExp(
    String.raw`(?=[^;.!?\n]{0,180}${remedyFragment})(?=[^;.!?\n]{0,180}${automationFragment})(?=[^;.!?\n]{0,180}${wrongFragment})[^;.!?\n]{0,180}\b(?:except(?:\s+for)?|unless|apart\s+from|other\s+than|with\s+the\s+exception\s+of|save(?:\s+for)?|excluding|barring)\b`,
  );
  const conditionalScopedCarveout = new RegExp(
    String.raw`(?=[^;.!?\n]{0,180}${remedyFragment})(?=[^;.!?\n]{0,180}${automationFragment})(?=[^;.!?\n]{0,180}${wrongFragment})[^;.!?\n]{0,180}\b(?:only\s+(?:if|when|where|above|below)|provid(?:ed|ing)\s+that|subject\s+to\s+(?!${remedyFragment}\b))`,
  );

  function hasUnprotectedRemedyRemoval(text) {
    const destructive =
      /\b(?:remove|eliminate|waive|drop|suspend|abolish|cancel|cap)\w*\b/g;
    for (const match of text.matchAll(destructive)) {
      const start = match.index;
      const end = start + match[0].length;
      const before = text.slice(Math.max(0, start - 55), start);
      const after = text.slice(end, Math.min(text.length, end + 55));
      const directlyScopesRemedy =
        /\b(?:penalt(?:y|ies)|credits?|remed(?:y|ies))\b[^,;.!?\n]{0,30}$/.test(
          before,
        ) ||
        /^[^,;.!?\n]{0,30}\b(?:penalt(?:y|ies)|credits?|remed(?:y|ies))\b/.test(
          after,
        );
      if (!directlyScopesRemedy) continue;

      const prefix = text.slice(Math.max(0, start - 110), start);
      const protective =
        /\b(?:do\s+not|don't|never|may\s+not|must\s+not|shall\s+not|can\s+not|cannot|can't)\s+$/.test(
          prefix,
        ) ||
        /\b(?:may|must|shall|can)\s+not\s+be\s+$/.test(prefix) ||
        /\b(?:no|without\s+(?:a\s+)?)\s+(?:penalt(?:y|ies)|credits?|remed(?:y|ies))\s+$/.test(
          prefix,
        ) ||
        /\b(?:do\s+not|don't|never)\s+allow\b[^;.!?\n]{0,80}\b(?:penalt(?:y|ies)|credits?|remed(?:y|ies))\b[^;.!?\n]{0,20}\bto\s+be\s+$/.test(
          prefix,
        );
      if (!protective) return true;
    }
    return false;
  }
  function hasPositiveAutomationExemption(text) {
    const automation = new RegExp(automationFragment, "g");
    const status = /\b(?:exempt|excluded|outside|not\s+subject)\b/g;
    const automationSpans = [...text.matchAll(automation)].map((match) => ({
      start: match.index,
      end: match.index + match[0].length,
    }));
    for (const match of text.matchAll(status)) {
      const statusStart = match.index;
      const statusEnd = statusStart + match[0].length;
      const nearestAutomation = automationSpans
        .filter(
          (span) =>
            Math.min(Math.abs(span.end - statusStart), Math.abs(span.start - statusEnd)) <= 80,
        )
        .sort(
          (left, right) =>
            Math.min(Math.abs(left.end - statusStart), Math.abs(left.start - statusEnd)) -
            Math.min(Math.abs(right.end - statusStart), Math.abs(right.start - statusEnd)),
        )[0];
      if (!nearestAutomation) continue;
      if (match[0].startsWith("not subject")) return true;
      const statusPrefix = text.slice(Math.max(0, statusStart - 18), statusStart);
      const automationPrefix = text.slice(
        Math.max(0, nearestAutomation.start - 18),
        nearestAutomation.start,
      );
      const protective =
        /\b(?:not|never)\s+(?:be\s+)?$/.test(statusPrefix) ||
        /\b(?:no|none)\s+$/.test(automationPrefix) ||
        /\b(?:do\s+not|don't|never)\s+$/.test(statusPrefix);
      if (!protective) return true;
    }
    return false;
  }
  const broadensPenaltyToCorrectOutcome = turn2
    .split(/[.;!?\n]+/)
    .some((clause) => {
      const namesCorrectOutcome =
        /\b(?:correct|successful|accurate|valid)\b|\bright\s+(?:resolutions?|outcomes?|outputs?|responses?|answers?)\b/.test(
          clause,
        );
      if (!namesCorrectOutcome || !penaltyTerm.test(clause)) return false;

      const explicitlyExcludesCorrectOutcome =
        /\b(?:do not|don't|never|exclude|excluding|exempt)\b[^,]{0,45}\b(?:correct|successful|accurate|valid|right\s+(?:resolutions?|outcomes?|outputs?|responses?|answers?))\b|\b(?:correct|successful|accurate|valid|right\s+(?:resolutions?|outcomes?|outputs?|responses?|answers?))\b[^,]{0,45}\b(?:must|should|do|does|are|is|be)?\s*(?:not|never|excluded|exempt|outside)\b|\bonly\b[^,]{0,30}\b(?:wrong(?:ly)?|incorrect(?:ly)?|erroneous(?:ly)?|errors?|failed|failures?)\b[^,]{0,45}\b(?:penalt(?:y|ies)|credits?|charg\w*)\b/.test(
          clause,
        );
      return !explicitlyExcludesCorrectOutcome;
    });
  const hasLocalScopedAutomationPenalty = scopedRemedyPatterns.some((pattern) =>
    pattern.test(turn2),
  );
  const scopedAutomationPenalty =
    hasLocalScopedAutomationPenalty &&
    !overbroadAllAutomation.test(turn2) &&
    !broadensPenaltyToCorrectOutcome &&
    !hasUnprotectedRemedyRemoval(turn2) &&
    !manualOnlyRemedy.test(turn2) &&
    !negatesScopedRemedy.test(turn2) &&
    !explicitScopedCarveout.test(turn2) &&
    !conditionalScopedCarveout.test(turn2) &&
    !hasPositiveAutomationExemption(turn2);

  const turn1Checks = [
    check(
      "derived_800k_monthly_exposure",
      /\$?\s*800(?:,?000|k)\b/.test(turn1) &&
        /(\/\s*(?:mo|month)|per month|monthly|a month)/.test(turn1),
      "Must state approximately $800K per month, not only 4,000 errors.",
    ),
    check(
      "liability_reframe",
      /(liabil|contractual exposure|contractual risk|financial exposure|financial risk|penalty credits?|cost decision|uncapped[- ](?:loss|downside))/.test(turn1) ||
        (/(?:monthly|per month|a month).{0,35}(?:credits?|penalt|cost)|(?:credits?|penalt|cost).{0,35}(?:monthly|per month|a month)/.test(turn1) &&
          /(not|isn't|is not).{0,45}(?:quality|metric|accuracy|ship-ready)/.test(turn1)),
      "Must reframe 90% accuracy as contractual or financial exposure.",
    ),
    check(
      "confidence_gated_human_fallback",
      /human/.test(turn1) &&
        /(confidence|segment(?:ed|s)?|ticket type|error (?:rate|ceiling)|gated? rollout)/.test(turn1),
      "Must confidence-gate automation and route uncertain work to a human.",
    ),
    check(
      "decision_forcing_question",
      (/\?/.test(turn1) || /\b(?:ask|request|demand)\b/.test(turn1)) &&
        (/(confidence|error rate)/.test(turn1) &&
          /(distribution|threshold|score|calibrat|percentile|by ticket type|by segment)/.test(turn1)),
      "Must ask engineering for the confidence distribution or threshold.",
    ),
    check(
      "no_wrong_math",
      !/\$\s*(?:8\s*k|8,?000|80\s*k|80,?000|8\s*m|8,?000,?000)\b/.test(
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
    penalty_each_wrong_automation: scopedAutomationPenalty,
    written_confidence_sla:
      /(written.{0,80}(confidence|acceptance|sla)|(confidence|acceptance).{0,80}(written|sla)|sla.{0,80}(confidence|acceptance|threshold|criteria))/.test(
        turn2,
      ) ||
      (/demand these terms/.test(turn2) &&
        /confidence (?:band|threshold)/.test(turn2) &&
        /(predefined acceptance gates|quality thresholds|correctness gate)/.test(turn2)),
    error_reporting_or_caps: /(error.{0,30}(report|cap)|report.{0,30}error|audit)/.test(turn2),
    human_reversion_right:
      /(right.{0,35}(revert|human|rollback)|(?:revert|rollback).{0,35}(?:right|human)|human-in-the-loop fallback|fallback.{0,25}human)/.test(
        turn2,
      ),
  };

  const turn2Checks = [
    check(
      "procurement_role_flip",
      /(for meridian|meridian(?:'s)? procurement|as (?:meridian|the customer)|procurement lead|customer-side|meridian (?:traffic|contract))/.test(
        turn2,
      ) ||
        (/\bpush for\b/.test(turn2) &&
          /meridian(?:'s)?/.test(turn2) &&
          /(right|credit|customer harm|acceptance threshold)/.test(turn2)) ||
        (/meridian.{0,60}(?:approves?|can pause|right|authorize)/.test(turn2) &&
          /(not the vendor|vendor consent|vendor-caused)/.test(turn2)),
      "Turn two must persistently advise Meridian procurement, not the vendor.",
    ),
    ...Object.entries(procurementProtections).map(([name, passed]) =>
      check(name, passed, `Missing procurement protection: ${name.replaceAll("_", " ")}.`),
    ),
    check(
      "no_vendor_role_regression",
      !/(as the vendor|protect your margin|limit your liability to meridian|push meridian|abolish safeguards|waive (?:the )?(?:protections|credits|sla)|reject (?:the )?(?:protections|safeguards))/.test(
        turn2,
      ),
      "The answer must not slip back into vendor-side advice.",
    ),
  ];

  const provenanceChecks = Array.isArray(responses.turn_1_evidence_ids)
    ? [
        check(
          "hero_evidence_chain",
          ["utterance-1", "utterance-3", "utterance-4"].every((id) =>
            responses.turn_1_evidence_ids.includes(id),
          ),
          "The $800K synthesis must cite accuracy, penalty, and monthly-volume evidence.",
        ),
      ]
    : [];
  const checks = [...turn1Checks, ...turn2Checks, ...provenanceChecks];
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
