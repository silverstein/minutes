import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { existsSync } from "node:fs";
import http from "node:http";
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

function packageBytes(name, packageVersion, variant = "original") {
  return Buffer.from(`${name}:${packageVersion}:${variant}\n`);
}

function integrityFor(name, packageVersion, variant = "original") {
  return `sha512-${createHash("sha512").update(packageBytes(name, packageVersion, variant)).digest("base64")}`;
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
  lock.packages[""].dependencies["minutes-sdk"] = packageJson.dependencies["minutes-sdk"];
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

async function startRegistry(t, root, config) {
  const markers = path.join(root, ".release-markers");
  await mkdir(markers, { recursive: true });
  const server = http.createServer((request, response) => {
    const segments = new URL(request.url, "http://fixture.invalid").pathname
      .split("/")
      .filter(Boolean)
      .map(decodeURIComponent);
    const packageName = segments[0];
    const packageVersion = segments[1];
    if (!packageName || packageVersion !== version) {
      response.writeHead(404).end(JSON.stringify({ error: "not found" }));
      return;
    }

    const sequence = config.sequences?.[packageName];
    if (sequence?.length) {
      const status = sequence.shift();
      if (status !== 200) {
        response.writeHead(status).end(JSON.stringify({ error: `fixture ${status}` }));
        return;
      }
    }

    let packageIntegrity = config.integrities?.[packageName];
    if (packageIntegrity === undefined && existsSync(path.join(markers, `published-${packageName}`))) {
      const remainingLag = config.visibilityLag?.[packageName] ?? 0;
      if (remainingLag > 0) {
        config.visibilityLag[packageName] = remainingLag - 1;
        response.writeHead(404).end(JSON.stringify({ error: "not visible yet" }));
        return;
      }
      packageIntegrity = config.expected[packageName];
    }
    if (packageIntegrity === undefined) {
      response.writeHead(404).end(JSON.stringify({ error: "not found" }));
      return;
    }
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ name: packageName, version, dist: { integrity: packageIntegrity } }));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const address = server.address();
  return { markers, url: `http://127.0.0.1:${address.port}/` };
}

async function makeRepo(t, registryConfig = {}) {
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
  await writeFile(log, "");
  const expected = {
    "minutes-sdk": integrityFor("minutes-sdk", version),
    "minutes-mcp": integrityFor("minutes-mcp", version),
  };
  const config = {
    expected,
    integrities: {},
    visibilityLag: {},
    ...registryConfig,
  };
  config.expected = { ...expected, ...(registryConfig.expected ?? {}) };
  config.integrities = { ...(registryConfig.integrities ?? {}) };
  config.visibilityLag = { ...(registryConfig.visibilityLag ?? {}) };
  const registry = await startRegistry(t, root, config);
  return { root, tools, log, config, registry };
}

function runRelease(fixture, args, extraEnvironment = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [releaseScript, ...args], {
      cwd: fixture.root,
      env: {
      ...process.env,
      RELEASE_TOOL_PATH: fixture.tools,
      RELEASE_REAL_GIT: run("which", ["git"]).stdout.trim(),
      RELEASE_REGISTRY_URL: fixture.registry.url,
      RELEASE_POLL_DELAYS_MS: "1,1,1",
      RELEASE_SHIM_LOG: fixture.log,
      RELEASE_SHIM_MARKERS: fixture.registry.markers,
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
  const result = await runRelease(fixture, ["phase1", version], extraEnvironment);
  assertSucceeded(result);
  return result;
}

async function completePhase2(fixture) {
  const result = await runRelease(fixture, ["phase2", version]);
  assertSucceeded(result);
  return result;
}

test("happy path completes phase1, phase2, and tag with resumable state", async (t) => {
  const fixture = await makeRepo(t);
  const phase1 = await completePhase1(fixture);
  assert.match(phase1.stdout, /Phase 1 complete/);
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
  const state = JSON.parse(await readFile(path.join(fixture.root, ".minutes-release-state.json"), "utf8"));
  assert.equal(state.phase, "tag-complete");
  assert.equal(state.sdkPublished, true);
  assert.equal(state.sdkIntegrity, integrityFor("minutes-sdk", version));
  const log = await readFile(fixture.log, "utf8");
  assert.match(log, /npm publish .*crates\/sdk/);
  assert.match(log, /npm publish .*crates\/mcp/);
});

test("phase1 polls through registry 404 responses until the SDK is visible", async (t) => {
  const fixture = await makeRepo(t, { visibilityLag: { "minutes-sdk": 2 } });
  const result = await completePhase1(fixture);
  assert.match(result.stdout, /404 \(not visible yet\)/);
  assert.match(result.stdout, /Registry confirms minutes-sdk@1\.2\.3/);
});

test("phase1 polling timeout fails with the npm lag-pattern explanation", async (t) => {
  const fixture = await makeRepo(t, { visibilityLag: { "minutes-sdk": 99 } });
  const result = await runRelease(fixture, ["phase1", version], { RELEASE_POLL_DELAYS_MS: "1,1" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /registry lag pattern/);
  const state = JSON.parse(await readFile(path.join(fixture.root, ".minutes-release-state.json"), "utf8"));
  assert.equal(state.sdkPublished, true);
  assert.equal(state.phase, "phase1-started");
});

test("phase1 skips an existing SDK version with matching integrity", async (t) => {
  const fixture = await makeRepo(t, {
    integrities: { "minutes-sdk": integrityFor("minutes-sdk", version) },
  });
  const result = await completePhase1(fixture);
  assert.match(result.stdout, /already published with matching integrity; skipping npm publish/);
  const log = await readFile(fixture.log, "utf8");
  assert.doesNotMatch(log, /npm publish/);
});

test("phase1 aborts when an existing SDK version has different integrity", async (t) => {
  const fixture = await makeRepo(t, {
    integrities: { "minutes-sdk": "sha512-not-the-local-package" },
  });
  const result = await runRelease(fixture, ["phase1", version]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /already exists with different integrity/);
  assert.match(result.stderr, /Refusing to replace published provenance/);
});

test("phase2 diff restriction aborts when another tracked file is dirty", async (t) => {
  const fixture = await makeRepo(t);
  await completePhase1(fixture);
  await writeFile(path.join(fixture.root, "extra.txt"), "modified\n");
  const result = await runRelease(fixture, ["phase2", version]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /phase2 diff restriction failed/);
  assert.match(result.stderr, /extra\.txt/);
});

test("tag aborts when SDK tarball provenance no longer matches phase1", async (t) => {
  const fixture = await makeRepo(t);
  await completePhase1(fixture);
  await completePhase2(fixture);
  assert.equal(git(fixture.root, "push", "-q", "origin", "main").status, 0);
  const result = await runRelease(fixture, ["tag", version], { RELEASE_SHIM_SDK_VARIANT: "changed" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /SDK provenance mismatch/);
  assert.equal(git(fixture.root, "tag", "--list", `v${version}`).stdout.trim(), "");
});

test("tag aborts when the --release version policy check fails", async (t) => {
  const fixture = await makeRepo(t);
  await completePhase1(fixture);
  await completePhase2(fixture);
  assert.equal(git(fixture.root, "push", "-q", "origin", "main").status, 0);
  const result = await runRelease(fixture, ["tag", version], { RELEASE_CHECK_FAIL: "release" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /simulated --release policy failure/);
  assert.equal(git(fixture.root, "tag", "--list", `v${version}`).stdout.trim(), "");
});
