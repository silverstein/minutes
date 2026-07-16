import test from "node:test";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import type { CanonicalSkillSource } from "../schema.js";
import { HOSTS } from "../hosts/index.js";
import { renderSkillForHost } from "./render.js";
import { findUnownedGeneratedArtifacts } from "./ownership.js";

function makeFixtureSkill(): CanonicalSkillSource {
  return {
    id: "minutes-fixture",
    sourcePath: "/tmp/minutes-fixture/skill.md",
    frontmatter: {
      name: "minutes-fixture",
      description: "Fixture skill used to verify generated-artifact ownership.",
      triggers: ["verify ownership fixture"],
      metadata: {
        display_name: "Minutes Fixture",
        short_description: "Verify generated ownership.",
        default_prompt: "Use Minutes Fixture.",
      },
    },
    body: "# Minutes Fixture\n",
  };
}

async function writeArtifact(repoRoot: string, relativePath: string, content = "fixture\n") {
  const absolutePath = path.join(repoRoot, relativePath);
  await mkdir(path.dirname(absolutePath), { recursive: true });
  await writeFile(absolutePath, content, "utf8");
}

async function createOwnedFixture(): Promise<{
  repoRoot: string;
  skills: CanonicalSkillSource[];
}> {
  const repoRoot = await mkdtemp(path.join(tmpdir(), "minutes-ownership-"));
  const skills = [makeFixtureSkill()];

  for (const host of Object.values(HOSTS)) {
    const artifact = renderSkillForHost(skills[0], host);
    await writeArtifact(repoRoot, artifact.outputPath, artifact.body);
    for (const sidecar of artifact.sidecarFiles) {
      await writeArtifact(repoRoot, sidecar.relativePath, sidecar.content);
    }
  }

  await writeArtifact(
    repoRoot,
    ".agents/skills/minutes/_runtime/hooks/lib/minutes-learn.mjs",
  );

  return { repoRoot, skills };
}

test("generated-artifact ownership passes when every artifact is owned", async (t) => {
  const fixture = await createOwnedFixture();
  t.after(async () => rm(fixture.repoRoot, { recursive: true, force: true }));

  assert.deepEqual(
    await findUnownedGeneratedArtifacts(fixture.repoRoot, fixture.skills),
    [],
  );
});

test("generated-artifact ownership rejects an orphan skill directory by name", async (t) => {
  const fixture = await createOwnedFixture();
  t.after(async () => rm(fixture.repoRoot, { recursive: true, force: true }));
  const orphanPath = ".agents/skills/minutes/minutes-orphan";
  await mkdir(path.join(fixture.repoRoot, orphanPath), { recursive: true });

  assert.deepEqual(
    await findUnownedGeneratedArtifacts(fixture.repoRoot, fixture.skills),
    [orphanPath],
  );
});

test("generated-artifact ownership rejects an orphan OpenCode command by name", async (t) => {
  const fixture = await createOwnedFixture();
  t.after(async () => rm(fixture.repoRoot, { recursive: true, force: true }));
  const orphanPath = ".opencode/commands/minutes-orphan.md";
  await writeArtifact(fixture.repoRoot, orphanPath);

  assert.deepEqual(
    await findUnownedGeneratedArtifacts(fixture.repoRoot, fixture.skills),
    [orphanPath],
  );
});
