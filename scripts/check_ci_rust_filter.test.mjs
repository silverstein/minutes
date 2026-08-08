import { execFileSync } from "child_process";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync, mkdirSync } from "fs";
import { tmpdir } from "os";
import { dirname, join } from "path";
import { fileURLToPath } from "url";
import test from "node:test";
import assert from "node:assert/strict";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const script = join(repoRoot, "scripts/check_ci_rust_filter.mjs");

/**
 * Run the guard against a throwaway repo whose Cargo.toml and ci.yml we control,
 * so a test can make the filter stale without touching the real ones.
 */
function runAgainst({ members, patterns }) {
  const dir = mkdtempSync(join(tmpdir(), "rust-filter-"));
  try {
    mkdirSync(join(dir, ".github/workflows"), { recursive: true });
    mkdirSync(join(dir, "scripts"), { recursive: true });
    cpSync(script, join(dir, "scripts/check_ci_rust_filter.mjs"));

    const memberList = members.map((m) => `    "${m}",`).join("\n");
    writeFileSync(
      join(dir, "Cargo.toml"),
      `[workspace]\nresolver = "2"\nmembers = [\n${memberList}\n]\n`
    );

    const patternList = patterns.map((p) => `              - '${p}'`).join("\n");
    writeFileSync(
      join(dir, ".github/workflows/ci.yml"),
      [
        "jobs:",
        "  changes:",
        "    steps:",
        "      - uses: dorny/paths-filter@v4",
        "        with:",
        "          filters: |",
        "            rust:",
        patternList,
        "            other:",
        "              - 'site/**'",
        "",
      ].join("\n")
    );

    try {
      const stdout = execFileSync(process.execPath, [join(dir, "scripts/check_ci_rust_filter.mjs")], {
        cwd: dir,
        encoding: "utf8",
      });
      return { code: 0, output: stdout };
    } catch (error) {
      return { code: error.status, output: `${error.stdout || ""}${error.stderr || ""}` };
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("passes when every member is covered", () => {
  const result = runAgainst({
    members: ["crates/core", "crates/cli", "tauri/src-tauri"],
    patterns: ["crates/core/**", "crates/cli/**", "tauri/**"],
  });
  assert.equal(result.code, 0, result.output);
  assert.match(result.output, /covers all 3 workspace members/);
});

test("fails and names the member when the filter goes stale", () => {
  const result = runAgainst({
    members: ["crates/core", "crates/cli", "crates/whisper-guard"],
    patterns: ["crates/core/**", "crates/cli/**"],
  });
  assert.equal(result.code, 1);
  assert.match(result.output, /uncovered: crates\/whisper-guard/);
  assert.match(result.output, /- 'crates\/whisper-guard\/\*\*'/);
});

test("a parent glob covers nested members", () => {
  const result = runAgainst({
    members: ["archive/src-tauri"],
    patterns: ["archive/**"],
  });
  assert.equal(result.code, 0, result.output);
});

test("a negated pattern never counts as coverage", () => {
  // The action compiles each pattern separately, so '!crates/mcp/**' matches
  // every file outside that subtree rather than excluding it. Treating one as
  // coverage would hide exactly the gap this guard exists to catch.
  const result = runAgainst({
    members: ["crates/core"],
    patterns: ["!crates/mcp/**"],
  });
  assert.equal(result.code, 1);
  assert.match(result.output, /uncovered: crates\/core/);
});

test("the real repo satisfies the guard", () => {
  const output = execFileSync(process.execPath, [script], {
    cwd: repoRoot,
    encoding: "utf8",
  });
  assert.match(output, /covers all \d+ workspace members/);
});

test("the real filter does not list the npm packages", () => {
  const ci = readFileSync(join(repoRoot, ".github/workflows/ci.yml"), "utf8");
  const rustBlock = ci.slice(ci.indexOf("rust:"), ci.indexOf("  test:"));
  // crates/mcp and crates/sdk have no Cargo.toml and no .rs files; a blanket
  // 'crates/**' made every TypeScript PR run the full Rust matrix.
  assert.ok(
    !/^\s*-\s*'crates\/\*\*'\s*$/m.test(rustBlock),
    "ci.yml rust filter should enumerate Rust crates, not use a blanket crates/** glob"
  );
});
