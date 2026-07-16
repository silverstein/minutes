import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

const checker = path.resolve(path.dirname(process.argv[1]), "check_version_sync.mjs");
const mainVersion = "1.2.3";
const changedVersion = "8.8.8";
const pluginVersion = "0.9.0";

async function writeFixture(root, file, contents) {
  const destination = path.join(root, file);
  await mkdir(path.dirname(destination), { recursive: true });
  await writeFile(destination, contents);
}

async function writeJson(root, file, value) {
  await writeFixture(root, file, `${JSON.stringify(value, null, 2)}\n`);
}

async function makeRepo(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), "minutes-version-sync-"));
  t.after(() => rm(root, { recursive: true, force: true }));

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
  await writeJson(root, "crates/sdk/package.json", { name: "minutes-sdk", version: mainVersion });
  await writeJson(root, "manifest.json", { version: mainVersion });
  await writeJson(root, "manifest.mcpb.json", { version: mainVersion });
  await writeFixture(
    root,
    "crates/mcp/src/index.ts",
    `const MCP_SERVER_VERSION = "${mainVersion}";\n`,
  );
  await writeJson(root, "crates/mcp/package-lock.json", {
    name: "minutes-mcp",
    version: mainVersion,
    lockfileVersion: 3,
    packages: { "": { name: "minutes-mcp", version: mainVersion } },
  });
  await writeJson(root, "crates/sdk/package-lock.json", {
    name: "minutes-sdk",
    version: mainVersion,
    lockfileVersion: 3,
    packages: { "": { name: "minutes-sdk", version: mainVersion } },
  });
  await writeFixture(
    root,
    "Cargo.lock",
    ["minutes-core", "minutes-cli", "minutes-reader", "whisper-guard"]
      .map(
        (name) =>
          `[[package]]\nname = "${name}"\nversion = "${name === "whisper-guard" ? "77.0.0" : mainVersion}"\n`,
      )
      .join("\n"),
  );
  await writeJson(root, ".claude-plugin/marketplace.json", {
    plugins: [{ name: "minutes", version: pluginVersion }],
  });
  await writeJson(root, ".claude/plugins/minutes/plugin.json", { version: pluginVersion });
  await writeJson(root, ".claude/plugins/minutes/.claude-plugin/plugin.json", {
    version: pluginVersion,
  });
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

  return root;
}

function runChecker(root, ...args) {
  return spawnSync(process.execPath, [checker, "--root", root, ...args], {
    encoding: "utf8",
  });
}

async function replaceInFile(root, file, before, after) {
  const target = path.join(root, file);
  const contents = await readFile(target, "utf8");
  assert.ok(contents.includes(before), `${file} contains mutation target`);
  await writeFile(target, contents.replace(before, after));
}

async function mutateJson(root, file, mutate) {
  const target = path.join(root, file);
  const value = JSON.parse(await readFile(target, "utf8"));
  mutate(value);
  await writeFile(target, `${JSON.stringify(value, null, 2)}\n`);
}

function parseReport(result) {
  assert.equal(result.stderr, "");
  return JSON.parse(result.stdout);
}

test("matching miniature repo passes with independent plugin and ignored versions", async (t) => {
  const root = await makeRepo(t);
  const result = runChecker(root, "--json");

  assert.equal(result.status, 0, result.stderr || result.stdout);
  const report = parseReport(result);
  assert.equal(report.ok, true);
  assert.equal(report.domain1.expected, mainVersion);
  assert.equal(report.domain2.expected, pluginVersion);
  assert.equal(report.domain1.sources.length, 15);
  assert.equal(report.domain2.sources.length, 3);
});

const domain1Mutations = [
  {
    name: "workspace package version",
    file: "Cargo.toml",
    key: "[workspace.package].version",
    mutate: (root) =>
      replaceInFile(root, "Cargo.toml", `version = "${mainVersion}"`, `version = "${changedVersion}"`),
  },
  {
    name: "CLI minutes-core dependency version",
    file: "crates/cli/Cargo.toml",
    key: "dependencies.minutes-core.version",
    mutate: (root) =>
      replaceInFile(
        root,
        "crates/cli/Cargo.toml",
        `version = "${mainVersion}"`,
        `version = "${changedVersion}"`,
      ),
  },
  ...[
    "tauri/src-tauri/tauri.conf.json",
    "crates/mcp/package.json",
    "crates/sdk/package.json",
    "manifest.json",
    "manifest.mcpb.json",
  ].map((file) => ({
    name: `${file} version`,
    file,
    key: ".version",
    mutate: (root) => mutateJson(root, file, (value) => (value.version = changedVersion)),
  })),
  {
    name: "MCP server constant",
    file: "crates/mcp/src/index.ts",
    key: "MCP_SERVER_VERSION",
    mutate: (root) => replaceInFile(root, "crates/mcp/src/index.ts", mainVersion, changedVersion),
  },
  ...["crates/mcp/package-lock.json", "crates/sdk/package-lock.json"].flatMap((file) => [
    {
      name: `${file} top-level version`,
      file,
      key: ".version",
      mutate: (root) => mutateJson(root, file, (value) => (value.version = changedVersion)),
    },
    {
      name: `${file} root package version`,
      file,
      key: '.packages[""].version',
      mutate: (root) =>
        mutateJson(root, file, (value) => (value.packages[""].version = changedVersion)),
    },
  ]),
  ...["minutes-core", "minutes-cli", "minutes-reader"].map((packageName) => ({
    name: `Cargo.lock ${packageName} package version`,
    file: "Cargo.lock",
    key: `package[${packageName}].version`,
    mutate: (root) =>
      replaceInFile(
        root,
        "Cargo.lock",
        `name = "${packageName}"\nversion = "${mainVersion}"`,
        `name = "${packageName}"\nversion = "${changedVersion}"`,
      ),
  })),
];

for (const fixture of domain1Mutations) {
  test(`Domain 1 detects ${fixture.name}`, async (t) => {
    const root = await makeRepo(t);
    await fixture.mutate(root);

    const result = runChecker(root, "--json");
    assert.equal(result.status, 1, result.stderr || result.stdout);
    const report = parseReport(result);
    assert.equal(report.ok, false);
    assert.equal(report.domain1.expected, mainVersion);
    assert.ok(
      report.domain1.sources.some(
        (source) => source.file === fixture.file && source.key === fixture.key && source.ok === false,
      ),
      `${fixture.file} [${fixture.key}] is reported as mismatched`,
    );
    assert.equal(report.domain2.sources.every((source) => source.ok), true);
  });
}

test("Domain 2 detects one mutated plugin version and names its file", async (t) => {
  const root = await makeRepo(t);
  const file = ".claude/plugins/minutes/.claude-plugin/plugin.json";
  await mutateJson(root, file, (value) => (value.version = changedVersion));

  const result = runChecker(root, "--json");
  assert.equal(result.status, 1, result.stderr || result.stdout);
  const report = parseReport(result);
  assert.equal(report.domain1.sources.every((source) => source.ok), true);
  assert.ok(report.domain2.sources.some((source) => source.file === file && source.ok === false));
});

test("whisper-guard versions are explicitly ignored", async (t) => {
  const root = await makeRepo(t);
  await replaceInFile(root, "crates/whisper-guard/Cargo.toml", "77.0.0", "9999.42.7");
  await replaceInFile(
    root,
    "Cargo.lock",
    'name = "whisper-guard"\nversion = "77.0.0"',
    'name = "whisper-guard"\nversion = "1234.5.6"',
  );

  const result = runChecker(root, "--json");
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.equal(parseReport(result).ok, true);
});

test("missing MCP_SERVER_VERSION declaration fails with a clear message", async (t) => {
  const root = await makeRepo(t);
  await writeFixture(root, "crates/mcp/src/index.ts", "export const unrelated = true;\n");

  const result = runChecker(root);
  assert.equal(result.status, 1, result.stderr || result.stdout);
  assert.match(result.stderr, /crates\/mcp\/src\/index\.ts \[MCP_SERVER_VERSION\]/);
  assert.match(result.stderr, /MCP_SERVER_VERSION.*not found/);
});

test("release mode rejects minutes-sdk ranges while default mode ignores them", async (t) => {
  for (const rangePrefix of ["^", "~"]) {
    const root = await makeRepo(t);
    const dependency = `${rangePrefix}${mainVersion}`;
    await mutateJson(
      root,
      "crates/mcp/package.json",
      (value) => (value.dependencies["minutes-sdk"] = dependency),
    );

    const defaultResult = runChecker(root, "--json");
    assert.equal(defaultResult.status, 0, defaultResult.stderr || defaultResult.stdout);
    assert.equal(Object.hasOwn(parseReport(defaultResult), "release"), false);

    const releaseResult = runChecker(root, "--release", "--json");
    assert.equal(releaseResult.status, 1, releaseResult.stderr || releaseResult.stdout);
    const report = parseReport(releaseResult);
    assert.equal(report.ok, false);
    assert.equal(report.release.expected, mainVersion);
    assert.deepEqual(report.release.sources, [
      {
        file: "crates/mcp/package.json",
        key: '.dependencies["minutes-sdk"]',
        value: dependency,
        ok: false,
      },
    ]);
  }
});

test("release mode rejects an exact minutes-sdk version that does not match the SDK", async (t) => {
  const root = await makeRepo(t);
  await mutateJson(
    root,
    "crates/mcp/package.json",
    (value) => (value.dependencies["minutes-sdk"] = changedVersion),
  );

  const result = runChecker(root, "--release", "--json");
  assert.equal(result.status, 1, result.stderr || result.stdout);
  const report = parseReport(result);
  assert.equal(report.release.expected, mainVersion);
  assert.equal(report.release.sources[0].value, changedVersion);
  assert.equal(report.release.sources[0].ok, false);
});

test("release mode accepts an exact minutes-sdk version equal to the SDK", async (t) => {
  const root = await makeRepo(t);

  const result = runChecker(root, "--release", "--json");
  assert.equal(result.status, 0, result.stderr || result.stdout);
  const report = parseReport(result);
  assert.equal(report.ok, true);
  assert.equal(report.release.expected, mainVersion);
  assert.equal(report.release.sources[0].value, mainVersion);
  assert.equal(report.release.sources[0].ok, true);
});

test("release mode rejects a file minutes-sdk dependency", async (t) => {
  const root = await makeRepo(t);
  await mutateJson(
    root,
    "crates/mcp/package.json",
    (value) => (value.dependencies["minutes-sdk"] = "file:../sdk"),
  );

  const result = runChecker(root, "--release", "--json");
  assert.equal(result.status, 1, result.stderr || result.stdout);
  const report = parseReport(result);
  assert.equal(report.release.sources[0].value, "file:../sdk");
  assert.equal(report.release.sources[0].ok, false);
});
