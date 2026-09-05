import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const scriptsDirectory = path.dirname(process.argv[1]);
const releaseScript = path.resolve(scriptsDirectory, "release.mjs");
const version = "1.2.3";

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: "utf8", ...options });
  assert.equal(result.error, undefined, result.error?.message);
  return result;
}

function git(root, ...args) {
  return run("git", args, { cwd: root });
}

async function writeFixture(root, file, contents) {
  const destination = path.join(root, file);
  await mkdir(path.dirname(destination), { recursive: true });
  await writeFile(destination, contents);
}

async function writeJson(root, file, value) {
  await writeFixture(root, file, `${JSON.stringify(value, null, 2)}\n`);
}

async function installToolShims(root) {
  const tools = path.join(root, ".release-tools");
  await mkdir(tools, { recursive: true });

  await writeFixture(
    tools,
    "git",
    `#!/usr/bin/env node
import { spawnSync } from "node:child_process";
const result = spawnSync(process.env.RELEASE_REAL_GIT, process.argv.slice(2), { stdio: "inherit" });
if (result.error) throw result.error;
process.exit(result.status ?? 1);
`,
  );
  await writeFixture(
    tools,
    "npm",
    `#!/usr/bin/env node
import { appendFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import path from "node:path";
const args = process.argv.slice(2);
await appendFile(process.env.RELEASE_SHIM_LOG, "npm " + args.join(" ") + " @ " + process.cwd() + "\\n");
const packageJson = JSON.parse(await readFile(path.join(process.cwd(), "package.json"), "utf8"));
if (args[0] === "pack") {
  const destination = args[args.indexOf("--pack-destination") + 1];
  await mkdir(destination, { recursive: true });
  const variant = packageJson.name === "minutes-sdk"
    ? (process.env.RELEASE_SHIM_SDK_VARIANT || "original")
    : (process.env.RELEASE_SHIM_MCP_VARIANT || "original");
  const filename = packageJson.name + "-" + packageJson.version + ".tgz";
  await writeFile(path.join(destination, filename), packageJson.name + ":" + packageJson.version + ":" + variant + "\\n");
  console.log(JSON.stringify([{ filename }]));
} else if (args[0] === "publish") {
  await writeFile(path.join(process.env.RELEASE_SHIM_MARKERS, "published-" + packageJson.name), "yes\\n");
} else if (args[0] === "install" && args.includes("--package-lock-only")) {
  const lockFile = path.join(process.cwd(), "package-lock.json");
  const lock = JSON.parse(await readFile(lockFile, "utf8"));
  const tarball = args.find((argument) => argument.endsWith(".tgz"));
  if (tarball) {
    const version = path.basename(tarball).match(/^minutes-sdk-(.+)\\.tgz$/)?.[1];
    const integrity = "sha512-" + createHash("sha512").update(await readFile(tarball)).digest("base64");
    const fileDependency = "file:" + tarball;
    packageJson.dependencies["minutes-sdk"] = fileDependency;
    await writeFile(path.join(process.cwd(), "package.json"), JSON.stringify(packageJson, null, 2) + "\\n");
    lock.packages[""].dependencies["minutes-sdk"] = fileDependency;
    lock.packages["node_modules/minutes-sdk"] = { version, resolved: fileDependency, integrity, license: "MIT" };
  } else {
    lock.packages[""].dependencies["minutes-sdk"] = packageJson.dependencies["minutes-sdk"];
  }
  await writeFile(lockFile, JSON.stringify(lock, null, 2) + "\\n");
}
`,
  );
  await writeFixture(
    tools,
    "npx",
    `#!/usr/bin/env node
import { appendFile } from "node:fs/promises";
await appendFile(process.env.RELEASE_SHIM_LOG, "npx " + process.argv.slice(2).join(" ") + " @ " + process.cwd() + "\\n");
`,
  );
  await writeFixture(
    tools,
    "gh",
    `#!/usr/bin/env node
if (process.env.RELEASE_SHIM_GH_FAIL === "1") {
  console.error("simulated gh failure");
  process.exit(42);
}
console.log(JSON.stringify([{
  status: process.env.RELEASE_SHIM_CI_STATUS || "completed",
  conclusion: process.env.RELEASE_SHIM_CI_CONCLUSION || "success",
  databaseId: 123
}]));
`,
  );

  for (const tool of ["git", "npm", "npx", "gh"]) await chmod(path.join(tools, tool), 0o755);
  return tools;
}

async function makeRepo(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), "minutes-release-fixture-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const remote = `${root}-remote.git`;
  t.after(() => rm(remote, { recursive: true, force: true }));

  await writeFixture(
    root,
    ".gitignore",
    "/.minutes-release-state.json\n/.release-tools/\n/.release-markers/\n/.release-shim.log\n",
  );
  await writeJson(root, "crates/sdk/package.json", { name: "minutes-sdk", version });
  await writeJson(root, "crates/mcp/package.json", {
    name: "minutes-mcp",
    version,
    dependencies: { "minutes-sdk": "^1.0.0" },
  });
  await writeJson(root, "crates/mcp/package-lock.json", {
    name: "minutes-mcp",
    version,
    lockfileVersion: 3,
    packages: {
      "": { name: "minutes-mcp", version, dependencies: { "minutes-sdk": "^1.0.0" } },
    },
  });
  await writeFixture(
    root,
    "scripts/check_version_sync.mjs",
    `import { readFile } from "node:fs/promises";
if (process.argv.includes("--release") && process.env.RELEASE_CHECK_FAIL === "release") {
  console.error("simulated --release policy failure");
  process.exit(1);
}
if (process.argv.includes("--release")) {
  const sdk = JSON.parse(await readFile("crates/sdk/package.json", "utf8"));
  const mcp = JSON.parse(await readFile("crates/mcp/package.json", "utf8"));
  if (mcp.dependencies["minutes-sdk"] !== sdk.version) process.exit(1);
}
console.log("Version sync check passed.");
`,
  );
  // The tag phase also gates on the site release constants, with the test
  // count binding. Stubbed like check_version_sync above: the real script
  // scans the whole repo for tests, which this fixture does not have.
  await writeFixture(
    root,
    "scripts/sync_site_release_version.mjs",
    `if (process.argv.includes("--check-release") && process.env.RELEASE_SITE_CHECK_FAIL === "1") {
  console.error("simulated stale site release constants");
  process.exit(1);
}
console.log("site release constants already match.");
`,
  );
  await writeFixture(root, "extra.txt", "original\n");

  assert.equal(git(root, "init", "-q", "-b", "main").status, 0);
  assert.equal(git(root, "config", "user.email", "fixture@example.com").status, 0);
  assert.equal(git(root, "config", "user.name", "Fixture Test").status, 0);
  assert.equal(git(root, "add", ".").status, 0);
  assert.equal(git(root, "commit", "-qm", "fixture").status, 0);
  assert.equal(run("git", ["init", "--bare", "-q", remote]).status, 0);
  assert.equal(git(root, "remote", "add", "origin", remote).status, 0);
  assert.equal(git(root, "push", "-qu", "origin", "main").status, 0);

  const tools = await installToolShims(root);
  const log = path.join(root, ".release-shim.log");
  const markers = path.join(root, ".release-markers");
  await writeFile(log, "");
  await mkdir(markers, { recursive: true });
  return { root, tools, log, markers };
}

function runRelease(fixture, args, extraEnvironment = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [releaseScript, ...args], {
      cwd: fixture.root,
      env: {
        ...process.env,
        RELEASE_TOOL_PATH: fixture.tools,
        RELEASE_REAL_GIT: run("which", ["git"]).stdout.trim(),
        RELEASE_SHIM_LOG: fixture.log,
        RELEASE_SHIM_MARKERS: fixture.markers,
        ...extraEnvironment,
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.once("error", reject);
    child.once("close", (status, signal) => resolve({ status, signal, stdout, stderr }));
  });
}

function assertSucceeded(result) {
  assert.equal(result.status, 0, result.stderr || result.stdout);
}

async function completePhase1(fixture, extraEnvironment = {}) {
  const result = await runRelease(fixture, ["phase1", version, "--dry-run"], extraEnvironment);
  assertSucceeded(result);
  return result;
}

async function completePhase2(fixture) {
  const result = await runRelease(fixture, ["phase2", version]);
  assertSucceeded(result);
  return result;
}

test("happy path preflights, pins, and tags without publishing before the tag push", async (t) => {
  const fixture = await makeRepo(t);
  const phase1 = await completePhase1(fixture);
  assert.match(phase1.stdout, /Credential-free SDK preflight complete/);
  assert.match(phase1.stdout, /No package was published/);
  const phase2 = await completePhase2(fixture);
  assert.match(phase2.stdout, /Committed exact minutes-sdk 1\.2\.3 pin/);
  assert.equal(git(fixture.root, "push", "-q", "origin", "main").status, 0);

  const phase3 = await runRelease(fixture, ["tag", version]);
  assertSucceeded(phase3);
  assert.match(phase3.stdout, /Created annotated tag v1\.2\.3/);
  assert.match(phase3.stdout, /git push origin v1\.2\.3/);
  assert.equal(git(fixture.root, "tag", "--list", `v${version}`).stdout.trim(), `v${version}`);

  const mcpPackage = JSON.parse(await readFile(path.join(fixture.root, "crates/mcp/package.json"), "utf8"));
  assert.equal(mcpPackage.dependencies["minutes-sdk"], version);
  const log = await readFile(fixture.log, "utf8");
  assert.match(log, /npm pack .*crates\/sdk/);
  assert.match(log, /npm install .*minutes-sdk-1\.2\.3\.tgz --no-save .*crates\/mcp/);
  assert.doesNotMatch(log, /--package-lock=false/);
  assert.match(log, /npm pack .*crates\/mcp/);
  assert.doesNotMatch(log, /npm publish/);
});

test("phase1 refuses to run without the explicit dry-run flag", async (t) => {
  const fixture = await makeRepo(t);
  const result = await runRelease(fixture, ["phase1", version]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /preflight-only command; pass --dry-run/);
  const log = await readFile(fixture.log, "utf8");
  assert.doesNotMatch(log, /npm (?:pack|publish)/);
});

test("status reports committed release inputs without legacy publish state", async (t) => {
  const fixture = await makeRepo(t);
  const result = await runRelease(fixture, ["status"]);
  assertSucceeded(result);
  assert.match(result.stdout, /release inputs: minutes-sdk 1\.2\.3; minutes-mcp 1\.2\.3/);
  assert.match(result.stdout, /MCP SDK pin: \^1\.0\.0/);
  assert.match(result.stdout, /registry publishing: tag-triggered workflow only/);
});

test("phase1 dry-run tests MCP against the packed SDK and restores dependencies", async (t) => {
  const fixture = await makeRepo(t);
  await completePhase1(fixture);
  const log = await readFile(fixture.log, "utf8");
  assert.match(log, /npm pack .*crates\/sdk/);
  assert.match(log, /npm install .*\.tgz --no-save .*crates\/mcp/);
  assert.doesNotMatch(log, /--package-lock=false/);
  assert.match(log, /npm run build .*crates\/mcp/);
  assert.match(log, /npx tsc --noEmit .*crates\/mcp/);
  assert.match(log, /npm ci .*crates\/mcp/);
  assert.doesNotMatch(log, /npm publish/);
});

test("phase2 diff restriction aborts when another tracked file is dirty", async (t) => {
  const fixture = await makeRepo(t);
  await writeFile(path.join(fixture.root, "extra.txt"), "modified\n");
  const result = await runRelease(fixture, ["phase2", version]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /phase2 diff restriction failed/);
  assert.match(result.stderr, /extra\.txt/);
});

test("phase1 is optional and tag packs current inputs without publishing", async (t) => {
  const fixture = await makeRepo(t);
  await completePhase2(fixture);
  assert.equal(git(fixture.root, "push", "-q", "origin", "main").status, 0);
  const result = await runRelease(fixture, ["tag", version]);
  assertSucceeded(result);
  assert.match(result.stdout, /Packed minutes-sdk .* and minutes-mcp .* without publishing/);
  assert.equal(git(fixture.root, "tag", "--list", `v${version}`).stdout.trim(), `v${version}`);
  const log = await readFile(fixture.log, "utf8");
  assert.match(log, /npm install .*minutes-sdk-1\.2\.3\.tgz --no-save .*crates\/mcp/);
  assert.doesNotMatch(log, /--package-lock=false/);
  assert.doesNotMatch(log, /npm publish/);
});

test("tag aborts when the --release version policy check fails", async (t) => {
  const fixture = await makeRepo(t);
  await completePhase2(fixture);
  assert.equal(git(fixture.root, "push", "-q", "origin", "main").status, 0);
  const result = await runRelease(fixture, ["tag", version], { RELEASE_CHECK_FAIL: "release" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /simulated --release policy failure/);
  assert.equal(git(fixture.root, "tag", "--list", `v${version}`).stdout.trim(), "");
});

test("tag aborts when the site release constants are stale", async (t) => {
  // The site test count is tolerated per-PR so a number that moves with every
  // added test cannot redden unrelated CI (#666), which means nothing refreshes
  // it on its own. It is binding here instead. This must fail BEFORE the tag
  // exists: release-cli.yml runs the same check, but only on the tag push, and
  // by then the immutable-tag policy rules out simply retagging.
  const fixture = await makeRepo(t);
  await completePhase2(fixture);
  assert.equal(git(fixture.root, "push", "-q", "origin", "main").status, 0);
  const result = await runRelease(fixture, ["tag", version], { RELEASE_SITE_CHECK_FAIL: "1" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /simulated stale site release constants/);
  assert.equal(git(fixture.root, "tag", "--list", `v${version}`).stdout.trim(), "");
});
