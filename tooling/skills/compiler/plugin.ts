import { readFile } from "node:fs/promises";
import path from "node:path";
import type { CanonicalSkillSource } from "../schema.js";

interface ClaudePluginManifest {
  name: string;
  version: string;
  description: string;
  skills: Array<{ name: string; path: string }>;
  agents?: Array<{ name: string; path: string }>;
  hooks?: Record<string, unknown>;
}

function getRelativeSkillPath(skill: CanonicalSkillSource): string {
  const configured =
    skill.frontmatter.output?.claude?.path ??
    `.claude/plugins/minutes/skills/${skill.frontmatter.name}/SKILL.md`;
  return configured.replace(/^\.claude\/plugins\/minutes\//, "");
}

export async function renderClaudePluginManifest(rootDir: string, skills: CanonicalSkillSource[]): Promise<string> {
  const manifestPath = path.join(rootDir, "..", "..", ".claude", "plugins", "minutes", "plugin.json");
  const raw = await readFile(manifestPath, "utf8");
  const manifest = JSON.parse(raw) as ClaudePluginManifest;

  const existingOrder = new Map(
    (manifest.skills ?? []).map((skill, index) => [skill.name, index]),
  );

  const generatedSkills = skills
    .map((skill) => ({
      name: skill.frontmatter.name,
      path: getRelativeSkillPath(skill),
    }))
    .sort((a, b) => {
      const aRank = existingOrder.get(a.name) ?? Number.MAX_SAFE_INTEGER;
      const bRank = existingOrder.get(b.name) ?? Number.MAX_SAFE_INTEGER;
      if (aRank !== bRank) return aRank - bRank;
      return a.name.localeCompare(b.name);
    });

  const nextManifest: ClaudePluginManifest = {
    ...manifest,
    skills: generatedSkills,
  };

  return `${JSON.stringify(nextManifest, null, 2)}\n`;
}
