import { execFileSync } from "child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { dirname, join } from "path";
import { fileURLToPath } from "url";
import test from "node:test";
import assert from "node:assert/strict";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const exporter = join(repoRoot, ".github/scripts/export-windows-cpu-baseline.sh");
const expected = [
  "GGML_NATIVE=OFF",
  "GGML_AVX=ON",
  "GGML_AVX2=ON",
  "GGML_AVX512=OFF",
  "GGML_AVX512_VBMI=OFF",
  "GGML_AVX512_VNNI=OFF",
  "GGML_AVX512_BF16=OFF",
];

test("the shared exporter writes the portable Windows CPU contract", () => {
  const dir = mkdtempSync(join(tmpdir(), "minutes-windows-cpu-"));
  const githubEnv = join(dir, "github-env");
  try {
    writeFileSync(githubEnv, "");
    execFileSync("bash", [exporter], {
      cwd: repoRoot,
      env: { ...process.env, GITHUB_ENV: githubEnv },
      stdio: "pipe",
    });
    const actual = readFileSync(githubEnv, "utf8").trim().split("\n");
    assert.deepEqual(actual, expected);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

for (const [workflow, buildNeedle] of [
  ["release-cli.yml", "cargo build --release -p minutes-cli"],
  ["release-windows-desktop.yml", "cargo tauri build --features parakeet"],
  ["ci.yml", "cargo tauri build --ci --bundles nsis"],
]) {
  test(`${workflow} exports the baseline before its shipped Windows build`, () => {
    const contents = readFileSync(join(repoRoot, ".github/workflows", workflow), "utf8");
    const exportAt = contents.indexOf(".github/scripts/export-windows-cpu-baseline.sh");
    const buildAt = contents.indexOf(buildNeedle);
    assert.notEqual(exportAt, -1, `${workflow} must invoke the shared exporter`);
    assert.notEqual(buildAt, -1, `${workflow} build command changed; update this guard deliberately`);
    assert.ok(exportAt < buildAt, `${workflow} must export the CPU ceiling before building`);
  });
}

test("the standalone release applies the baseline only to Windows", () => {
  const contents = readFileSync(
    join(repoRoot, ".github/workflows/release-cli.yml"),
    "utf8"
  );
  assert.match(
    contents,
    /name: Configure portable Windows CPU baseline\n\s+if: runner\.os == 'Windows'\n\s+shell: bash\n\s+run: \.github\/scripts\/export-windows-cpu-baseline\.sh/
  );
});

test("changing the shared baseline triggers the Windows CI artifact", () => {
  const contents = readFileSync(join(repoRoot, ".github/workflows/ci.yml"), "utf8");
  assert.match(contents, /- '\.github\/scripts\/export-windows-cpu-baseline\.sh'/);
});
