import path from "node:path";
import { constants } from "node:fs";
import { access, readFile } from "node:fs/promises";
import { cwd, exit } from "node:process";
import { discoverCanonicalSkills } from "./discover.js";
import { HOSTS } from "../hosts/index.js";
import { renderSkillForHost } from "./render.js";
import { validateSkillAssets } from "./validate.js";
import { renderClaudePluginManifest } from "./plugin.js";
import { renderSiteSkillCatalog } from "./site.js";

interface CheckFailure {
  skill: string;
  host: string;
  message: string;
}

function getRootDir(): string {
  return cwd().endsWith(path.join("tooling", "skills"))
    ? cwd()
    : path.join(cwd(), "tooling", "skills");
}

async function main(): Promise<void> {
  const rootDir = getRootDir();
  const skills = await discoverCanonicalSkills(rootDir);
  const failures: CheckFailure[] = [];

  const expectedClaudeManifest = await renderClaudePluginManifest(rootDir, skills);
  const actualClaudeManifest = await readFile(
    path.join(rootDir, "..", "..", ".claude", "plugins", "minutes", "plugin.json"),
    "utf8",
  );
  if (actualClaudeManifest !== expectedClaudeManifest) {
    failures.push({
      skill: "plugin.json",
      host: "claude",
      message: "Claude plugin manifest skills[] block is out of sync with canonical skill metadata",
    });
  }

  const expectedSiteCatalog = renderSiteSkillCatalog(skills);
  let actualSiteCatalog: string | null = null;
  try {
    actualSiteCatalog = await readFile(
      path.join(rootDir, "..", "..", "site", "lib", "skills-catalog.json"),
      "utf8",
    );
  } catch (error) {
    if (!(error instanceof Error) || !("code" in error) || error.code !== "ENOENT") {
      throw error;
    }
  }
  if (actualSiteCatalog !== expectedSiteCatalog) {
    failures.push({
      skill: "skills-catalog.json",
      host: "site",
      message: "Public website skill catalog is out of sync with canonical skill metadata",
    });
  }

  for (const skill of skills) {
    await validateSkillAssets(skill);
    for (const host of Object.values(HOSTS)) {
      const artifact = renderSkillForHost(skill, host);
      if (
        host.name === "codex" &&
        (artifact.body.includes("${CLAUDE_PLUGIN_ROOT}") ||
          artifact.body.includes(".claude/plugins/minutes") ||
          artifact.body.includes("$MINUTES_SKILL_ROOT/skills/") ||
          artifact.body.includes("$MINUTES_SKILLS_ROOT/skills/"))
      ) {
        failures.push({
          skill: skill.id,
          host: host.name,
          message: "Codex output still contains unresolved Claude or invalid helper path references",
        });
      }

      if (host.name === "claude" && artifact.body.includes(".agents/skills/minutes")) {
        failures.push({
          skill: skill.id,
          host: host.name,
          message: "Claude output contains Codex repo-local skill paths",
        });
      }

      if (
        host.name === "opencode" &&
        (artifact.body.includes("${CLAUDE_PLUGIN_ROOT}") ||
          artifact.body.includes(".claude/plugins/minutes") ||
          artifact.body.includes(".agents/skills/minutes") ||
          artifact.body.includes("$MINUTES_SKILL_ROOT/skills/") ||
          artifact.body.includes("$MINUTES_SKILLS_ROOT/skills/"))
      ) {
        failures.push({
          skill: skill.id,
          host: host.name,
          message: "OpenCode output still contains unresolved Claude/Codex or invalid helper path references",
        });
      }

      if (
        host.name === "codex" &&
        !artifact.sidecarFiles.some((file) => file.relativePath.endsWith("agents/openai.yaml"))
      ) {
        failures.push({
          skill: skill.id,
          host: host.name,
          message: "Codex output is missing agents/openai.yaml sidecar metadata",
        });
      }

      if (
        host.name === "codex" &&
        (skill.frontmatter.assets?.scripts?.length ||
          skill.frontmatter.assets?.templates?.length ||
          skill.frontmatter.assets?.references?.length) &&
        artifact.assetFiles.length === 0
      ) {
        failures.push({
          skill: skill.id,
          host: host.name,
          message: "Codex output declares assets but no emitted asset files were planned",
        });
      }

      if (
        host.name === "opencode" &&
        (skill.frontmatter.assets?.scripts?.length ||
          skill.frontmatter.assets?.templates?.length ||
          skill.frontmatter.assets?.references?.length) &&
        artifact.assetFiles.length === 0
      ) {
        failures.push({
          skill: skill.id,
          host: host.name,
          message: "OpenCode output declares assets but no emitted asset files were planned",
        });
      }
    }
  }

  const runtimeFiles = [
    ".agents/skills/minutes/_runtime/hooks/lib/minutes-learn.mjs",
    ".agents/skills/minutes/_runtime/hooks/lib/minutes-learn-cli.mjs",
    ".opencode/skills/_runtime/hooks/lib/minutes-learn.mjs",
    ".opencode/skills/_runtime/hooks/lib/minutes-learn-cli.mjs",
  ];

  for (const runtimePath of runtimeFiles) {
    try {
      await access(path.join(rootDir, "..", "..", runtimePath), constants.F_OK);
    } catch {
      failures.push({
        skill: "_runtime",
        host: runtimePath.includes(".opencode/") ? "opencode" : "codex",
        message: `Missing generated runtime helper: ${runtimePath}`,
      });
    }
  }

  if (failures.length > 0) {
    console.error(JSON.stringify({ status: "error", failures }, null, 2));
    exit(1);
  }

  console.log(
    JSON.stringify({
      status: "ok",
      skillCount: skills.length,
      hosts: Object.keys(HOSTS),
    }),
  );
}

main().catch((error) => {
  console.error(
    JSON.stringify({
      status: "error",
      message: error instanceof Error ? error.message : String(error),
    }),
  );
  exit(1);
});
