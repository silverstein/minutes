import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const scriptsDirectory = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptsDirectory, "..");
const sourceScript = path.join(scriptsDirectory, "install_mcp_dependencies.mjs");
const permissionsScript = path.join(
  scriptsDirectory,
  "normalize_npm_pack_permissions.mjs",
);

async function writeJson(root, file, value) {
  const target = path.join(root, file);
  await mkdir(path.dirname(target), { recursive: true });
  await writeFile(target, `${JSON.stringify(value, null, 2)}\n`);
}

async function makeFixture(t, dependency, integrity = "sha512-fixture") {
  const root = await mkdtemp(path.join(os.tmpdir(), "minutes-mcp-install-test-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(path.join(root, "scripts"), { recursive: true });
  await cp(sourceScript, path.join(root, "scripts", "install_mcp_dependencies.mjs"));
  await cp(permissionsScript, path.join(root, "scripts", "normalize_npm_pack_permissions.mjs"));
  await writeJson(root, "crates/sdk/package.json", { name: "minutes-sdk", version: "1.2.3" });
  await writeJson(root, "crates/mcp/package.json", {
    name: "minutes-mcp",
    dependencies: { "minutes-sdk": dependency },
  });
  await writeJson(root, "crates/mcp/package-lock.json", {
    lockfileVersion: 3,
    packages: {
      "": { dependencies: { "minutes-sdk": dependency } },
      "node_modules/minutes-sdk": {
        version: "1.2.3",
        resolved: "https://registry.npmjs.org/minutes-sdk/-/minutes-sdk-1.2.3.tgz",
        integrity,
      },
    },
  });

  const tools = path.join(root, "tools");
  const log = path.join(root, "npm.log");
  await mkdir(tools, { recursive: true });
  const npmShim = path.join(tools, "npm");
  await writeFile(
    npmShim,
    `#!/usr/bin/env node
import { appendFile, mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
const args = process.argv.slice(2);
await appendFile(process.env.NPM_TEST_LOG, args.join(" ") + " @ " + process.cwd() + "\\n");
if (args[0] === "ci" && args.includes("--cache")) {
  const lock = JSON.parse(await readFile("package-lock.json", "utf8"));
  await appendFile(process.env.NPM_TEST_LOG, "installed-integrity=" + lock.packages["node_modules/minutes-sdk"].integrity + "\\n");
  if (process.env.NPM_TEST_FAIL_INSTALL === "1") process.exit(42);
}
if (args[0] === "pack") {
  const destination = args[args.indexOf("--pack-destination") + 1];
  await mkdir(destination, { recursive: true });
  await writeFile(path.join(destination, "minutes-sdk-1.2.3.tgz"), "fixture");
  console.log(JSON.stringify([{ filename: "minutes-sdk-1.2.3.tgz", integrity: "sha512-fixture" }]));
}
`,
  );
  await chmod(npmShim, 0o755);
  await writeFile(log, "");
  return { root, tools, log };
}

function runFixture(fixture, args = [], env = {}) {
  return spawnSync(process.execPath, [path.join(fixture.root, "scripts", "install_mcp_dependencies.mjs"), ...args], {
    cwd: fixture.root,
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${fixture.tools}${path.delimiter}${process.env.PATH}`,
      NPM_TEST_LOG: fixture.log,
      // Do not inherit the developer's strict-install policy or failure injection.
      MINUTES_MCP_REQUIRE_PUBLISHED_SDK: "0",
      NPM_TEST_FAIL_INSTALL: "0",
      ...env,
    },
  });
}

test("uses ordinary npm ci while MCP still targets the published SDK", async (t) => {
  const fixture = await makeFixture(t, "0.24.0");
  const result = runFixture(fixture);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /published minutes-sdk 0\.24\.0/);
  const log = await readFile(fixture.log, "utf8");
  assert.match(log, /ci @ .*crates\/mcp/);
  assert.doesNotMatch(log, /pack --json/);
});

test("seeds npm from the exact local SDK when the release pin is unpublished", async (t) => {
  const fixture = await makeFixture(t, "1.2.3");
  const result = runFixture(fixture);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /exact local minutes-sdk 1\.2\.3 tarball/);
  const log = await readFile(fixture.log, "utf8");
  assert.match(log, /ci @ .*crates\/sdk/);
  assert.match(log, /run build @ .*crates\/sdk/);
  assert.match(log, /pack --json --pack-destination/);
  assert.match(log, /cache add .*minutes-sdk-1\.2\.3\.tgz --cache/);
  assert.match(log, /ci --cache .* @ .*crates\/mcp/);
});

test("strict published policy refuses mismatched SDK without changing the lock", async (t) => {
  const fixture = await makeFixture(t, "1.2.3", "sha512-different");
  const lockPath = path.join(fixture.root, "crates/mcp/package-lock.json");
  const originalLock = await readFile(lockPath, "utf8");
  const result = runFixture(fixture, ["--sdk-ready"], { MINUTES_MCP_REQUIRE_PUBLISHED_SDK: "1" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /integrity does not match the MCP lockfile/);
  const log = await readFile(fixture.log, "utf8");
  assert.doesNotMatch(log, /ci --cache .*crates\/mcp/);
  assert.equal(await readFile(lockPath, "utf8"), originalLock);
});

test("local SDK policy installs changed source and restores the exact committed lock", async (t) => {
  const fixture = await makeFixture(t, "1.2.3", "sha512-different");
  const lockPath = path.join(fixture.root, "crates/mcp/package-lock.json");
  const originalLock = await readFile(lockPath, "utf8");
  const result = runFixture(fixture, ["--sdk-ready"]);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /using its archive integrity for this install only/);
  assert.match(await readFile(fixture.log, "utf8"), /installed-integrity=sha512-fixture/);
  assert.equal(await readFile(lockPath, "utf8"), originalLock);
});

test("failed local SDK installation still restores the exact committed lock", async (t) => {
  const fixture = await makeFixture(t, "1.2.3", "sha512-different");
  const lockPath = path.join(fixture.root, "crates/mcp/package-lock.json");
  const originalLock = await readFile(lockPath, "utf8");
  const result = runFixture(fixture, ["--sdk-ready"], { NPM_TEST_FAIL_INSTALL: "1" });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /failed with exit code 42/);
  assert.match(await readFile(fixture.log, "utf8"), /installed-integrity=sha512-fixture/);
  assert.equal(await readFile(lockPath, "utf8"), originalLock);
});

test("strict published policy accepts a matching SDK archive", async (t) => {
  const fixture = await makeFixture(t, "1.2.3");
  const result = runFixture(fixture, ["--sdk-ready"], { MINUTES_MCP_REQUIRE_PUBLISHED_SDK: "1" });
  assert.equal(result.status, 0, result.stderr);
  assert.match(await readFile(fixture.log, "utf8"), /installed-integrity=sha512-fixture/);
});

test("release bundle installs the SDK from the exact checkout", async () => {
  const workflow = await readFile(path.join(repoRoot, ".github", "workflows", "release-cli.yml"), "utf8");
  const bundleJob = workflow.match(/\n  mcpb:\n[\s\S]*?\n  checksums:/)?.[0];
  assert.ok(bundleJob, "release workflow must contain the mcpb job");
  assert.match(bundleJob, /node scripts\/install_mcp_dependencies\.mjs/);
  assert.doesNotMatch(bundleJob, /working-directory: crates\/mcp\n\s+run: \|\n\s+npm ci/);
});
