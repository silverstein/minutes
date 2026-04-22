import type { CanonicalSkillSource } from "../schema.js";

interface SkillSiteEntry {
  category: string;
  example: string;
  bestFor: string;
}

function sortByCategoryThenName(
  a: { category: string; displayName: string },
  b: { category: string; displayName: string },
): number {
  return a.category === b.category
    ? a.displayName.localeCompare(b.displayName)
    : a.category.localeCompare(b.category);
}

export function renderSiteSkillCatalog(skills: CanonicalSkillSource[]): string {
  const missing = skills
    .filter((skill) => {
      const metadata = skill.frontmatter.metadata;
      return !metadata?.site_category || !metadata.site_example || !metadata.site_best_for;
    })
    .map((skill) => skill.frontmatter.name);
  if (missing.length > 0) {
    throw new Error(
      `Missing site skill metadata for: ${missing.join(", ")}. Update canonical skill frontmatter metadata.site_* fields.`,
    );
  }

  const catalog = skills
    .map((skill) => {
      const metadata = skill.frontmatter.metadata!;
      const siteEntry: SkillSiteEntry = {
        category: metadata.site_category!,
        example: metadata.site_example!,
        bestFor: metadata.site_best_for!,
      };
      const name = skill.frontmatter.name;
      const displayName =
        metadata.display_name ??
        name
          .split("-")
          .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
          .join(" ");
      return {
        name,
        displayName,
        category: siteEntry.category,
        shortDescription: metadata.short_description ?? skill.frontmatter.description,
        description: skill.frontmatter.description,
        bestFor: siteEntry.bestFor,
        example: siteEntry.example,
        triggers: skill.frontmatter.triggers,
        phase: skill.frontmatter.phase ?? null,
        userInvocable: skill.frontmatter.user_invocable ?? true,
      };
    })
    .sort(sortByCategoryThenName);

  return `${JSON.stringify(catalog, null, 2)}\n`;
}
