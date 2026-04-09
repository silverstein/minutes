import { access } from "node:fs/promises";
import path from "node:path";
import type { CanonicalSkillSource } from "../schema.js";

export function validateCanonicalSkillSource(skill: CanonicalSkillSource): void {
  if (!skill.frontmatter.name) {
    throw new Error(`Canonical skill ${skill.id} is missing "name"`);
  }
  if (skill.frontmatter.name !== skill.id) {
    throw new Error(
      `Canonical skill name "${skill.frontmatter.name}" must match directory "${skill.id}"`,
    );
  }
  if (!skill.frontmatter.description) {
    throw new Error(`Canonical skill ${skill.id} is missing "description"`);
  }
  if (!Array.isArray(skill.frontmatter.triggers) || skill.frontmatter.triggers.length === 0) {
    throw new Error(`Canonical skill ${skill.id} must declare at least one trigger`);
  }
}

export async function resolveSkillAssetSourcePath(
  skill: CanonicalSkillSource,
  relativeAsset: string,
): Promise<string> {
  const sourceDir = path.dirname(skill.sourcePath);
  const repoRoot = path.join(sourceDir, "..", "..", "..", "..");
  const legacySkillDir = path.join(repoRoot, ".claude", "plugins", "minutes", "skills", skill.id);
  const assetPath = path.join(sourceDir, relativeAsset);
  try {
    await access(assetPath);
    return assetPath;
  } catch {
    const legacyAssetPath = path.join(legacySkillDir, relativeAsset);
    try {
      await access(legacyAssetPath);
      return legacyAssetPath;
    } catch {
      throw new Error(
        `Canonical skill ${skill.id} references missing asset: ${relativeAsset}`,
      );
    }
  }
}

export async function validateSkillAssets(skill: CanonicalSkillSource): Promise<void> {
  const assetGroups = skill.frontmatter.assets ?? {};
  for (const group of [assetGroups.scripts, assetGroups.templates, assetGroups.references]) {
    for (const relativeAsset of group ?? []) {
      await resolveSkillAssetSourcePath(skill, relativeAsset);
    }
  }
}
