#!/usr/bin/env node
/**
 * Assert that CI's `rust` path filter covers every Cargo workspace member.
 *
 * The filter decides whether the Rust jobs run at all. When it drifts, a PR
 * touching Rust skips every Rust job and reports green -- which is how a change
 * that statically linked the Windows CRT sailed through (#657 follow-up).
 *
 * The obvious fix, a blanket 'crates/**', overshoots: crates/mcp and crates/sdk
 * are npm packages with no Cargo.toml, so every TypeScript-only PR paid for
 * test x3, install x2, a full Linux build and the Windows desktop installer.
 * Negation entries cannot trim it back either -- the action compiles each
 * pattern on its own, and a lone '!crates/mcp/**' matches every file outside
 * that subtree, turning the filter always-on.
 *
 * So the filter enumerates, and this script is the thing that stops the
 * enumeration going stale: add a workspace member without listing it and CI
 * fails here rather than silently skipping the Rust jobs forever.
 */
import { readFileSync } from "fs";
import { dirname, join } from "path";
import { fileURLToPath } from "url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

/** Workspace member paths from the root Cargo.toml. */
function workspaceMembers() {
  const raw = readFileSync(join(repoRoot, "Cargo.toml"), "utf8");
  const block = raw.match(/^\s*members\s*=\s*\[([\s\S]*?)\]/m);
  if (!block) throw new Error("Cargo.toml has no [workspace] members array");
  return [...block[1].matchAll(/["']([^"']+)["']/g)].map((m) => m[1]);
}

/** Patterns listed under the `rust:` filter in ci.yml. */
function rustFilterPatterns() {
  const raw = readFileSync(join(repoRoot, ".github/workflows/ci.yml"), "utf8");
  const lines = raw.split(/\r?\n/);
  const start = lines.findIndex((l) => /^\s*rust:\s*$/.test(l));
  if (start === -1) throw new Error("ci.yml has no `rust:` filter");

  const indent = lines[start].match(/^\s*/)[0].length;
  const patterns = [];
  for (const line of lines.slice(start + 1)) {
    if (line.trim() === "" || line.trim().startsWith("#")) continue;
    // Dedent to or past the `rust:` key ends the block.
    if (line.match(/^\s*/)[0].length <= indent) break;
    const entry = line.match(/^\s*-\s*['"]?([^'"]+)['"]?\s*$/);
    if (entry) patterns.push(entry[1].trim());
  }
  if (patterns.length === 0) throw new Error("`rust:` filter is empty");
  return patterns;
}

/**
 * Does any pattern cover this member's files?
 *
 * Only prefix globs count. A pattern like 'crates/core/**' covers
 * 'crates/core' and anything below it.
 */
function coveredBy(member, patterns) {
  return patterns.some((pattern) => {
    if (pattern.startsWith("!")) return false;
    if (!pattern.endsWith("/**")) return pattern === `${member}/Cargo.toml`;
    const prefix = pattern.slice(0, -3);
    return member === prefix || member.startsWith(`${prefix}/`);
  });
}

const members = workspaceMembers();
const patterns = rustFilterPatterns();
const missing = members.filter((m) => !coveredBy(m, patterns));

if (missing.length > 0) {
  console.error("CI's `rust` path filter does not cover every workspace member.");
  console.error("");
  for (const m of missing) console.error(`  uncovered: ${m}`);
  console.error("");
  console.error("A PR touching those crates would skip every Rust job and report green.");
  console.error("Add the member to the `rust:` filter in .github/workflows/ci.yml:");
  for (const m of missing) console.error(`  - '${m}/**'`);
  process.exit(1);
}

console.log(
  `CI rust filter covers all ${members.length} workspace members ` +
    `(${patterns.length} patterns).`
);
