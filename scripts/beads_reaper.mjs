#!/usr/bin/env node
// Local-only beads hygiene report. Beads state lives in local Dolt (the
// committed JSONL is its export), so this NEVER runs in hosted CI and never
// writes anything — it reports, a human (or agent, with the maintainer's
// blessing) closes.
//
// Reports three drift classes that accumulated historically:
//   1. in_progress issues idle > --idle-days (default 30), by updated_at
//   2. open/in_progress issues whose title matches a merged PR title on main
//   3. exact-duplicate titles among open/in_progress issues
//
// Usage: node scripts/beads_reaper.mjs [--idle-days N] [--json]
// Reads .beads/issues.jsonl at the repo root (the export bd keeps current).

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const asJson = args.includes("--json");
const idleFlag = args.indexOf("--idle-days");
const idleDays = idleFlag !== -1 ? Number(args[idleFlag + 1]) : 30;
if (!Number.isFinite(idleDays) || idleDays <= 0) {
  console.error("--idle-days must be a positive number");
  process.exit(2);
}

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const jsonlPath = path.join(repoRoot, ".beads", "issues.jsonl");

const issues = readFileSync(jsonlPath, "utf8")
  .split("\n")
  .filter(Boolean)
  .flatMap((line) => {
    try {
      return [JSON.parse(line)];
    } catch {
      return [];
    }
  });

const active = issues.filter((issue) => ["open", "in_progress"].includes(issue.status));
const now = Date.now();
const dayMs = 24 * 60 * 60 * 1000;

const idle = active
  .filter((issue) => issue.status === "in_progress" && issue.updated_at)
  .map((issue) => ({
    id: issue.id,
    title: issue.title,
    idleDays: Math.floor((now - Date.parse(issue.updated_at)) / dayMs),
  }))
  .filter((issue) => issue.idleDays > idleDays)
  .sort((a, b) => b.idleDays - a.idleDays);

// Merged-PR title match: squash merges land as "<PR title> (#N)" on main.
let mergedSubjects = [];
try {
  mergedSubjects = execFileSync(
    "git",
    ["log", "--pretty=%s", "--grep", "(#", "-5000", "origin/main"],
    { cwd: repoRoot, encoding: "utf8" },
  )
    .split("\n")
    .filter(Boolean)
    .map((subject) => subject.replace(/\s*\(#\d+\)\s*$/, "").toLowerCase());
} catch {
  // git unavailable — skip class 2 rather than fail the report.
}
const mergedSet = new Set(mergedSubjects);
const shipped = active
  .filter((issue) => issue.title && mergedSet.has(issue.title.toLowerCase()))
  .map((issue) => ({ id: issue.id, title: issue.title }));

const byTitle = new Map();
for (const issue of active) {
  const key = (issue.title || "").trim().toLowerCase();
  if (!key) continue;
  if (!byTitle.has(key)) byTitle.set(key, []);
  byTitle.get(key).push(issue.id);
}
const duplicates = [...byTitle.entries()]
  .filter(([, ids]) => ids.length > 1)
  .map(([title, ids]) => ({ title, ids }));

const report = { idleDays, idle, shippedButOpen: shipped, duplicates };

if (asJson) {
  console.log(JSON.stringify(report, null, 2));
} else {
  console.log(`beads reaper report (${active.length} active issues)`);
  console.log(`\nin_progress idle > ${idleDays}d: ${idle.length}`);
  for (const issue of idle) console.log(`  ${issue.id}  ${issue.idleDays}d  ${issue.title}`);
  console.log(`\nopen but title matches a merged PR: ${shipped.length}`);
  for (const issue of shipped) console.log(`  ${issue.id}  ${issue.title}`);
  console.log(`\nexact-duplicate titles: ${duplicates.length}`);
  for (const dup of duplicates) console.log(`  ${dup.ids.join(", ")}  ${dup.title}`);
  console.log("\nreport-only: close/merge via bd after human review.");
}
