#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

function parseArgs(argv) {
  let json = false;
  let root;

  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--json") {
      json = true;
    } else if (argument === "--root") {
      if (index + 1 >= argv.length) {
        throw new Error("--root requires a directory argument");
      }
      root = argv[index + 1];
      index += 1;
    } else {
      throw new Error(`unknown argument: ${argument}`);
    }
  }

  const scriptRoot = path.resolve(path.dirname(process.argv[1]), "..");
  return {
    json,
    root: root === undefined ? scriptRoot : path.resolve(root),
  };
}

function readText(root, file) {
  return fs.readFileSync(path.join(root, file), "utf8");
}

function readJson(root, file) {
  return JSON.parse(readText(root, file));
}

function requireVersion(value, description) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${description} is missing or is not a string`);
  }
  return value;
}

function tomlSectionValue(text, section, key) {
  let currentSection = null;
  const keyPattern = new RegExp(`^${key.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*=\\s*\"([^\"]+)\"`);

  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    const sectionMatch = /^\[([^\]]+)\]$/.exec(line);
    if (sectionMatch) {
      currentSection = sectionMatch[1];
      continue;
    }
    if (currentSection === section) {
      const valueMatch = keyPattern.exec(line);
      if (valueMatch) return valueMatch[1];
    }
  }

  throw new Error(`[${section}].${key} declaration not found`);
}

function cliMinutesCoreVersion(text) {
  const dependency = /^minutes-core\s*=\s*\{([\s\S]*?)\}/m.exec(text);
  if (!dependency) {
    throw new Error("minutes-core dependency declaration not found");
  }
  const version = /\bversion\s*=\s*"([^"]+)"/.exec(dependency[1]);
  if (!version) {
    throw new Error("minutes-core dependency version field not found");
  }
  return version[1];
}

function mcpServerVersion(text) {
  const declaration = /\bconst\s+MCP_SERVER_VERSION\s*=\s*"([^"]+)"/.exec(text);
  if (!declaration) {
    throw new Error('declaration `const MCP_SERVER_VERSION = "X.Y.Z"` not found');
  }
  return declaration[1];
}

function cargoLockVersion(text, packageName) {
  const matches = [];
  for (const block of text.split(/^\[\[package\]\]\s*$/m).slice(1)) {
    const name = /^name\s*=\s*"([^"]+)"\s*$/m.exec(block)?.[1];
    if (name !== packageName) continue;
    const version = /^version\s*=\s*"([^"]+)"\s*$/m.exec(block)?.[1];
    if (!version) {
      throw new Error(`Cargo.lock package ${packageName} has no version`);
    }
    matches.push(version);
  }

  if (matches.length === 0) {
    throw new Error(`Cargo.lock package ${packageName} not found`);
  }
  if (matches.length > 1) {
    throw new Error(`Cargo.lock contains multiple exact packages named ${packageName}`);
  }
  return matches[0];
}

function source(file, key, extract) {
  try {
    return { file, key, value: extract(), error: null };
  } catch (error) {
    return {
      file,
      key,
      value: null,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

function expectedVersion(sources) {
  const counts = new Map();
  let expected = null;
  let highestCount = 0;

  for (const item of sources) {
    if (item.error !== null) continue;
    const count = (counts.get(item.value) ?? 0) + 1;
    counts.set(item.value, count);
    if (count > highestCount) {
      expected = item.value;
      highestCount = count;
    }
  }

  return expected;
}

function evaluateDomain(sources) {
  const expected = expectedVersion(sources);
  const evaluatedSources = sources.map((item) => ({
    ...item,
    ok: item.error === null && item.value === expected,
  }));
  return {
    expected,
    sources: evaluatedSources,
    ok: expected !== null && evaluatedSources.every((item) => item.ok),
  };
}

function collectDomains(root) {
  const domain1 = evaluateDomain([
    source("Cargo.toml", "[workspace.package].version", () =>
      tomlSectionValue(readText(root, "Cargo.toml"), "workspace.package", "version"),
    ),
    source("crates/cli/Cargo.toml", "dependencies.minutes-core.version", () =>
      cliMinutesCoreVersion(readText(root, "crates/cli/Cargo.toml")),
    ),
    source("tauri/src-tauri/tauri.conf.json", ".version", () =>
      requireVersion(readJson(root, "tauri/src-tauri/tauri.conf.json").version, ".version"),
    ),
    source("crates/mcp/package.json", ".version", () =>
      requireVersion(readJson(root, "crates/mcp/package.json").version, ".version"),
    ),
    source("crates/sdk/package.json", ".version", () =>
      requireVersion(readJson(root, "crates/sdk/package.json").version, ".version"),
    ),
    source("manifest.json", ".version", () =>
      requireVersion(readJson(root, "manifest.json").version, ".version"),
    ),
    source("manifest.mcpb.json", ".version", () =>
      requireVersion(readJson(root, "manifest.mcpb.json").version, ".version"),
    ),
    source("crates/mcp/src/index.ts", "MCP_SERVER_VERSION", () =>
      mcpServerVersion(readText(root, "crates/mcp/src/index.ts")),
    ),
    source("crates/mcp/package-lock.json", ".version", () =>
      requireVersion(readJson(root, "crates/mcp/package-lock.json").version, ".version"),
    ),
    source("crates/mcp/package-lock.json", '.packages[""].version', () =>
      requireVersion(
        readJson(root, "crates/mcp/package-lock.json").packages?.[""]?.version,
        '.packages[""].version',
      ),
    ),
    source("crates/sdk/package-lock.json", ".version", () =>
      requireVersion(readJson(root, "crates/sdk/package-lock.json").version, ".version"),
    ),
    source("crates/sdk/package-lock.json", '.packages[""].version', () =>
      requireVersion(
        readJson(root, "crates/sdk/package-lock.json").packages?.[""]?.version,
        '.packages[""].version',
      ),
    ),
    ...["minutes-core", "minutes-cli", "minutes-reader"].map((packageName) =>
      source("Cargo.lock", `package[${packageName}].version`, () =>
        cargoLockVersion(readText(root, "Cargo.lock"), packageName),
      ),
    ),
  ]);

  const domain2 = evaluateDomain([
    source(".claude-plugin/marketplace.json", ".plugins[0].version", () =>
      requireVersion(
        readJson(root, ".claude-plugin/marketplace.json").plugins?.[0]?.version,
        ".plugins[0].version",
      ),
    ),
    source(".claude/plugins/minutes/plugin.json", ".version", () =>
      requireVersion(readJson(root, ".claude/plugins/minutes/plugin.json").version, ".version"),
    ),
    source(".claude/plugins/minutes/.claude-plugin/plugin.json", ".version", () =>
      requireVersion(
        readJson(root, ".claude/plugins/minutes/.claude-plugin/plugin.json").version,
        ".version",
      ),
    ),
  ]);

  return { domain1, domain2 };
}

function jsonReport(domain1, domain2) {
  const publicDomain = (domain) => ({
    expected: domain.expected,
    sources: domain.sources.map(({ file, key, value, ok }) => ({ file, key, value, ok })),
  });
  return {
    ok: domain1.ok && domain2.ok,
    domain1: publicDomain(domain1),
    domain2: publicDomain(domain2),
  };
}

function printHumanReport(report, domains) {
  if (report.ok) {
    console.log("Version sync check passed.");
    console.log(`Domain 1 (main release): ${domains.domain1.expected}`);
    console.log(`Domain 2 (plugin lockstep): ${domains.domain2.expected}`);
    return;
  }

  console.error("Version sync check failed.");
  for (const [name, label] of [
    ["domain1", "Domain 1 (main release)"],
    ["domain2", "Domain 2 (plugin lockstep)"],
  ]) {
    const domain = domains[name];
    if (domain.ok) continue;
    const expected = domain.expected ?? "<unknown>";
    console.error(`${label} failed; expected ${expected}:`);
    for (const item of domain.sources.filter((candidate) => !candidate.ok)) {
      const found = item.error === null ? item.value : `<error: ${item.error}>`;
      console.error(`  ${item.file} [${item.key}] = ${found} (expected ${expected})`);
    }
  }
}

let options;
try {
  options = parseArgs(process.argv.slice(2));
} catch (error) {
  console.error(`check_version_sync: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(2);
}

const domains = collectDomains(options.root);
const report = jsonReport(domains.domain1, domains.domain2);

if (options.json) {
  console.log(JSON.stringify(report, null, 2));
} else {
  printHumanReport(report, domains);
}

process.exitCode = report.ok ? 0 : 1;
