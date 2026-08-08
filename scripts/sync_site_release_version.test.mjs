import { spawnSync } from "child_process";
import { readFileSync, writeFileSync } from "fs";
import { dirname, join } from "path";
import { fileURLToPath } from "url";
import test from "node:test";
import assert from "node:assert/strict";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const script = join(repoRoot, "scripts/sync_site_release_version.mjs");
const releaseTs = join(repoRoot, "site/lib/release.ts");

function run(args) {
  // spawnSync, not execFileSync: the tolerated-drift path exits 0 and writes
  // its warning to stderr, which execFileSync discards on success.
  const result = spawnSync(process.execPath, [script, ...args], {
    cwd: repoRoot,
    encoding: "utf8",
  });
  return { code: result.status, output: `${result.stdout || ""}${result.stderr || ""}` };
}

/**
 * Run `body` with release.ts temporarily edited, always restoring it.
 *
 * `edit` must actually change the file. A regex that quietly fails to match
 * leaves the tree clean and the assertion then passes for the wrong reason,
 * which is how the first version of these tests reported a false result.
 */
function withEditedReleaseTs(edit, body) {
  const original = readFileSync(releaseTs, "utf8");
  const edited = edit(original);
  assert.notEqual(edited, original, "test edit did not change release.ts");
  try {
    writeFileSync(releaseTs, edited);
    return body();
  } finally {
    writeFileSync(releaseTs, original);
  }
}

test("--check tolerates a stale test count", () => {
  const result = withEditedReleaseTs(
    (s) => s.replace(/MINUTES_TEST_COUNT = \d+/, "MINUTES_TEST_COUNT = 1"),
    () => run(["--check"]),
  );
  // A count that moves whenever anyone adds a test must not redden shared CI
  // and every open PR with it (#664, #666).
  assert.equal(result.code, 0, result.output);
  assert.match(result.output, /except the test count/);
});

test("--check-release does not tolerate a stale test count", () => {
  const result = withEditedReleaseTs(
    (s) => s.replace(/MINUTES_TEST_COUNT = \d+/, "MINUTES_TEST_COUNT = 1"),
    () => run(["--check-release"]),
  );
  // Nothing refreshes the count automatically, so without a binding gate
  // somewhere it drifts indefinitely. Tag time is where it gets published.
  assert.equal(result.code, 1);
  assert.match(result.output, /out of sync/);
});

test("--check still fails when a release link constant drifts", () => {
  const result = withEditedReleaseTs(
    (s) => s.replace(/MINUTES_MCP_TOOL_COUNT = \d+/, "MINUTES_MCP_TOOL_COUNT = 999"),
    () => run(["--check"]),
  );
  assert.equal(result.code, 1);
  assert.match(result.output, /out of sync/);
});

test("--check fails when the version and the test count both drift", () => {
  // The tolerance keys off "everything except the count is identical", so a
  // combined change must not slip through on the count exemption.
  const result = withEditedReleaseTs(
    (s) =>
      s
        .replace(/MINUTES_TEST_COUNT = \d+/, "MINUTES_TEST_COUNT = 1")
        .replace(/MINUTES_RELEASE_VERSION = "[^"]+"/, 'MINUTES_RELEASE_VERSION = "0.0.1"'),
    () => run(["--check"]),
  );
  assert.equal(result.code, 1);
  assert.match(result.output, /out of sync/);
});

test("both modes pass on a clean tree", () => {
  assert.equal(run(["--check"]).code, 0);
  assert.equal(run(["--check-release"]).code, 0);
});
