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
import { normalizeNpmPackPermissions } from "./normalize_npm_pack_permissions.mjs";

const SDK_DIRECTORY = "crates/sdk";
const MCP_DIRECTORY = "crates/mcp";
const PHASE2_FILES = ["crates/mcp/package.json", "crates/mcp/package-lock.json"];
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
    "  node scripts/release.mjs phase1 <version> --dry-run",
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
  let dryRun = false;
  for (const argument of rest) {
    if (argument === "--dry-run") {
      if (command !== "phase1") throw new Error("--dry-run is only valid with phase1");
      if (dryRun) throw new Error("--dry-run may only be specified once");
      dryRun = true;
    } else if (argument === "--skip-ci-check") {
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
  return { command, version, skipCiCheck, dryRun };
}

async function repositoryRoot() {
  const { stdout } = await exec("git", ["rev-parse", "--show-toplevel"], { cwd: process.cwd() });
  return stdout.trim();
}

async function readJson(file) {
  return JSON.parse(await readFile(file, "utf8"));
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
  if (release) {
    // Site constants, with the test count binding. Per-PR CI tolerates a stale
    // count so a number that moves with every added test cannot redden
    // unrelated checks (#666), which means nothing refreshes it on its own.
    //
    // This has to run here rather than only in release-cli.yml: that workflow
    // triggers on the tag push, so a failure there arrives after the tag and
    // the draft release already exist, and the recovery is delete-and-retag,
    // which the immutable-tag policy forbids. Here it is a local failure with
    // no tag created yet.
    await exec(
      process.execPath,
      [path.join(root, "scripts", "sync_site_release_version.mjs"), "--check-release"],
      { cwd: root },
    );
  }
}

async function assertTreeVersion(root, version) {
  const sdk = await readJson(path.join(root, SDK_DIRECTORY, "package.json"));
  if (sdk.version !== version) {
    throw new Error(
      `tree version is ${sdk.version ?? "missing"}, not ${version}; run bump-version.mjs and merge the bump first`,
    );
  }
}

async function withPackedPackage(root, directory, callback, { localSdkTarball } = {}) {
  const temporaryDirectory = await mkdtemp(path.join(os.tmpdir(), "minutes-release-pack-"));
  try {
    if (localSdkTarball === undefined) {
      await exec("npm", ["ci"], { cwd: path.join(root, directory) });
    } else {
      if (directory !== MCP_DIRECTORY) {
        throw new Error("a local SDK tarball may only be used while packing minutes-mcp");
      }
      await exec(
        "npm",
        ["install", localSdkTarball, "--no-save"],
        { cwd: path.join(root, directory) },
      );
    }
    // minutes-sdk builds in prepublishOnly, a lifecycle that npm pack does not
    // run. Build explicitly so the provenance tarball contains the same dist/
    // payload that npm publish will pack. This is also harmlessly idempotent for
    // minutes-mcp and makes its publish artifact independent of an old dist/.
    await exec("npm", ["run", "build"], { cwd: path.join(root, directory) });
    await normalizeNpmPackPermissions(path.join(root, directory));
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

async function packPackage(root, directory, options) {
  return withPackedPackage(root, directory, ({ integrity }) => integrity, options);
}

async function testMcpAgainstSdkTarball(root, tarball, sdkIntegrity) {
  let primaryError;
  try {
    const bytes = await readFile(tarball);
    const installedIntegrity = `sha512-${createHash("sha512").update(bytes).digest("base64")}`;
    if (installedIntegrity !== sdkIntegrity) throw new Error("SDK tarball changed between packing and MCP validation");

    // Keep the committed dependency graph while overlaying this exact SDK.
    // --no-save prevents manifest and lockfile writes; disabling the lockfile
    // also discards its dependency resolutions and can pull unrelated peers.
    await exec("npm", ["install", tarball, "--no-save"], {
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

async function phase1(root, version, dryRun) {
  if (!dryRun) {
    throw new Error(
      "phase1 is a preflight-only command; pass --dry-run (registry publishing begins only after the release tag is pushed)",
    );
  }
  await assertCleanAndPushed(root, { requireMain: true });
  await runVersionCheck(root);
  await assertTreeVersion(root, version);

  const integrity = await withPackedPackage(root, SDK_DIRECTORY, async ({ tarball, integrity: packedIntegrity }) => {
    await testMcpAgainstSdkTarball(root, tarball, packedIntegrity);
    return packedIntegrity;
  });

  console.log(`Credential-free SDK preflight complete (${integrity}). No package was published.`);
  console.log("\nRun Phase 2 from this checkout:");
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

async function phase2(root, version) {
  await runVersionCheck(root);
  await assertTreeVersion(root, version);

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
    console.log(`Phase 2 is already committed for ${version}; nothing to change.`);
    printPhase3Instructions(version);
    return;
  }
  if (!packageJson.dependencies || typeof packageJson.dependencies["minutes-sdk"] !== "string") {
    throw new Error('crates/mcp/package.json is missing dependencies["minutes-sdk"]');
  }
  const dependencyText = `"minutes-sdk": ${JSON.stringify(packageJson.dependencies["minutes-sdk"])}`;
  const pinnedPackageText = originalPackageText.replace(
    dependencyText,
    `"minutes-sdk": ${JSON.stringify(version)}`,
  );
  if (pinnedPackageText === originalPackageText) {
    throw new Error("could not locate the minutes-sdk dependency text in crates/mcp/package.json");
  }
  let committed = false;
  try {
    await withPackedPackage(root, SDK_DIRECTORY, async ({ tarball, integrity }) => {
      packageJson.dependencies["minutes-sdk"] = version;
      await writeFile(packageFile, pinnedPackageText, "utf8");

      // Let npm derive the lock entry from the exact local SDK artifact. Then
      // replace only the temporary file reference with the registry URL that
      // will become valid after the trusted tag workflow publishes the same
      // tarball. This keeps Phase 2 credential-free and registry-independent.
      await exec(
        "npm",
        [
          "install",
          "--package-lock-only",
          "--ignore-scripts",
          "--no-audit",
          "--no-fund",
          "--save-exact",
          tarball,
        ],
        { cwd: path.join(root, MCP_DIRECTORY) },
      );

      const resultingLock = await readJson(lockFile);
      const lockedSdk = resultingLock.packages?.["node_modules/minutes-sdk"];
      if (lockedSdk?.version !== version || lockedSdk.integrity !== integrity) {
        throw new Error(`npm did not lock the exact local minutes-sdk ${version} artifact`);
      }
      resultingLock.packages[""].dependencies["minutes-sdk"] = version;
      lockedSdk.resolved = `https://registry.npmjs.org/minutes-sdk/-/minutes-sdk-${version}.tgz`;
      await writeFile(packageFile, pinnedPackageText, "utf8");
      await writeFile(lockFile, `${JSON.stringify(resultingLock, null, 2)}\n`, "utf8");
    });

    const resultingLock = await readJson(lockFile);
    if (resultingLock.packages?.[""]?.dependencies?.["minutes-sdk"] !== version) {
      throw new Error(`package-lock.json does not contain exact minutes-sdk ${version}`);
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
  const head = await assertCleanAndPushed(root);
  await assertCiGreen(root, head, skipCiCheck);
  await runVersionCheck(root, true);
  await assertTreeVersion(root, version);
  let sdkIntegrity;
  let mcpIntegrity;
  await withPackedPackage(root, SDK_DIRECTORY, async ({ tarball, integrity }) => {
    sdkIntegrity = integrity;

    // Build and pack MCP against this exact, still-unpublished SDK artifact
    // before creating the tag. The trusted workflow publishes SDK first, but
    // the pre-tag proof cannot depend on a registry version that does not exist.
    mcpIntegrity = await packPackage(root, MCP_DIRECTORY, { localSdkTarball: tarball });
  });

  const tag = await ensureAnnotatedTag(root, version, head);
  console.log(`Push it only after the draft GitHub release is ready:\n  git push origin ${tag}`);
  console.log(`Packed minutes-sdk (${sdkIntegrity}) and minutes-mcp (${mcpIntegrity}) without publishing.`);
  console.log(`\nLocal tag ${tag} is ready. Registry publishing begins only in the tag-triggered workflow.`);
}

async function printStatus(root) {
  const sdkPackage = await readJson(path.join(root, SDK_DIRECTORY, "package.json"));
  const mcpPackage = await readJson(path.join(root, MCP_DIRECTORY, "package.json"));
  console.log(`release inputs: minutes-sdk ${sdkPackage.version}; minutes-mcp ${mcpPackage.version}`);
  console.log(`MCP SDK pin: ${mcpPackage.dependencies?.["minutes-sdk"] ?? "missing"}`);
  console.log("registry publishing: tag-triggered workflow only");
}

let options;
try {
  options = parseArgs(process.argv.slice(2));
  const root = await repositoryRoot();
  if (options.command === "status") await printStatus(root);
  else if (options.command === "phase1") await phase1(root, options.version, options.dryRun);
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
