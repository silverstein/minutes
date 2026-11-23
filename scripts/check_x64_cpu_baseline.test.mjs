import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import assert from "node:assert/strict";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const exporterPath = ".github/scripts/export-x64-cpu-baseline.sh";
const cliWorkflow = "release-cli.yml";
const standaloneExporter = exporterPath + ' "${{ runner.os }}"';
const common = [
  "GGML_NATIVE=OFF",
  "GGML_AVX=ON",
  "GGML_AVX2=ON",
  "GGML_AVX512=OFF",
  "GGML_AVX512_VBMI=OFF",
  "GGML_AVX512_VNNI=OFF",
  "GGML_AVX512_BF16=OFF",
];

for (const [platform, options] of [
  ["Linux", [
    "GGML_SSE42=ON",
    "GGML_BMI2=ON",
    "GGML_FMA=ON",
    "GGML_F16C=ON",
    "GGML_AVX_VNNI=OFF",
    "GGML_AMX_TILE=OFF",
    "GGML_AMX_INT8=OFF",
    "GGML_AMX_BF16=OFF",
  ]],
  ["Windows", [
    // Restore MSVC release optimization lost through the cmake crate (#1256).
    "CMAKE_C_FLAGS_RELEASE=/MD /O2 /Ob2 /DNDEBUG",
    "CMAKE_CXX_FLAGS_RELEASE=/MD /O2 /Ob2 /DNDEBUG",
  ]],
]) {
  test("the exporter writes the portable " + platform + " Whisper CPU contract", () => {
    const dir = mkdtempSync(join(tmpdir(), "minutes-x64-cpu-"));
    const githubEnv = join(dir, "github-env");
    try {
      writeFileSync(githubEnv, "");
      execFileSync("bash", [join(repoRoot, exporterPath), platform], {
        cwd: repoRoot,
        env: { ...process.env, GITHUB_ENV: githubEnv },
        stdio: "pipe",
      });
      const actual = readFileSync(githubEnv, "utf8").trim().split("\n");
      assert.deepEqual(actual, [...common, ...options]);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
}

function readWorkflow(name) {
  return readFileSync(join(repoRoot, ".github/workflows", name), "utf8");
}

for (const [workflow, exportCommand, buildNeedles] of [
  [
    cliWorkflow,
    standaloneExporter,
    [
      "name: Build release binary",
      "name: Build sherpa-enabled Linux archive",
    ],
  ],
  ["release-windows-desktop.yml", exporterPath + " Windows", ["cargo tauri build --features parakeet"]],
  ["ci.yml", exporterPath + " Windows", ["cargo tauri build --ci --bundles nsis"]],
]) {
  test(workflow + " exports the baseline before its x64 builds", () => {
    const contents = readWorkflow(workflow);
    const exportAt = contents.indexOf(exportCommand);
    assert.notEqual(exportAt, -1, workflow + " must invoke the shared exporter");
    for (const buildNeedle of buildNeedles) {
      const buildAt = contents.indexOf(buildNeedle);
      assert.notEqual(buildAt, -1, workflow + " build command changed; update this guard deliberately");
      assert.ok(exportAt < buildAt, workflow + " must export the CPU baseline before building");
    }
  });
}

test("the standalone release applies the baseline to shipped Linux and Windows x64 targets", () => {
  const step = [
    "name: Configure portable x64 Whisper CPU baseline",
    "if: runner.os == 'Windows' || matrix.target == 'x86_64-unknown-linux-gnu'",
    "shell: bash",
    "run: " + standaloneExporter,
  ].join("\n        ");
  assert.ok(readWorkflow(cliWorkflow).includes(step));
});

test("the exporter is included in the standalone release cache key", () => {
  assert.match(
    readWorkflow(cliWorkflow),
    /key: [^\n]*hashFiles\('\*\*\/Cargo\.lock', '\.github\/scripts\/export-x64-cpu-baseline\.sh'\)/
  );
});

test("changing the shared baseline triggers the Windows CI artifact", () => {
  assert.match(readWorkflow("ci.yml"), /- '\.github\/scripts\/export-x64-cpu-baseline\.sh'/);
});
