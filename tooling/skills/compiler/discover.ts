import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import type { CanonicalSkillSource } from "../schema.js";
import { extractFrontmatter, parseFrontmatter } from "./frontmatter.js";
import { validateCanonicalSkillSource } from "./validate.js";

export async function discoverCanonicalSkills(rootDir: string): Promise<CanonicalSkillSource[]> {
  const sourcesDir = path.join(rootDir, "sources");
  const entries = await readdir(sourcesDir, { withFileTypes: true });
  const skills: CanonicalSkillSource[] = [];

  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    const sourcePath = path.join(sourcesDir, entry.name, "skill.md");
    const raw = await readFile(sourcePath, "utf8");
    const { frontmatterRaw, body } = extractFrontmatter(raw);
    const skill: CanonicalSkillSource = {
      id: entry.name,
      sourcePath,
      frontmatter: parseFrontmatter(frontmatterRaw),
      body,
    };
    validateCanonicalSkillSource(skill);
    skills.push(skill);
  }

  return skills.sort((a, b) => a.id.localeCompare(b.id));
}
