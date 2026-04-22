import test from "node:test";
import assert from "node:assert/strict";
import type { CanonicalSkillSource } from "../schema.js";
import { renderSiteSkillCatalog } from "./site.js";

function makeSkill(
  name: string,
  overrides: Partial<CanonicalSkillSource["frontmatter"]> = {},
): CanonicalSkillSource {
  return {
    id: name,
    sourcePath: `/tmp/${name}/skill.md`,
    frontmatter: {
      name,
      description: `${name} description`,
      triggers: [`${name} trigger`],
      user_invocable: true,
      metadata: {
        display_name: name,
        short_description: `${name} short`,
        default_prompt: `Use ${name}`,
        site_category: "Lifecycle",
        site_example: `/${name}`,
        site_best_for: `${name} best for`,
      },
      ...overrides,
    },
    body: `# ${name}`,
  };
}

test("renderSiteSkillCatalog sorts by category then display name", () => {
  const skills = [
    makeSkill("minutes-zeta", {
      metadata: {
        display_name: "Zeta",
        short_description: "zeta short",
        default_prompt: "Use zeta",
        site_category: "Capture",
        site_example: "/minutes-zeta",
        site_best_for: "zeta best",
      },
    }),
    makeSkill("minutes-alpha", {
      metadata: {
        display_name: "Alpha",
        short_description: "alpha short",
        default_prompt: "Use alpha",
        site_category: "Artifacts",
        site_example: "/minutes-alpha",
        site_best_for: "alpha best",
      },
    }),
    makeSkill("minutes-beta", {
      metadata: {
        display_name: "Beta",
        short_description: "beta short",
        default_prompt: "Use beta",
        site_category: "Capture",
        site_example: "/minutes-beta",
        site_best_for: "beta best",
      },
    }),
  ];

  const rendered = JSON.parse(renderSiteSkillCatalog(skills));
  assert.deepEqual(
    rendered.map((skill: { name: string }) => skill.name),
    ["minutes-alpha", "minutes-beta", "minutes-zeta"],
  );
});

test("renderSiteSkillCatalog uses canonical frontmatter metadata", () => {
  const skills = [
    makeSkill("minutes-video-review", {
      metadata: {
        display_name: "Minutes Video Review",
        short_description: "Review a video into a durable brief.",
        default_prompt: "Use Minutes Video Review",
        site_category: "Artifacts",
        site_example: "/minutes-video-review https://example.com/video",
        site_best_for: "Turn a walkthrough into an artifact bundle.",
      },
    }),
  ];

  const [entry] = JSON.parse(renderSiteSkillCatalog(skills));
  assert.equal(entry.category, "Artifacts");
  assert.equal(entry.example, "/minutes-video-review https://example.com/video");
  assert.equal(entry.bestFor, "Turn a walkthrough into an artifact bundle.");
  assert.equal(entry.shortDescription, "Review a video into a durable brief.");
});

test("renderSiteSkillCatalog throws when site metadata is missing", () => {
  const skills = [
    makeSkill("minutes-broken", {
      metadata: {
        display_name: "Minutes Broken",
        short_description: "broken short",
        default_prompt: "Use Minutes Broken",
      },
    }),
  ];

  assert.throws(
    () => renderSiteSkillCatalog(skills),
    /Missing site skill metadata/,
  );
});
