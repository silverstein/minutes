#!/usr/bin/env node

import { readFile, writeFile, readdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");
const manifestPath = path.join(repoRoot, "manifest.json");
const siteReleasePath = path.join(repoRoot, "site", "lib", "release.ts");
const cliMainPath = path.join(repoRoot, "crates", "cli", "src", "main.rs");
const mcpSourcePath = path.join(repoRoot, "crates", "mcp", "src", "index.ts");
const cratesDir = path.join(repoRoot, "crates");
const checkOnly = process.argv.includes("--check");

const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
const version = manifest.version;

if (!version || typeof version !== "string") {
  throw new Error(`manifest.json is missing a valid string version: ${version}`);
}

if (!Array.isArray(manifest.tools)) {
  throw new Error(`manifest.json is missing a tools array`);
}
if (!Array.isArray(manifest.prompts)) {
  throw new Error(`manifest.json is missing a prompts array`);
}

const mcpToolCount = manifest.tools.length;
const mcpPromptCount = manifest.prompts.length;

async function countMcpResources() {
  const source = await readFile(mcpSourcePath, "utf8");
  const identities = new Set();
  const patterns = [
    /registerAppResource\(\s*server,\s*"([^"]+)",\s*([A-Z0-9_":/.{}-]+),\s*\{\s*description:\s*"[^"]+"\s*,?\s*\}/gs,
    /server\.resource\(\s*"([^"]+)",\s*("[^"]+"|[A-Z][A-Z0-9_]*),\s*\{\s*description:\s*"[^"]+"\s*,?\s*\}/gs,
    /server\.resource\(\s*"([^"]+)",\s*new ResourceTemplate\(("[^"]+"|[A-Z][A-Z0-9_]*),[\s\S]*?\),\s*\{\s*description:\s*"[^"]+"\s*,?\s*\}/gs,
  ];
  for (const pattern of patterns) {
    for (const match of source.matchAll(pattern)) {
      identities.add(`${match[1]}\0${match[2]}`);
    }
  }
  if (identities.size === 0) {
    throw new Error(`Found no MCP resources in ${mcpSourcePath}`);
  }
  return identities.size;
}

async function countCliCommands() {
  const src = await readFile(cliMainPath, "utf8");
  const enumMatch = src.match(/enum Commands\s*\{([\s\S]*?)\n\}/);
  if (!enumMatch) {
    throw new Error(`Could not find 'enum Commands' block in ${cliMainPath}`);
  }
  const body = enumMatch[1];
  const lines = body.split("\n");
  const variants = new Set();
  let hiddenPending = false;
  const variantStartRe = /^ {4}([A-Z][A-Za-z0-9]*)\s*(?:\{|\(|,|$)/;
  const hideAttrRe = /#\[command\([^)]*hide\s*=\s*true[^)]*\)\]/;
  for (const line of lines) {
    if (hideAttrRe.test(line)) {
      hiddenPending = true;
      continue;
    }
    const m = variantStartRe.exec(line);
    if (!m) continue;
    if (!hiddenPending) variants.add(m[1]);
    hiddenPending = false;
  }
  if (variants.size === 0) {
    throw new Error(`Found no user-visible CLI subcommand variants in ${cliMainPath}`);
  }
  return variants.size;
}

async function walkRust(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    if (entry.name === "target" || entry.name === "node_modules") continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await walkRust(full)));
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      files.push(full);
    }
  }
  return files;
}

async function walkTs(dir, acc = []) {
  const entries = await readdir(dir, { withFileTypes: true });
  for (const entry of entries) {
    if (
      entry.name === "node_modules" ||
      entry.name === "dist" ||
      entry.name === "build" ||
      entry.name === ".next" ||
      entry.name === "target"
    ) {
      continue;
    }
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      await walkTs(full, acc);
    } else if (
      entry.isFile() &&
      (entry.name.endsWith(".test.ts") ||
        entry.name.endsWith(".test.tsx") ||
        entry.name.endsWith(".test.mjs"))
    ) {
      acc.push(full);
    }
  }
  return acc;
}

async function countRustTests() {
  const files = await walkRust(cratesDir);
  let count = 0;
  for (const f of files) {
    const src = await readFile(f, "utf8");
    const matches = src.match(/^\s*#\[(?:tokio::)?test\](?!\w)/gm);
    if (matches) count += matches.length;
  }
  return count;
}

async function countTsTests() {
  const files = await walkTs(cratesDir);
  let count = 0;
  for (const f of files) {
    const src = await readFile(f, "utf8");
    const matches = src.match(
      /^\s*(?:test|it)(?:\.(?:runIf|skipIf|only|skip|todo))?\s*\(/gm,
    );
    if (matches) count += matches.length;
  }
  return count;
}

const cliCommandCount = await countCliCommands();
const mcpResourceCount = await countMcpResources();
const rustTestCount = await countRustTests();
const tsTestCount = await countTsTests();
const totalTestCount = rustTestCount + tsTestCount;

const nextContent = `// Generated from manifest.json + source scan by
// scripts/sync_site_release_version.mjs.
// Do not edit by hand. Run \`node scripts/sync_site_release_version.mjs\`.

export const MINUTES_RELEASE_VERSION = "${version}";
export const MINUTES_RELEASE_TAG = \`v\${MINUTES_RELEASE_VERSION}\`;

export const MINUTES_MCP_TOOL_COUNT = ${mcpToolCount};
export const MINUTES_MCP_RESOURCE_COUNT = ${mcpResourceCount};
export const MINUTES_MCP_PROMPT_COUNT = ${mcpPromptCount};
export const MINUTES_CLI_COMMAND_COUNT = ${cliCommandCount};
export const MINUTES_TEST_COUNT = ${totalTestCount};

export const APPLE_SILICON_DMG =
  \`https://github.com/silverstein/minutes/releases/download/\${MINUTES_RELEASE_TAG}/Minutes_\${MINUTES_RELEASE_VERSION}_aarch64.dmg\`;

export const WINDOWS_SETUP_EXE =
  \`https://github.com/silverstein/minutes/releases/download/\${MINUTES_RELEASE_TAG}/minutes-desktop-windows-x64-setup.exe\`;
`;

let currentContent = "";
try {
  currentContent = await readFile(siteReleasePath, "utf8");
} catch (error) {
  if (checkOnly) {
    throw new Error(`Missing ${siteReleasePath}. Run node scripts/sync_site_release_version.mjs`);
  }
}

if (currentContent === nextContent) {
  console.log(
    `site release constants already match: v${version}, ${mcpToolCount} tools, ${mcpResourceCount} resources, ${mcpPromptCount} prompts, ${cliCommandCount} CLI commands, ${totalTestCount} tests`,
  );
  process.exit(0);
}

if (checkOnly) {
  // The test count is derived by scanning for #[test]/#[tokio::test], so it
  // moves whenever anyone adds a test. Comparing it for exact equality made a
  // shared, required check go red on main every time tests landed, and every
  // open PR then inherited a failure it did not cause. That happened twice in
  // two days (#664 and again the next morning).
  //
  // Release *links* are what this job is named for and what must be exact:
  // version, and the tool/resource/prompt/command counts that appear in
  // documented URLs. A slightly stale test count on a marketing page is not
  // worth reddening everyone's CI, and the pre-push hook still refreshes it.
  const staleOnlyByTestCount =
    currentContent.replace(/MINUTES_TEST_COUNT = \d+/, "MINUTES_TEST_COUNT = 0") ===
    nextContent.replace(/MINUTES_TEST_COUNT = \d+/, "MINUTES_TEST_COUNT = 0");
  if (staleOnlyByTestCount) {
    console.warn(
      `site release constants match except the test count (committed differs from ${totalTestCount}). ` +
        "Not failing: run scripts/sync_site_release_version.mjs to refresh it.",
    );
    process.exit(0);
  }
  console.error(
    [
      "site release constants are out of sync with source",
      `manifest version: ${version}`,
      `mcp tool count: ${mcpToolCount}`,
      `mcp resource count: ${mcpResourceCount}`,
      `mcp prompt count: ${mcpPromptCount}`,
      `cli command count: ${cliCommandCount}`,
      `test count: ${totalTestCount} (rust: ${rustTestCount}, ts: ${tsTestCount})`,
      `target file: ${path.relative(repoRoot, siteReleasePath)}`,
      "run: node scripts/sync_site_release_version.mjs",
    ].join("\n"),
  );
  process.exit(1);
}

await writeFile(siteReleasePath, nextContent, "utf8");
console.log(
  `updated ${path.relative(repoRoot, siteReleasePath)}: v${version}, ${mcpToolCount} tools, ${mcpResourceCount} resources, ${mcpPromptCount} prompts, ${cliCommandCount} CLI commands, ${totalTestCount} tests`,
);
