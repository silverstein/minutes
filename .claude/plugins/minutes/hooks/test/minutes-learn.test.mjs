#!/usr/bin/env node

import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const tempHome = mkdtempSync(join(tmpdir(), "minutes-learn-test-"));
process.env.HOME = tempHome;

const modulePath = new URL("../lib/minutes-learn.mjs", import.meta.url);
const {
  clearLearning,
  getAliasCluster,
  getLatestLearning,
  rememberAlias,
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

  console.log("minutes-learn tests passed");
} finally {
  rmSync(tempHome, { recursive: true, force: true });
}
