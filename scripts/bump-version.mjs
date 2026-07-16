#!/usr/bin/env node

import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

const numericIdentifier = "(?:0|[1-9]\\d*)";
const prereleaseIdentifier = `(?:${numericIdentifier}|\\d*[A-Za-z-][0-9A-Za-z-]*)`;
const exactVersionPattern = new RegExp(
  `^${numericIdentifier}\\.${numericIdentifier}\\.${numericIdentifier}` +
    `(?:-${prereleaseIdentifier}(?:\\.${prereleaseIdentifier})*)?$`,
);

const pluginFiles = [
  ".claude-plugin/marketplace.json",
  ".claude/plugins/minutes/plugin.json",
  ".claude/plugins/minutes/.claude-plugin/plugin.json",
];

const domainOneFiles = [
  "Cargo.toml",
  "crates/cli/Cargo.toml",
  "tauri/src-tauri/tauri.conf.json",
  "crates/mcp/package.json",
  "crates/sdk/package.json",
  "manifest.json",
  "manifest.mcpb.json",
  "crates/mcp/src/index.ts",
  "crates/mcp/package-lock.json",
  "crates/sdk/package-lock.json",
  "Cargo.lock",
];

function usage() {
  return [
    "Usage:",
    "  node scripts/bump-version.mjs [--dry-run] <version>",
    "  node scripts/bump-version.mjs [--dry-run] --plugin <version>",
  ].join("\n");
}

function parseArgs(argv) {
  let dryRun = false;
  let plugin = false;
  let version;

  for (const argument of argv) {
    if (argument === "--dry-run") {
      if (dryRun) throw new Error("--dry-run may only be specified once");
      dryRun = true;
    } else if (argument === "--plugin") {
      if (plugin) throw new Error("--plugin may only be specified once");
      plugin = true;
    } else if (argument.startsWith("-")) {
      throw new Error(`unknown option: ${argument}`);
    } else if (version === undefined) {
      version = argument;
    } else {
      throw new Error(`unexpected argument: ${argument}`);
    }
  }

  if (version === undefined) throw new Error("a version is required");
  if (!exactVersionPattern.test(version)) {
    throw new Error(`invalid version ${JSON.stringify(version)}; expected x.y.z or x.y.z-prerelease`);
  }

  return { dryRun, plugin, version };
}

const toolPath = process.env.BUMP_TOOL_PATH;
const childEnvironment = {
  ...process.env,
  ...(toolPath
    ? { PATH: `${toolPath}${path.delimiter}${process.env.PATH ?? ""}` }
    : {}),
};
const activeChildren = new Set();

class CommandError extends Error {
  constructor(command, args, code, signal, stdout, stderr) {
    const outcome = signal ? `signal ${signal}` : `exit code ${code}`;
    const detail = stderr.trim() || stdout.trim();
    super(`${command} ${args.join(" ")} failed with ${outcome}${detail ? `\n${detail}` : ""}`);
    this.name = "CommandError";
  }
}

function exec(command, args, { cwd, input, env = childEnvironment } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd,
      env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    activeChildren.add(child);

    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.once("error", (error) => {
      activeChildren.delete(child);
      reject(error);
    });
    child.once("close", (code, signal) => {
      activeChildren.delete(child);
      if (code === 0) {
        resolve({ stdout, stderr });
      } else {
        reject(new CommandError(command, args, code, signal, stdout, stderr));
      }
    });

    child.stdin.end(input);
  });
}

async function readText(root, file) {
  return readFile(path.join(root, file), "utf8");
}

async function writeText(root, file, contents) {
  await writeFile(path.join(root, file), contents, "utf8");
}

async function readJson(root, file) {
  return JSON.parse(await readText(root, file));
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

async function updateJsonVersion(root, file, readCurrent, version) {
  const text = await readText(root, file);
  const value = JSON.parse(text);
  const current = readCurrent(value);
  if (typeof current !== "string") throw new Error(`${file} is missing its version field`);
  const pattern = new RegExp(
    `^([\\t ]*"version"[\\t ]*:[\\t ]*")${escapeRegExp(current)}("[\\t ]*,?[\\t ]*)$`,
    "gm",
  );
  const matches = [...text.matchAll(pattern)];
  if (matches.length !== 1) {
    throw new Error(`${file} must contain exactly one live JSON version declaration`);
  }
  await writeText(root, file, text.replace(pattern, `$1${version}$2`));
}

function workspaceVersion(text) {
  const section = /^\[workspace\.package\][\t ]*\r?\n([\s\S]*?)(?=^\[|$(?![\s\S]))/m.exec(text);
  const match = section && /^version[\t ]*=[\t ]*"([^"]+)"[\t ]*$/m.exec(section[1]);
  if (!match) throw new Error("[workspace.package].version declaration not found in Cargo.toml");
  return match[1];
}

function updateWorkspaceVersion(text, version) {
  const sectionPattern = /(^\[workspace\.package\][\t ]*\r?\n)([\s\S]*?)(?=^\[|$(?![\s\S]))/m;
  const section = sectionPattern.exec(text);
  if (!section) throw new Error("[workspace.package] section not found in Cargo.toml");
  const versionPattern = /^version[\t ]*=[\t ]*"[^"]+"[\t ]*$/m;
  if (!versionPattern.test(section[2])) {
    throw new Error("[workspace.package].version declaration not found in Cargo.toml");
  }
  const nextBody = section[2].replace(versionPattern, `version = "${version}"`);
  return text.slice(0, section.index) + section[1] + nextBody + text.slice(section.index + section[0].length);
}

function updateCliDependency(text, version) {
  const dependencyPattern = /^minutes-core\s*=\s*\{([\s\S]*?)\}/m;
  const dependency = dependencyPattern.exec(text);
  if (!dependency) throw new Error("minutes-core dependency declaration not found");
  const versionPattern = /\bversion\s*=\s*"[^"]+"/;
  if (!versionPattern.test(dependency[1])) {
    throw new Error("minutes-core dependency version field not found");
  }
  const replacement = dependency[0].replace(versionPattern, `version = "${version}"`);
  return text.slice(0, dependency.index) + replacement + text.slice(dependency.index + dependency[0].length);
}

function updateMcpServerVersion(text, version) {
  const declarationPattern =
    /^[\t ]*(?:export[\t ]+)?const[\t ]+MCP_SERVER_VERSION[\t ]*=[\t ]*"([^"\r\n]+)"[\t ]*;?[\t ]*$/m;
  const declaration = declarationPattern.exec(text);
  if (!declaration) {
    throw new Error('declaration `const MCP_SERVER_VERSION = "X.Y.Z"` not found');
  }
  const replacement = declaration[0].replace(`"${declaration[1]}"`, `"${version}"`);
  return text.slice(0, declaration.index) + replacement + text.slice(declaration.index + declaration[0].length);
}

function updateCargoLockVersions(text, version) {
  const packageNames = new Set(["minutes-core", "minutes-cli", "minutes-reader"]);
  const updated = new Set();
  const sections = text.split(/(?=^\[\[package\]\][\t ]*$)/m);
  const nextSections = sections.map((section) => {
    const name = /^name[\t ]*=[\t ]*"([^"]+)"[\t ]*$/m.exec(section)?.[1];
    if (!packageNames.has(name)) return section;
    if (updated.has(name)) throw new Error(`Cargo.lock contains multiple packages named ${name}`);
    if (!/^version[\t ]*=[\t ]*"[^"]+"[\t ]*$/m.test(section)) {
      throw new Error(`Cargo.lock package ${name} has no version`);
    }
    updated.add(name);
    return section.replace(/^version[\t ]*=[\t ]*"[^"]+"[\t ]*$/m, `version = "${version}"`);
  });

  for (const packageName of packageNames) {
    if (!updated.has(packageName)) throw new Error(`Cargo.lock package ${packageName} not found`);
  }
  return nextSections.join("");
}

async function fileExists(file) {
  try {
    return (await stat(file)).isFile();
  } catch (error) {
    if (error && error.code === "ENOENT") return false;
    throw error;
  }
}

async function currentVersion(root, plugin) {
  if (plugin) {
    const marketplace = await readJson(root, pluginFiles[0]);
    const version = marketplace.plugins?.[0]?.version;
    if (typeof version !== "string") throw new Error("marketplace plugin version not found");
    return version;
  }
  return workspaceVersion(await readText(root, "Cargo.toml"));
}

async function applyDomainOneWrites(root, version) {
  await writeText(
    root,
    "Cargo.toml",
    updateWorkspaceVersion(await readText(root, "Cargo.toml"), version),
  );
  await writeText(
    root,
    "crates/cli/Cargo.toml",
    updateCliDependency(await readText(root, "crates/cli/Cargo.toml"), version),
  );

  for (const file of [
    "tauri/src-tauri/tauri.conf.json",
    "crates/mcp/package.json",
    "crates/sdk/package.json",
    "manifest.json",
    "manifest.mcpb.json",
  ]) {
    await updateJsonVersion(root, file, (value) => value.version, version);
  }

  await writeText(
    root,
    "crates/mcp/src/index.ts",
    updateMcpServerVersion(await readText(root, "crates/mcp/src/index.ts"), version),
  );

  await exec("npm", ["install", "--package-lock-only"], { cwd: path.join(root, "crates/mcp") });
  await exec("npm", ["install", "--package-lock-only"], { cwd: path.join(root, "crates/sdk") });

  const cargoLockBefore = await readText(root, "Cargo.lock");
  const expectedCargoLock = updateCargoLockVersions(cargoLockBefore, version);
  await exec(
    "cargo",
    ["update", "-p", "minutes-core", "-p", "minutes-cli", "-p", "minutes-reader"],
    { cwd: root },
  );
  const cargoLockAfter = await readText(root, "Cargo.lock");
  if (cargoLockAfter !== expectedCargoLock) {
    throw new Error(
      "cargo update changed Cargo.lock beyond the version fields for minutes-core, minutes-cli, and minutes-reader",
    );
  }

  const siteSync = path.join(root, "scripts/sync_site_release_version.mjs");
  if (await fileExists(siteSync)) {
    await exec(process.execPath, [siteSync], { cwd: root });
  }
}

async function applyPluginWrites(root, version) {
  await updateJsonVersion(root, pluginFiles[0], (value) => value.plugins?.[0]?.version, version);
  for (const file of pluginFiles.slice(1)) {
    await updateJsonVersion(root, file, (value) => value.version, version);
  }
}

function parseNulList(output) {
  return output.split("\0").filter(Boolean);
}

function assertChangedFiles(actualFiles, expectedFiles, mode) {
  const actual = new Set(actualFiles);
  const expected = new Set(expectedFiles);
  const missing = [...expected].filter((file) => !actual.has(file));
  const unexpected = [...actual].filter((file) => !expected.has(file));
  if (missing.length || unexpected.length) {
    const details = [
      missing.length ? `missing: ${missing.join(", ")}` : null,
      unexpected.length ? `unexpected: ${unexpected.join(", ")}` : null,
    ].filter(Boolean);
    throw new Error(`${mode} bump produced the wrong file set (${details.join("; ")})`);
  }
}

let temporaryParent;
let temporaryWorktree;
let worktreeAdded = false;
let cleanupPromise;
let repositoryRoot;

async function cleanup() {
  if (cleanupPromise) return cleanupPromise;
  cleanupPromise = (async () => {
    let removeFailed = false;
    if (worktreeAdded && temporaryWorktree) {
      try {
        await exec("git", ["worktree", "remove", "--force", temporaryWorktree], {
          cwd: repositoryRoot,
        });
      } catch {
        removeFailed = true;
      }
    }
    if (temporaryParent) await rm(temporaryParent, { recursive: true, force: true });
    if (removeFailed) {
      // Removing the directory first makes the linked-worktree metadata immediately prunable.
      try {
        await exec("git", ["worktree", "prune", "--expire", "now"], { cwd: repositoryRoot });
      } catch {
        // Preserve the original command failure.
      }
    }
  })();
  return cleanupPromise;
}

let handlingSignal = false;
function handleSignal(signal, exitCode) {
  if (handlingSignal) return;
  handlingSignal = true;
  for (const child of activeChildren) child.kill(signal);
  void cleanup().finally(() => process.exit(exitCode));
}
process.once("SIGINT", () => handleSignal("SIGINT", 130));
process.once("SIGTERM", () => handleSignal("SIGTERM", 143));

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const { stdout: rootOutput } = await exec("git", ["rev-parse", "--show-toplevel"], {
    cwd: process.cwd(),
  });
  const root = rootOutput.trim();
  repositoryRoot = root;
  const mode = options.plugin ? "plugin" : "main";
  const existingVersion = await currentVersion(root, options.plugin);

  if (existingVersion === options.version) {
    console.log(`${mode} version is already at ${options.version}; nothing to do.`);
    return;
  }

  // Untracked files are ignored: the bump patch only ever touches tracked
  // sources, so scratch files must not block a release bump.
  const { stdout: status } = await exec(
    "git",
    ["status", "--porcelain=v1", "--untracked-files=no"],
    { cwd: root },
  );
  if (status !== "") {
    throw new Error("refusing to bump version: working tree is dirty");
  }

  temporaryParent = await mkdtemp(path.join(os.tmpdir(), "minutes-bump-version-"));
  temporaryWorktree = path.join(temporaryParent, "worktree");
  try {
    await exec("git", ["worktree", "add", "--detach", temporaryWorktree, "HEAD"], { cwd: root });
    worktreeAdded = true;

    if (options.plugin) {
      await applyPluginWrites(temporaryWorktree, options.version);
    } else {
      await applyDomainOneWrites(temporaryWorktree, options.version);
    }

    await exec(
      process.execPath,
      [path.join(temporaryWorktree, "scripts/check_version_sync.mjs"), "--root", temporaryWorktree],
      { cwd: temporaryWorktree },
    );

    const { stdout: changedOutput } = await exec(
      "git",
      ["diff", "--name-only", "-z", "--no-ext-diff"],
      { cwd: temporaryWorktree },
    );
    const changedFiles = parseNulList(changedOutput);
    const expectedFiles = options.plugin
      ? pluginFiles
      : [
          ...domainOneFiles,
          ...((await fileExists(path.join(temporaryWorktree, "scripts/sync_site_release_version.mjs")))
            ? ["site/lib/release.ts"]
            : []),
        ];
    assertChangedFiles(changedFiles, expectedFiles, mode);

    const { stdout: patch } = await exec(
      "git",
      ["diff", "--binary", "--full-index", "--no-ext-diff"],
      { cwd: temporaryWorktree },
    );
    if (!patch) throw new Error(`${mode} bump unexpectedly produced an empty patch`);

    if (options.dryRun) {
      console.log(`Dry run: ${mode} version ${existingVersion} -> ${options.version}`);
      console.log("Files that would change:");
      for (const file of changedFiles) console.log(`  ${file}`);
      console.log("\nUnified diff:");
      process.stdout.write(patch);
    } else {
      await exec("git", ["apply", "--whitespace=nowarn", "-"], { cwd: root, input: patch });
      console.log(`Updated ${mode} version ${existingVersion} -> ${options.version}:`);
      for (const file of changedFiles) console.log(`  ${file}`);
    }
  } finally {
    await cleanup();
  }
}

try {
  await main();
} catch (error) {
  await cleanup();
  console.error(`bump-version: ${error instanceof Error ? error.message : String(error)}`);
  if (
    error instanceof Error &&
    /^(?:a version is required|invalid version|unknown option|unexpected argument|--)/.test(
      error.message,
    )
  ) {
    console.error(usage());
    process.exitCode = 2;
  } else {
    process.exitCode = 1;
  }
}
