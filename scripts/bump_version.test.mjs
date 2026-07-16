import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  appendFile,
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

const scriptsDirectory = path.dirname(process.argv[1]);
const bumper = path.resolve(scriptsDirectory, "bump-version.mjs");
const checker = path.resolve(scriptsDirectory, "check_version_sync.mjs");
const initialVersion = "1.2.3";
const nextVersion = "1.3.0-beta.1";
const initialPluginVersion = "0.9.0";
const nextPluginVersion = "0.10.0";

async function writeFixture(root, file, contents) {
  const destination = path.join(root, file);
  await mkdir(path.dirname(destination), { recursive: true });
  await writeFile(destination, contents);
}

async function writeJson(root, file, value) {
  await writeFixture(root, file, `${JSON.stringify(value, null, 2)}\n`);
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: "utf8", ...options });
  assert.equal(result.error, undefined, result.error?.message);
  return result;
}

function git(root, ...args) {
  return run("git", args, { cwd: root });
}

async function installToolShims(root) {
  const tools = path.join(root, ".test-tools");
  await mkdir(tools, { recursive: true });

  await writeFixture(
    tools,
    "npm",
    `#!/usr/bin/env node
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
if (process.env.BUMP_SHIM_FAIL === "npm") {
  console.error("simulated npm failure");
  process.exit(51);
}
if (process.env.BUMP_SHIM_PAUSE === "npm") {
  const worktree = path.resolve(process.cwd(), "../..");
  await writeFile(process.env.BUMP_SHIM_READY_FILE, worktree);
  await new Promise((resolve) => setTimeout(resolve, 60000));
}
const packageJson = JSON.parse(await readFile(path.join(process.cwd(), "package.json"), "utf8"));
const lockPath = path.join(process.cwd(), "package-lock.json");
const lock = JSON.parse(await readFile(lockPath, "utf8"));
lock.version = packageJson.version;
lock.packages[""].version = packageJson.version;
await writeFile(lockPath, JSON.stringify(lock, null, 2) + "\\n");
`,
  );
  await writeFixture(
    tools,
    "cargo",
    `#!/usr/bin/env node
import { readFile, writeFile } from "node:fs/promises";
if (process.env.BUMP_SHIM_FAIL === "cargo") {
  console.error("simulated cargo failure");
  process.exit(52);
}
const manifest = await readFile("Cargo.toml", "utf8");
const version = /^version\\s*=\\s*"([^"]+)"/m.exec(manifest)?.[1];
if (!version) throw new Error("fixture workspace version not found");
const lockPath = "Cargo.lock";
let lock = await readFile(lockPath, "utf8");
for (const name of ["minutes-core", "minutes-cli", "minutes-reader"]) {
  const pattern = new RegExp('(name = "' + name + '"\\nversion = ")[^"]+(")');
  if (!pattern.test(lock)) throw new Error("fixture lock package not found: " + name);
  lock = lock.replace(pattern, (_match, prefix, suffix = "") => prefix + version + suffix);
}
await writeFile(lockPath, lock);
`,
  );
  await chmod(path.join(tools, "npm"), 0o755);
  await chmod(path.join(tools, "cargo"), 0o755);
  return tools;
}

async function makeRepo(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), "minutes-bump-fixture-"));
  t.after(() => rm(root, { recursive: true, force: true }));

  await writeFixture(
    root,
    "Cargo.toml",
    `[workspace]\nmembers = []\n\n[workspace.package]\nversion = "${initialVersion}"\nedition = "2021"\n`,
  );
  await writeFixture(
    root,
    "crates/cli/Cargo.toml",
    `[package]\nname = "minutes-cli"\nversion.workspace = true\n\n[dependencies]\nminutes-core = { path = "../core", version = "${initialVersion}", default-features = false }\n`,
  );
  await writeJson(root, "tauri/src-tauri/tauri.conf.json", { version: initialVersion });
  await writeJson(root, "crates/mcp/package.json", {
    name: "minutes-mcp",
    version: initialVersion,
  });
  await writeJson(root, "crates/sdk/package.json", {
    name: "minutes-sdk",
    version: initialVersion,
  });
  await writeJson(root, "manifest.json", { version: initialVersion, tools: [] });
  await writeJson(root, "manifest.mcpb.json", { version: initialVersion });
  await writeFixture(
    root,
    "crates/mcp/src/index.ts",
    `const MCP_SERVER_VERSION = "${initialVersion}";\n`,
  );
  for (const [directory, name] of [
    ["mcp", "minutes-mcp"],
    ["sdk", "minutes-sdk"],
  ]) {
    await writeJson(root, `crates/${directory}/package-lock.json`, {
      name,
      version: initialVersion,
      lockfileVersion: 3,
      packages: { "": { name, version: initialVersion } },
    });
  }
  await writeFixture(
    root,
    "Cargo.lock",
    ["minutes-core", "minutes-cli", "minutes-reader", "whisper-guard"]
      .map(
        (name) =>
          `[[package]]\nname = "${name}"\nversion = "${name === "whisper-guard" ? "77.0.0" : initialVersion}"\n`,
      )
      .join("\n"),
  );
  await writeFixture(
    root,
    "crates/whisper-guard/Cargo.toml",
    `[package]\nname = "whisper-guard"\nversion = "77.0.0"\n`,
  );
  await writeFixture(
    root,
    "tauri/src-tauri/Cargo.toml",
    `[package]\nname = "minutes-app"\nversion = "0.1.0"\n`,
  );
  await writeJson(root, ".claude-plugin/marketplace.json", {
    plugins: [{ name: "minutes", version: initialPluginVersion }],
  });
  await writeJson(root, ".claude/plugins/minutes/plugin.json", {
    version: initialPluginVersion,
  });
  await writeJson(root, ".claude/plugins/minutes/.claude-plugin/plugin.json", {
    version: initialPluginVersion,
  });
  await writeFixture(root, "site/lib/release.ts", `export const VERSION = "${initialVersion}";\n`);
  await writeFixture(root, "scripts/check_version_sync.mjs", await readFile(checker, "utf8"));
  await writeFixture(
    root,
    "scripts/sync_site_release_version.mjs",
    `#!/usr/bin/env node
import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifest = JSON.parse(await readFile(path.join(root, "manifest.json"), "utf8"));
await writeFile(path.join(root, "site/lib/release.ts"), 'export const VERSION = "' + manifest.version + '";\\n');
`,
  );
  const tools = await installToolShims(root);

  assert.equal(git(root, "init", "-q").status, 0);
  assert.equal(git(root, "config", "user.email", "fixture@example.com").status, 0);
  assert.equal(git(root, "config", "user.name", "Fixture Test").status, 0);
  assert.equal(git(root, "add", ".").status, 0);
  const commit = git(root, "commit", "-qm", "fixture");
  assert.equal(commit.status, 0, commit.stderr);
  return { root, tools };
}

function runBump(root, tools, ...args) {
  return run(process.execPath, [bumper, ...args], {
    cwd: root,
    env: {
      ...process.env,
      BUMP_TOOL_PATH: tools,
    },
  });
}

function runChecker(root) {
  return run(process.execPath, [checker, "--root", root], { cwd: root });
}

function status(root) {
  return git(root, "status", "--porcelain=v1", "--untracked-files=all").stdout;
}

async function trackedSnapshot(root) {
  const files = git(root, "ls-files", "-z").stdout.split("\0").filter(Boolean);
  return new Map(
    await Promise.all(
      files.map(async (file) => [file, (await readFile(path.join(root, file))).toString("base64")]),
    ),
  );
}

async function assertSnapshot(root, expected) {
  assert.deepEqual(await trackedSnapshot(root), expected);
}

async function readVersion(root, file) {
  return (await readFile(path.join(root, file), "utf8")).match(/1\.3\.0-beta\.1/)?.[0];
}

test("dry-run prints its file plan and diff without touching the real tree", async (t) => {
  const { root, tools } = await makeRepo(t);
  const before = await trackedSnapshot(root);
  const result = runBump(root, tools, "--dry-run", nextVersion);

  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.match(result.stdout, /Dry run: main version 1\.2\.3 -> 1\.3\.0-beta\.1/);
  assert.match(result.stdout, /Files that would change:/);
  assert.match(result.stdout, /diff --git a\/Cargo\.toml b\/Cargo\.toml/);
  assert.equal(status(root), "");
  await assertSnapshot(root, before);
  assert.equal(git(root, "worktree", "list", "--porcelain").stdout.match(/^worktree /gm)?.length, 1);
});

test("real run updates every Domain-1 source and passes the checker", async (t) => {
  const { root, tools } = await makeRepo(t);
  const result = runBump(root, tools, nextVersion);

  assert.equal(result.status, 0, result.stderr || result.stdout);
  for (const file of [
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
    "site/lib/release.ts",
  ]) {
    assert.equal(await readVersion(root, file), nextVersion, `${file} contains the new version`);
  }
  const check = runChecker(root);
  assert.equal(check.status, 0, check.stderr || check.stdout);
  for (const pluginFile of [
    ".claude-plugin/marketplace.json",
    ".claude/plugins/minutes/plugin.json",
    ".claude/plugins/minutes/.claude-plugin/plugin.json",
  ]) {
    assert.doesNotMatch(await readFile(path.join(root, pluginFile), "utf8"), /1\.3\.0/);
  }
  assert.match(
    await readFile(path.join(root, "crates/whisper-guard/Cargo.toml"), "utf8"),
    /version = "77\.0\.0"/,
  );
  assert.match(
    await readFile(path.join(root, "tauri/src-tauri/Cargo.toml"), "utf8"),
    /version = "0\.1\.0"/,
  );
});

test("dirty tree (tracked modification) is refused", async (t) => {
  const { root, tools } = await makeRepo(t);
  await appendFile(path.join(root, "manifest.json"), "\n");
  const result = runBump(root, tools, nextVersion);

  assert.equal(result.status, 1);
  assert.match(result.stderr, /working tree is dirty/);
  assert.equal(await readVersion(root, "Cargo.toml"), undefined);
});

test("untracked files do not block a bump", async (t) => {
  const { root, tools } = await makeRepo(t);
  await writeFixture(root, "untracked.txt", "scratch\n");
  const result = runBump(root, tools, "--dry-run", nextVersion);

  assert.equal(result.status, 0);
});

test("invalid semantic versions are refused", async (t) => {
  const { root, tools } = await makeRepo(t);
  for (const invalid of ["1.2", "v1.2.3", "01.2.3", "1.2.3+build"]) {
    const result = runBump(root, tools, invalid);
    assert.equal(result.status, 2, `${invalid}: ${result.stderr}`);
    assert.match(result.stderr, /invalid version/);
  }
  assert.equal(status(root), "");
});

test("re-running the current version is an idempotent no-op", async (t) => {
  const { root, tools } = await makeRepo(t);
  const first = runBump(root, tools, nextVersion);
  assert.equal(first.status, 0, first.stderr || first.stdout);
  const before = await trackedSnapshot(root);

  const second = runBump(root, tools, nextVersion);
  assert.equal(second.status, 0, second.stderr || second.stdout);
  assert.match(second.stdout, /already at 1\.3\.0-beta\.1; nothing to do/);
  await assertSnapshot(root, before);
});

test("a committed partial bump is repaired, not treated as a no-op", async (t) => {
  const { root, tools } = await makeRepo(t);
  const first = runBump(root, tools, nextVersion);
  assert.equal(first.status, 0, first.stderr || first.stdout);

  // Regress one non-canonical source and commit it: canonical version still
  // matches the target, so a naive no-op check would accept the drift.
  const manifestPath = path.join(root, "manifest.json");
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  manifest.version = "0.0.1";
  await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  spawnSync("git", ["commit", "-am", "regress manifest"], { cwd: root });

  const repair = runBump(root, tools, nextVersion);
  assert.equal(repair.status, 0, repair.stderr || repair.stdout);
  assert.match(repair.stdout, /not synchronized; re-applying/);
  const repaired = JSON.parse(await readFile(manifestPath, "utf8"));
  assert.equal(repaired.version, nextVersion);
});

test("tool failure leaves the real tree byte-identical", async (t) => {
  const { root, tools } = await makeRepo(t);
  const before = await trackedSnapshot(root);
  const result = run(process.execPath, [bumper, nextVersion], {
    cwd: root,
    env: {
      ...process.env,
      BUMP_TOOL_PATH: tools,
      BUMP_SHIM_FAIL: "cargo",
    },
  });

  assert.equal(result.status, 1);
  assert.match(result.stderr, /simulated cargo failure/);
  assert.equal(status(root), "");
  await assertSnapshot(root, before);
  assert.equal(git(root, "worktree", "list", "--porcelain").stdout.match(/^worktree /gm)?.length, 1);
});

test(
  "SIGKILL after worktree creation leaves the real tree byte-identical",
  { skip: process.platform === "win32" },
  async (t) => {
    const { root, tools } = await makeRepo(t);
    const before = await trackedSnapshot(root);
    const markerDirectory = await mkdtemp(path.join(os.tmpdir(), "minutes-bump-marker-"));
    t.after(() => rm(markerDirectory, { recursive: true, force: true }));
    const marker = path.join(markerDirectory, "ready");

    const child = spawn(process.execPath, [bumper, nextVersion], {
      cwd: root,
      env: {
        ...process.env,
        BUMP_TOOL_PATH: tools,
        BUMP_SHIM_PAUSE: "npm",
        BUMP_SHIM_READY_FILE: marker,
      },
      detached: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    const closed = new Promise((resolve) =>
      child.once("close", (code, signal) => resolve({ code, signal })),
    );

    let ready = false;
    for (let attempt = 0; attempt < 250; attempt += 1) {
      try {
        await stat(marker);
        ready = true;
        break;
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
    }
    assert.equal(ready, true, "bump subprocess created its temporary worktree");
    process.kill(-child.pid, "SIGKILL");
    const outcome = await closed;
    assert.equal(outcome.signal, "SIGKILL");

    assert.equal(status(root), "");
    await assertSnapshot(root, before);

    const abandonedWorktree = await readFile(marker, "utf8");
    const remove = git(root, "worktree", "remove", "--force", abandonedWorktree);
    assert.equal(remove.status, 0, remove.stderr);
    await rm(path.dirname(abandonedWorktree), { recursive: true, force: true });
  },
);

test("plugin mode updates only the plugin trio", async (t) => {
  const { root, tools } = await makeRepo(t);
  const result = runBump(root, tools, "--plugin", nextPluginVersion);

  assert.equal(result.status, 0, result.stderr || result.stdout);
  const changed = git(root, "diff", "--name-only").stdout.trim().split("\n").sort();
  assert.deepEqual(changed, [
    ".claude-plugin/marketplace.json",
    ".claude/plugins/minutes/.claude-plugin/plugin.json",
    ".claude/plugins/minutes/plugin.json",
  ]);
  for (const file of changed) {
    assert.match(await readFile(path.join(root, file), "utf8"), /"version": "0\.10\.0"/);
  }
  assert.match(await readFile(path.join(root, "Cargo.toml"), "utf8"), /version = "1\.2\.3"/);
  const check = runChecker(root);
  assert.equal(check.status, 0, check.stderr || check.stdout);
});
