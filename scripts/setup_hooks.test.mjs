import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const scriptsDirectory = path.dirname(fileURLToPath(import.meta.url));
const sourceRoot = path.resolve(scriptsDirectory, "..");
const setupScript = path.join(sourceRoot, "scripts/setup-hooks.sh");
const prePushHook = path.join(sourceRoot, ".githooks/pre-push");
const versionChecker = path.join(sourceRoot, "scripts/check_version_sync.mjs");

const mainVersion = "1.2.3";
const pluginVersion = "0.9.0";

async function writeFixture(root, file, contents) {
  const destination = path.join(root, file);
  await mkdir(path.dirname(destination), { recursive: true });
  await writeFile(destination, contents);
}

async function writeJson(root, file, value) {
  await writeFixture(root, file, `${JSON.stringify(value, null, 2)}\n`);
}

function fixtureEnv(root) {
  return {
    ...process.env,
    GIT_CONFIG_GLOBAL: path.join(root, "global.gitconfig"),
    GIT_CONFIG_NOSYSTEM: "1",
  };
}

function run(root, command, args = []) {
  return spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    env: fixtureEnv(root),
  });
}

async function makeGitRepo(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), "minutes-setup-hooks-"));
  t.after(() => rm(root, { recursive: true, force: true }));

  const init = run(root, "git", ["init", "--quiet"]);
  assert.equal(init.status, 0, init.stderr);

  await mkdir(path.join(root, "scripts"), { recursive: true });
  await copyFile(setupScript, path.join(root, "scripts/setup-hooks.sh"));
  await chmod(path.join(root, "scripts/setup-hooks.sh"), 0o755);

  return root;
}

function runSetup(root, ...args) {
  return run(root, path.join(root, "scripts/setup-hooks.sh"), args);
}

function getHooksPath(root) {
  return run(root, "git", ["config", "--local", "--get", "core.hooksPath"]);
}

test("setup-hooks sets core.hooksPath in an unconfigured repository", async (t) => {
  const root = await makeGitRepo(t);
  const result = runSetup(root);

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /set core\.hooksPath to '\.githooks'/);

  const configured = getHooksPath(root);
  assert.equal(configured.status, 0, configured.stderr);
  assert.equal(configured.stdout.trim(), ".githooks");
});

test("setup-hooks refuses to replace a different core.hooksPath", async (t) => {
  const root = await makeGitRepo(t);
  const configure = run(root, "git", ["config", "--local", "core.hooksPath", "custom-hooks"]);
  assert.equal(configure.status, 0, configure.stderr);

  const result = runSetup(root);

  assert.equal(result.status, 1, result.stderr || result.stdout);
  assert.match(result.stderr, /refusing to replace existing core\.hooksPath 'custom-hooks'/);
  assert.match(result.stderr, /--force/);
  assert.equal(getHooksPath(root).stdout.trim(), "custom-hooks");
});

test("setup-hooks --force replaces a different core.hooksPath", async (t) => {
  const root = await makeGitRepo(t);
  const configure = run(root, "git", ["config", "--local", "core.hooksPath", "custom-hooks"]);
  assert.equal(configure.status, 0, configure.stderr);

  const result = runSetup(root, "--force");

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /replaced core\.hooksPath 'custom-hooks' with '\.githooks'/);
  assert.equal(getHooksPath(root).stdout.trim(), ".githooks");
});

async function makeVersionRepo(t) {
  const root = await makeGitRepo(t);

  await writeFixture(
    root,
    "Cargo.toml",
    `[workspace]\nmembers = []\n\n[workspace.package]\nversion = "${mainVersion}"\nedition = "2021"\n`,
  );
  await writeFixture(
    root,
    "crates/cli/Cargo.toml",
    `[package]\nname = "minutes-cli"\nversion.workspace = true\n\n[dependencies]\nminutes-core = { path = "../core", version = "${mainVersion}", default-features = false }\n`,
  );
  await writeJson(root, "tauri/src-tauri/tauri.conf.json", { version: mainVersion });
  await writeJson(root, "crates/mcp/package.json", {
    name: "minutes-mcp",
    version: mainVersion,
    dependencies: { "minutes-sdk": mainVersion },
  });
  await writeJson(root, "crates/sdk/package.json", {
    name: "minutes-sdk",
    version: mainVersion,
  });
  await writeJson(root, "manifest.json", { version: mainVersion });
  await writeJson(root, "manifest.mcpb.json", { version: mainVersion });
  await writeFixture(
    root,
    "crates/mcp/src/index.ts",
    `const MCP_SERVER_VERSION = "${mainVersion}";\n`,
  );
  for (const file of ["crates/mcp/package-lock.json", "crates/sdk/package-lock.json"]) {
    await writeJson(root, file, {
      version: mainVersion,
      lockfileVersion: 3,
      packages: { "": { version: mainVersion } },
    });
  }
  await writeFixture(
    root,
    "Cargo.lock",
    ["minutes-core", "minutes-cli", "minutes-reader"]
      .map((name) => `[[package]]\nname = "${name}"\nversion = "${mainVersion}"\n`)
      .join("\n"),
  );
  await writeJson(root, ".claude-plugin/marketplace.json", {
    plugins: [{ name: "minutes", version: pluginVersion }],
  });
  await writeJson(root, ".claude/plugins/minutes/plugin.json", { version: pluginVersion });
  await writeJson(root, ".claude/plugins/minutes/.claude-plugin/plugin.json", {
    version: pluginVersion,
  });

  await mkdir(path.join(root, ".githooks"), { recursive: true });
  await copyFile(prePushHook, path.join(root, ".githooks/pre-push"));
  await chmod(path.join(root, ".githooks/pre-push"), 0o755);
  await copyFile(versionChecker, path.join(root, "scripts/check_version_sync.mjs"));

  return root;
}

function runPrePush(root) {
  return run(root, path.join(root, ".githooks/pre-push"));
}

test("pre-push passes and prints the skills skip path when build artifacts are absent", async (t) => {
  const root = await makeVersionRepo(t);
  const result = runPrePush(root);

  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.equal(result.stderr, "");
  assert.match(result.stdout, /Version sync check passed\./);
  assert.equal(
    result.stdout
      .split(/\r?\n/)
      .filter(
        (line) =>
          line ===
          "skipping skills check (enable: cd tooling/skills && npm ci && npm run build)",
      ).length,
    1,
  );
});

test("pre-push exits non-zero when the version sync check fails", async (t) => {
  const root = await makeVersionRepo(t);
  const manifest = JSON.parse(await readFile(path.join(root, "manifest.json"), "utf8"));
  manifest.version = "8.8.8";
  await writeJson(root, "manifest.json", manifest);

  const result = runPrePush(root);

  assert.equal(result.status, 1, result.stderr || result.stdout);
  assert.match(result.stderr, /Version sync check failed\./);
  assert.match(result.stderr, /manifest\.json \[\.version\]/);
});
