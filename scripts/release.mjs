#!/usr/bin/env node

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdtemp,
  readFile,
  readdir,
  rm,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";

const STATE_FILE = ".minutes-release-state.json";
const SDK_DIRECTORY = "crates/sdk";
const MCP_DIRECTORY = "crates/mcp";
const PHASE2_FILES = ["crates/mcp/package.json", "crates/mcp/package-lock.json"];
const DEFAULT_REGISTRY_URL = "https://registry.npmjs.org/";
const DEFAULT_POLL_DELAYS_MS = [5_000, 10_000, 20_000, 40_000, 60_000, 60_000, 60_000, 45_000];
const exactVersionPattern = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*)?$/;

const toolPath = process.env.RELEASE_TOOL_PATH;
const childEnvironment = {
  ...process.env,
  ...(toolPath ? { PATH: `${toolPath}${path.delimiter}${process.env.PATH ?? ""}` } : {}),
};

class CommandError extends Error {
  constructor(command, args, code, signal, stdout, stderr) {
    const outcome = signal ? `signal ${signal}` : `exit code ${code}`;
    const detail = stderr.trim() || stdout.trim();
    super(`${command} ${args.join(" ")} failed with ${outcome}${detail ? `\n${detail}` : ""}`);
    this.name = "CommandError";
    this.command = command;
    this.code = code;
    this.signal = signal;
    this.stdout = stdout;
    this.stderr = stderr;
  }
}

function exec(command, args, { cwd, input, env = childEnvironment } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, env, stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.once("error", reject);
    child.once("close", (code, signal) => {
      if (code === 0) resolve({ stdout, stderr });
      else reject(new CommandError(command, args, code, signal, stdout, stderr));
    });
    child.stdin.end(input);
  });
}

function usage() {
  return [
    "Usage:",
    "  node scripts/release.mjs phase1 <version>",
    "  node scripts/release.mjs phase2 <version>",
    "  node scripts/release.mjs tag <version> [--skip-ci-check]",
    "  node scripts/release.mjs status",
  ].join("\n");
}

function parseArgs(argv) {
  const [command, ...rest] = argv;
  if (command === "status") {
    if (rest.length !== 0) throw new Error("status does not accept arguments");
    return { command };
  }
  if (!["phase1", "phase2", "tag"].includes(command)) {
    throw new Error(command === undefined ? "a subcommand is required" : `unknown subcommand: ${command}`);
  }

  let version;
  let skipCiCheck = false;
  for (const argument of rest) {
    if (argument === "--skip-ci-check") {
      if (command !== "tag") throw new Error("--skip-ci-check is only valid with tag");
      if (skipCiCheck) throw new Error("--skip-ci-check may only be specified once");
      skipCiCheck = true;
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
  return { command, version, skipCiCheck };
}

async function repositoryRoot() {
  const { stdout } = await exec("git", ["rev-parse", "--show-toplevel"], { cwd: process.cwd() });
  return stdout.trim();
}

async function readJson(file) {
  return JSON.parse(await readFile(file, "utf8"));
}

async function readState(root) {
  try {
    return await readJson(path.join(root, STATE_FILE));
  } catch (error) {
    if (error && error.code === "ENOENT") return null;
    throw new Error(`cannot read ${STATE_FILE}: ${error instanceof Error ? error.message : String(error)}`);
  }
}

async function writeState(root, state) {
  await writeFile(path.join(root, STATE_FILE), `${JSON.stringify(state, null, 2)}\n`, "utf8");
}

function now() {
  return new Date().toISOString();
}

function requireStateVersion(state, version) {
  if (state === null) {
    throw new Error(`phase1 state is missing for ${version}; run node scripts/release.mjs phase1 ${version}`);
  }
  if (state.version !== version) {
    throw new Error(
      `${STATE_FILE} belongs to ${state.version ?? "an unknown version"}, not ${version}; finish or remove that state deliberately`,
    );
  }
  if (typeof state.sdkIntegrity !== "string" || !state.sdkIntegrity.startsWith("sha512-")) {
    throw new Error(`${STATE_FILE} has no valid sdkIntegrity`);
  }
}

function phase1Complete(state) {
  return ["phase1-complete", "phase2-complete", "tag-complete"].includes(state?.phase);
}

async function assertCleanAndPushed(root, { requireMain = false } = {}) {
  const { stdout: status } = await exec(
    "git",
    ["status", "--porcelain=v1", "--untracked-files=all"],
    { cwd: root },
  );
  if (status !== "") throw new Error("working tree is not clean; release commands require a committed tree");

  if (requireMain) {
    const { stdout: branchOutput } = await exec("git", ["rev-parse", "--abbrev-ref", "HEAD"], { cwd: root });
    if (branchOutput.trim() !== "main") {
      throw new Error(`phase1 must run on main (current branch: ${branchOutput.trim() || "unknown"})`);
    }
  }

  const [{ stdout: headOutput }, { stdout: upstreamOutput }] = await Promise.all([
    exec("git", ["rev-parse", "HEAD"], { cwd: root }),
    exec("git", ["rev-parse", "@{u}"], { cwd: root }),
  ]);
  if (headOutput.trim() !== upstreamOutput.trim()) {
    throw new Error("HEAD is not pushed to its upstream; push the release commit first");
  }
  return headOutput.trim();
}

async function runVersionCheck(root, release = false) {
  await exec(
    process.execPath,
    [path.join(root, "scripts", "check_version_sync.mjs"), ...(release ? ["--release"] : [])],
    { cwd: root },
  );
}

async function assertTreeVersion(root, version) {
  const sdk = await readJson(path.join(root, SDK_DIRECTORY, "package.json"));
  if (sdk.version !== version) {
    throw new Error(
      `tree version is ${sdk.version ?? "missing"}, not ${version}; run bump-version.mjs and merge the bump first`,
    );
  }
}

async function withPackedPackage(root, directory, callback) {
  const temporaryDirectory = await mkdtemp(path.join(os.tmpdir(), "minutes-release-pack-"));
  try {
    // minutes-sdk builds in prepublishOnly, a lifecycle that npm pack does not
    // run. Build explicitly so the provenance tarball contains the same dist/
    // payload that npm publish will pack. This is also harmlessly idempotent for
    // minutes-mcp and makes its publish artifact independent of an old dist/.
    await exec("npm", ["run", "build"], { cwd: path.join(root, directory) });
    await exec("npm", ["pack", "--json", "--pack-destination", temporaryDirectory], {
      cwd: path.join(root, directory),
    });
    const tarballs = (await readdir(temporaryDirectory)).filter((file) => file.endsWith(".tgz"));
    if (tarballs.length !== 1) {
      throw new Error(`npm pack in ${directory} produced ${tarballs.length} tarballs; expected exactly one`);
    }
    const tarball = path.join(temporaryDirectory, tarballs[0]);
    const bytes = await readFile(tarball);
    const integrity = `sha512-${createHash("sha512").update(bytes).digest("base64")}`;
    return await callback({ tarball, integrity });
  } finally {
    await rm(temporaryDirectory, { recursive: true, force: true });
  }
}

async function packPackage(root, directory) {
  return withPackedPackage(root, directory, ({ integrity }) => integrity);
}

function registryUrl() {
  const configured = process.env.RELEASE_REGISTRY_URL ?? DEFAULT_REGISTRY_URL;
  return configured.endsWith("/") ? configured : `${configured}/`;
}

async function registryLookup(packageName, version) {
  const url = new URL(`${encodeURIComponent(packageName)}/${encodeURIComponent(version)}`, registryUrl());
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 10_000);
  let response;
  try {
    response = await fetch(url, {
      headers: { accept: "application/json" },
      signal: controller.signal,
    });
  } catch (error) {
    return { kind: "server-error", detail: error instanceof Error ? error.message : String(error) };
  } finally {
    clearTimeout(timeout);
  }

  if (response.status === 404) return { kind: "missing" };
  if (response.status >= 500 && response.status <= 599) {
    return { kind: "server-error", detail: `HTTP ${response.status}` };
  }
  if (!response.ok) {
    throw new Error(`registry lookup for ${packageName}@${version} failed with HTTP ${response.status}`);
  }
  let metadata;
  try {
    metadata = await response.json();
  } catch (error) {
    throw new Error(
      `registry returned invalid JSON for ${packageName}@${version}: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  return { kind: "found", integrity: metadata?.dist?.integrity };
}

function assertRegistryIntegrity(packageName, version, actual, expected) {
  if (typeof actual !== "string") {
    throw new Error(`registry metadata for ${packageName}@${version} has no dist.integrity`);
  }
  if (actual !== expected) {
    throw new Error(
      `${packageName}@${version} already exists with different integrity\n` +
        `  registry: ${actual}\n  local:    ${expected}\nRefusing to replace published provenance.`,
    );
  }
}

function pollDelays() {
  const configured = process.env.RELEASE_POLL_DELAYS_MS;
  if (configured === undefined) return DEFAULT_POLL_DELAYS_MS;
  const delays = configured.split(",").map((value) => Number(value));
  if (delays.length === 0 || delays.some((value) => !Number.isFinite(value) || value < 0)) {
    throw new Error("RELEASE_POLL_DELAYS_MS must be a comma-separated list of non-negative numbers");
  }
  return delays;
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function pollForPackage(packageName, version, integrity) {
  const delays = pollDelays();
  let lastResult;
  for (let attempt = 0; attempt <= delays.length; attempt += 1) {
    lastResult = await registryLookup(packageName, version);
    if (lastResult.kind === "found") {
      assertRegistryIntegrity(packageName, version, lastResult.integrity, integrity);
      console.log(`Registry confirms ${packageName}@${version} (${integrity}).`);
      return;
    }
    if (attempt < delays.length) {
      const description = lastResult.kind === "missing" ? "404 (not visible yet)" : lastResult.detail;
      console.log(`Registry poll ${attempt + 1}: ${description}; retrying in ${delays[attempt]}ms.`);
      await sleep(delays[attempt]);
    }
  }
  const lastDescription = lastResult?.kind === "missing" ? "404" : lastResult?.detail ?? "unknown error";
  throw new Error(
    `${packageName}@${version} did not become visible before the registry polling timeout (last result: ${lastDescription}). ` +
      "This is the npm registry lag pattern that can leave minutes-mcp depending on an SDK version the registry cannot resolve; stop and retry phase1 later.",
  );
}

async function publishPackage(root, directory, packageName, version, integrity, { poll = false } = {}) {
  const firstLookup = await registryLookup(packageName, version);
  if (firstLookup.kind === "found") {
    assertRegistryIntegrity(packageName, version, firstLookup.integrity, integrity);
    console.log(`${packageName}@${version} is already published with matching integrity; skipping npm publish.`);
    if (poll) await pollForPackage(packageName, version, integrity);
    return false;
  }
  if (firstLookup.kind === "server-error") {
    throw new Error(
      `cannot safely determine whether ${packageName}@${version} already exists (${firstLookup.detail}); refusing to publish`,
    );
  }

  try {
    await exec(
      "npm",
      ["publish", "--access", "public", "--registry", registryUrl()],
      { cwd: path.join(root, directory) },
    );
  } catch (error) {
    // A concurrent publisher can win after our 404. Verify provenance before
    // deciding whether the failed publish is nevertheless an idempotent success.
    const afterFailure = await registryLookup(packageName, version);
    if (afterFailure.kind === "found") {
      assertRegistryIntegrity(packageName, version, afterFailure.integrity, integrity);
      console.log(`${packageName}@${version} appeared concurrently with matching integrity; continuing.`);
      if (poll) await pollForPackage(packageName, version, integrity);
      return false;
    }
    throw error;
  }

  console.log(`Published ${packageName}@${version}.`);
  if (poll) await pollForPackage(packageName, version, integrity);
  return true;
}

async function testMcpAgainstSdkTarball(root, tarball, sdkIntegrity) {
  let primaryError;
  try {
    const bytes = await readFile(tarball);
    const installedIntegrity = `sha512-${createHash("sha512").update(bytes).digest("base64")}`;
    if (installedIntegrity !== sdkIntegrity) throw new Error("SDK tarball changed between packing and MCP validation");

    await exec("npm", ["install", tarball, "--no-save", "--package-lock=false"], {
      cwd: path.join(root, MCP_DIRECTORY),
    });
    await exec("npm", ["run", "build"], { cwd: path.join(root, MCP_DIRECTORY) });
    await exec("npx", ["tsc", "--noEmit"], { cwd: path.join(root, MCP_DIRECTORY) });
  } catch (error) {
    primaryError = error;
  }

  try {
    await exec("npm", ["ci"], { cwd: path.join(root, MCP_DIRECTORY) });
  } catch (error) {
    if (primaryError === undefined) throw error;
    console.error(`Additionally failed to restore MCP node_modules with npm ci: ${error.message}`);
  }
  if (primaryError !== undefined) throw primaryError;
}

async function phase1(root, version) {
  await assertCleanAndPushed(root, { requireMain: true });
  await runVersionCheck(root);
  await assertTreeVersion(root, version);

  let existingState = await readState(root);
  if (existingState !== null && existingState.version !== version && existingState.phase === "tag-complete") {
    console.log(`Previous release ${existingState.version} is complete; starting release ${version}.`);
    existingState = null;
  }
  if (existingState !== null) requireStateVersion(existingState, version);

  const sdkPackage = await readJson(path.join(root, SDK_DIRECTORY, "package.json"));
  let state;
  const integrity = await withPackedPackage(root, SDK_DIRECTORY, async ({ tarball, integrity: packedIntegrity }) => {
    if (existingState?.sdkIntegrity && existingState.sdkIntegrity !== packedIntegrity) {
      throw new Error(
        `SDK provenance changed since phase1 began\n  state: ${existingState.sdkIntegrity}\n  local: ${packedIntegrity}`,
      );
    }
    state = {
      version,
      phase: phase1Complete(existingState) ? existingState.phase : "phase1-started",
      sdkPublished: existingState?.sdkPublished === true,
      sdkIntegrity: packedIntegrity,
      timestamps: {
        ...(existingState?.timestamps ?? {}),
        phase1StartedAt: existingState?.timestamps?.phase1StartedAt ?? now(),
      },
    };
    await writeState(root, state);
    if (!phase1Complete(existingState)) {
      await testMcpAgainstSdkTarball(root, tarball, packedIntegrity);
    }
    return packedIntegrity;
  });

  const publishedNow = await publishPackage(
    root,
    SDK_DIRECTORY,
    sdkPackage.name,
    version,
    integrity,
    { poll: false },
  );
  state.sdkPublished = true;
  state.timestamps.sdkPublishedAt = state.timestamps.sdkPublishedAt ?? now();
  if (publishedNow) state.timestamps.sdkPublishCommandCompletedAt = now();
  await writeState(root, state);

  await pollForPackage(sdkPackage.name, version, integrity);
  state.phase = phase1Complete(existingState) ? existingState.phase : "phase1-complete";
  state.timestamps.phase1CompletedAt = state.timestamps.phase1CompletedAt ?? now();
  await writeState(root, state);

  console.log("\nPhase 1 complete. Run Phase 2 from this checkout:");
  console.log(`  node scripts/release.mjs phase2 ${version}`);
}

async function changedFilesFromHead(root) {
  const { stdout } = await exec("git", ["diff", "HEAD", "--name-only", "-z", "--no-ext-diff"], { cwd: root });
  return stdout.split("\0").filter(Boolean).sort();
}

function assertOnlyPhase2Files(files, context) {
  const unexpected = files.filter((file) => !PHASE2_FILES.includes(file));
  if (unexpected.length > 0) {
    throw new Error(`phase2 diff restriction failed ${context}; unexpected files: ${unexpected.join(", ")}`);
  }
}

async function verifySdkStateOnRegistry(state) {
  const lookup = await registryLookup("minutes-sdk", state.version);
  if (lookup.kind !== "found") {
    const detail = lookup.kind === "missing" ? "404 (missing)" : lookup.detail;
    throw new Error(`minutes-sdk@${state.version} is not currently visible on the registry (${detail})`);
  }
  assertRegistryIntegrity("minutes-sdk", state.version, lookup.integrity, state.sdkIntegrity);
}

async function phase2(root, version) {
  const state = await readState(root);
  requireStateVersion(state, version);
  if (!phase1Complete(state)) {
    throw new Error(`phase1 is not complete for ${version} (current state: ${state.phase ?? "unknown"})`);
  }
  await verifySdkStateOnRegistry(state);

  const preexisting = await changedFilesFromHead(root);
  assertOnlyPhase2Files(preexisting, "before pinning");
  if (preexisting.length > 0) {
    throw new Error(`phase2 requires a clean committed tree; already changed: ${preexisting.join(", ")}`);
  }

  const packageFile = path.join(root, MCP_DIRECTORY, "package.json");
  const lockFile = path.join(root, MCP_DIRECTORY, "package-lock.json");
  const originalPackageText = await readFile(packageFile, "utf8");
  const originalLockText = await readFile(lockFile, "utf8");
  const packageJson = await readJson(packageFile);
  const lockJson = JSON.parse(originalLockText);
  if (
    packageJson.dependencies?.["minutes-sdk"] === version &&
    lockJson.packages?.[""]?.dependencies?.["minutes-sdk"] === version
  ) {
    await runVersionCheck(root, true);
    if (state.phase !== "tag-complete") state.phase = "phase2-complete";
    state.timestamps = { ...(state.timestamps ?? {}), phase2CompletedAt: state.timestamps?.phase2CompletedAt ?? now() };
    await writeState(root, state);
    console.log(`Phase 2 is already committed for ${version}; nothing to change.`);
    printPhase3Instructions(version);
    return;
  }
  if (!packageJson.dependencies || typeof packageJson.dependencies["minutes-sdk"] !== "string") {
    throw new Error('crates/mcp/package.json is missing dependencies["minutes-sdk"]');
  }
  let committed = false;
  try {
    packageJson.dependencies["minutes-sdk"] = version;
    await writeFile(packageFile, `${JSON.stringify(packageJson, null, 2)}\n`, "utf8");
    await exec("npm", ["install", "--package-lock-only"], { cwd: path.join(root, MCP_DIRECTORY) });

    const resultingLock = await readJson(lockFile);
    if (resultingLock.packages?.[""]?.dependencies?.["minutes-sdk"] !== version) {
      throw new Error(`npm did not regenerate package-lock.json with exact minutes-sdk ${version}`);
    }
    const changed = await changedFilesFromHead(root);
    assertOnlyPhase2Files(changed, "after pinning");
    if (changed.length === 0) throw new Error("phase2 unexpectedly produced no committed pin diff");
    await runVersionCheck(root, true);
    await exec("git", ["add", "--", ...PHASE2_FILES], { cwd: root });
    await exec("git", ["commit", "-m", `release: pin minutes-sdk ${version} for mcp`], { cwd: root });
    committed = true;
  } catch (error) {
    if (!committed) {
      try {
        await exec("git", ["restore", "--staged", "--", ...PHASE2_FILES], { cwd: root });
        await writeFile(packageFile, originalPackageText, "utf8");
        await writeFile(lockFile, originalLockText, "utf8");
      } catch (rollbackError) {
        throw new Error(
          `${error instanceof Error ? error.message : String(error)}\n` +
            `Additionally failed to roll back the Phase-2 manifest edits: ${rollbackError instanceof Error ? rollbackError.message : String(rollbackError)}`,
        );
      }
    }
    throw error;
  }

  state.phase = "phase2-complete";
  state.timestamps = { ...(state.timestamps ?? {}), phase2CompletedAt: now() };
  await writeState(root, state);
  console.log(`Committed exact minutes-sdk ${version} pin for MCP.`);
  printPhase3Instructions(version);
}

function printPhase3Instructions(version) {
  console.log("\nNext, push the Phase-2 commit, wait for CI on that exact HEAD to pass, then run:");
  console.log("  git push origin main");
  console.log(`  node scripts/release.mjs tag ${version}`);
}

async function assertCiGreen(root, head, skipCiCheck) {
  if (skipCiCheck) {
    console.log("Skipping CI verification because --skip-ci-check was explicitly supplied.");
    return;
  }
  let result;
  try {
    result = await exec(
      "gh",
      ["run", "list", "--commit", head, "--workflow", "CI", "--limit", "1", "--json", "status,conclusion,databaseId"],
      { cwd: root },
    );
  } catch (error) {
    throw new Error(
      `could not verify CI with gh (${error instanceof Error ? error.message : String(error)}). ` +
        "Install/authenticate gh, or rerun with --skip-ci-check only after manually verifying CI on this HEAD.",
    );
  }
  let runs;
  try {
    runs = JSON.parse(result.stdout);
  } catch {
    throw new Error("gh returned invalid JSON while checking CI; use --skip-ci-check only after manual verification");
  }
  if (!Array.isArray(runs) || runs.length === 0) {
    throw new Error(`no CI run found for HEAD ${head}; wait for CI before tagging`);
  }
  if (runs[0].status !== "completed" || runs[0].conclusion !== "success") {
    throw new Error(
      `CI is not green for HEAD ${head} (status=${runs[0].status ?? "unknown"}, conclusion=${runs[0].conclusion ?? "unknown"})`,
    );
  }
}

async function ensureAnnotatedTag(root, version, head) {
  const tag = `v${version}`;
  try {
    const { stdout } = await exec("git", ["rev-parse", "--verify", `${tag}^{commit}`], { cwd: root });
    if (stdout.trim() !== head) {
      throw new Error(`${tag} already exists at ${stdout.trim()}, not HEAD ${head}`);
    }
    const { stdout: typeOutput } = await exec("git", ["cat-file", "-t", `refs/tags/${tag}`], { cwd: root });
    if (typeOutput.trim() !== "tag") {
      throw new Error(`${tag} already exists at HEAD but is not an annotated tag`);
    }
    console.log(`${tag} already exists at HEAD; reusing it for this resumed release.`);
  } catch (error) {
    if (!(error instanceof CommandError)) throw error;
    await exec("git", ["tag", "-a", tag, "-m", tag], { cwd: root });
    console.log(`Created annotated tag ${tag}.`);
  }
  return tag;
}

async function tagRelease(root, version, skipCiCheck) {
  const state = await readState(root);
  requireStateVersion(state, version);
  if (!["phase2-complete", "tag-complete"].includes(state.phase)) {
    throw new Error(`phase2 is not complete for ${version} (current state: ${state.phase ?? "unknown"})`);
  }
  await verifySdkStateOnRegistry(state);

  const head = await assertCleanAndPushed(root);
  await assertCiGreen(root, head, skipCiCheck);
  await runVersionCheck(root, true);
  const sdkIntegrity = await packPackage(root, SDK_DIRECTORY);
  if (sdkIntegrity !== state.sdkIntegrity) {
    throw new Error(
      `SDK provenance mismatch: npm pack from HEAD no longer reproduces phase1\n` +
        `  phase1: ${state.sdkIntegrity}\n  HEAD:   ${sdkIntegrity}`,
    );
  }

  // Build and pack MCP before creating the tag. npm publish has no MCP build
  // lifecycle, so from the annotated tag onward every publish input is frozen.
  const mcpPackage = await readJson(path.join(root, MCP_DIRECTORY, "package.json"));
  const mcpIntegrity = await packPackage(root, MCP_DIRECTORY);

  const tag = await ensureAnnotatedTag(root, version, head);
  console.log(`Push it only after the draft GitHub release is ready:\n  git push origin ${tag}`);

  await publishPackage(root, MCP_DIRECTORY, mcpPackage.name, version, mcpIntegrity);

  state.phase = "tag-complete";
  state.timestamps = { ...(state.timestamps ?? {}), tagCompletedAt: now() };
  await writeState(root, state);
  console.log(`\nRelease command flow complete for ${tag}. The local tag has not been pushed.`);
}

async function printStatus(root) {
  const state = await readState(root);
  if (state === null) {
    console.log("no release in progress");
    return;
  }
  console.log(`release ${state.version ?? "<unknown>"}: ${state.phase ?? "<unknown phase>"}`);
  console.log(`sdk published: ${state.sdkPublished === true ? "yes" : "no"}`);
  if (state.sdkIntegrity) console.log(`sdk integrity: ${state.sdkIntegrity}`);
  if (state.timestamps && typeof state.timestamps === "object") {
    for (const [name, value] of Object.entries(state.timestamps)) console.log(`${name}: ${value}`);
  }
}

let options;
try {
  options = parseArgs(process.argv.slice(2));
  const root = await repositoryRoot();
  if (options.command === "status") await printStatus(root);
  else if (options.command === "phase1") await phase1(root, options.version);
  else if (options.command === "phase2") await phase2(root, options.version);
  else await tagRelease(root, options.version, options.skipCiCheck);
} catch (error) {
  console.error(`release: ${error instanceof Error ? error.message : String(error)}`);
  if (
    options === undefined ||
    (error instanceof Error && /^(?:a subcommand|a version|invalid version|unknown subcommand|unknown option|unexpected argument|--|status does not)/.test(error.message))
  ) {
    console.error(usage());
    process.exitCode = 2;
  } else {
    process.exitCode = 1;
  }
}
