import path from "node:path";
import { cwd, exit } from "node:process";
import { discoverCanonicalSkills } from "./discover.js";
import { HOSTS } from "../hosts/index.js";
import { renderSkillForHost } from "./render.js";
import { validateSkillAssets } from "./validate.js";

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
