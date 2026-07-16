import test from "node:test";
import assert from "node:assert/strict";
import { appendFile, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const COMPILE_CLI = fileURLToPath(new URL("./compile.js", import.meta.url));

function runCompile(repoRoot: string, dryRun: boolean) {
  return spawnSync(
    process.execPath,
    [
      COMPILE_CLI,
      "--root",
      repoRoot,
      ...(dryRun ? ["--dry-run"] : []),
      "--host",
      "claude",
    ],
    { encoding: "utf8" },
  );
}

async function createFixtureRepo(): Promise<string> {
  const repoRoot = await mkdtemp(path.join(tmpdir(), "minutes-compile-drift-"));
  const sourceDir = path.join(repoRoot, "tooling", "skills", "sources", "minutes-fixture");
  const pluginDir = path.join(repoRoot, ".claude", "plugins", "minutes");

  await mkdir(sourceDir, { recursive: true });
  await mkdir(pluginDir, { recursive: true });
  await writeFile(
    path.join(sourceDir, "skill.md"),
    `---
name: minutes-fixture
description: Fixture skill used to verify generated-file drift detection.
triggers:
  - verify fixture drift
metadata:
  display_name: Minutes Fixture
  short_description: Verify generated skill drift.
  default_prompt: Use Minutes Fixture.
  site_category: Testing
  site_example: /minutes-fixture
  site_best_for: Exercising the compiler drift gate.
assets:
  scripts: []
  templates: []
  references: []
output:
  claude:
    path: .claude/plugins/minutes/skills/minutes-fixture/SKILL.md
---

# Minutes Fixture
`,
    "utf8",
  );
  await writeFile(
    path.join(pluginDir, "plugin.json"),
    `${JSON.stringify(
      {
        name: "minutes-fixture",
        version: "0.0.0",
        description: "Compiler drift fixture",
        skills: [],
      },
      null,
      2,
    )}\n`,
    "utf8",
  );

  return repoRoot;
}

test("compile:dry passes clean generated skills and rejects drift", async (t) => {
  const repoRoot = await createFixtureRepo();
  t.after(async () => rm(repoRoot, { recursive: true, force: true }));

  const generated = runCompile(repoRoot, false);
  assert.equal(generated.status, 0, generated.stderr);
  assert.match(generated.stdout, /"status":"written"/);

  const clean = runCompile(repoRoot, true);
  assert.equal(clean.status, 0, clean.stderr);
  assert.match(clean.stdout, /"status":"clean"/);

  const generatedSkill = path.join(
    repoRoot,
    ".claude",
    "plugins",
    "minutes",
    "skills",
    "minutes-fixture",
    "SKILL.md",
  );
  await appendFile(generatedSkill, "\nmutated generated content\n", "utf8");

  const drifted = runCompile(repoRoot, true);
  assert.equal(drifted.status, 1, drifted.stderr);
  assert.match(drifted.stdout, /"status":"drift"/);
  assert.match(drifted.stdout, /minutes-fixture\/SKILL\.md/);
});
