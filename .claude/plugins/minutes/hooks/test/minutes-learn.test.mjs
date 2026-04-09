#!/usr/bin/env node

import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const tempHome = mkdtempSync(join(tmpdir(), "minutes-learn-test-"));
process.env.HOME = tempHome;

const modulePath = new URL("../lib/minutes-learn.mjs", import.meta.url);
const {
  finalizePendingMeetingPrepNudge,
  clearLearning,
  getAliasCluster,
  getLatestLearning,
  inferMeetingPrepModeFromUsage,
  rememberAlias,
  rememberObserved,
  recordPendingMeetingPrepNudge,
  shouldSuppressMeetingPrepNudge,
} = await import(modulePath.href);

try {
  rememberAlias("Sarah Chen", "Sarah", "test alias");
  let aliases = getAliasCluster("Sarah Chen");
  assert.equal(aliases.length, 2, "alias cluster should include both names");
  assert.ok(
    aliases.some((entry) => entry.normalized === "sarah"),
    "cluster should include short name",
  );

  clearLearning("alias", "Sarah Chen");
  aliases = getAliasCluster("Sarah Chen");
  assert.equal(aliases.length, 1, "cleared alias should no longer keep the old edge");
  assert.equal(aliases[0].normalized, "sarah chen");

  const latest = getLatestLearning("alias", "Sarah Chen");
  assert.equal(latest?.value, null, "latest alias learning should reflect tombstone");

  const agentBase = join(tempHome);
  const prepsDir = join(agentBase, ".minutes", "preps");
  const briefsDir = join(agentBase, ".minutes", "briefs");
  mkdirSync(prepsDir, { recursive: true });
  mkdirSync(briefsDir, { recursive: true });
  // Strong prep preference from usage
  for (let i = 0; i < 3; i += 1) {
    const file = join(prepsDir, `2026-04-0${i + 1}-alex.prep.md`);
    writeFileSync(file, "prep");
    const recent = new Date(Date.now() - i * 60 * 1000);
    utimesSync(file, recent, recent);
  }
  assert.equal(inferMeetingPrepModeFromUsage(agentBase), "prep");

  // Old historical preference should decay out of the mode inference window.
  const oldBrief = join(briefsDir, "2025-01-01-legacy.brief.md");
  writeFileSync(oldBrief, "legacy brief");
  const oldTs = new Date(Date.now() - 40 * 24 * 60 * 60 * 1000);
  utimesSync(oldBrief, oldTs, oldTs);
  assert.equal(inferMeetingPrepModeFromUsage(agentBase), "prep");

  // Pending nudge finalized as engaged when a prep is created afterward.
  recordPendingMeetingPrepNudge("prep", agentBase);
  writeFileSync(join(prepsDir, "2026-04-09-jordan.prep.md"), "prep later");
  const finalized = finalizePendingMeetingPrepNudge(agentBase);
  assert.equal(finalized?.outcome, "engaged");
  assert.equal(finalized?.mode, "prep");

  // Three ignored outcomes suppress future nudges.
  const tsBase = Date.now() - 2 * 24 * 60 * 60 * 1000;
  for (let i = 0; i < 3; i += 1) {
    rememberObserved(
      "nudge_feedback",
      "meeting_prep_nudge_outcome",
      { mode: "auto", outcome: "ignored", shown_at: new Date(tsBase + i * 1000).toISOString() },
      0.7,
      "ignored for test",
    );
  }
  assert.equal(shouldSuppressMeetingPrepNudge(), true);

  // Old ignored nudges should decay out and stop suppressing.
  const oldIgnoredBase = Date.now() - 10 * 24 * 60 * 60 * 1000;
  for (let i = 0; i < 3; i += 1) {
    rememberObserved(
      "nudge_feedback",
      "meeting_prep_nudge_outcome",
      { mode: "auto", outcome: "ignored", shown_at: new Date(oldIgnoredBase + i * 1000).toISOString() },
      0.7,
      "old ignored for test",
    );
  }
  assert.equal(shouldSuppressMeetingPrepNudge(), true, "recent ignored nudges still dominate");

  console.log("minutes-learn tests passed");
} finally {
  rmSync(tempHome, { recursive: true, force: true });
}
