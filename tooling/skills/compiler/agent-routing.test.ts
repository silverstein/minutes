import test from "node:test";
import assert from "node:assert/strict";
import type { CanonicalSkillSource } from "../schema.js";
import {
  buildAgentRoutingPrompt,
  classifyAgentUnavailable,
  extractSkillChoice,
} from "./agent-routing.js";

function makeSkill(
  name: string,
  triggers: string[],
): CanonicalSkillSource {
  return {
    id: name,
    sourcePath: `/tmp/${name}/skill.md`,
    frontmatter: {
      name,
      description: `${name} description`,
      triggers,
      user_invocable: true,
      metadata: {
        display_name: name,
        short_description: `${name} short`,
        default_prompt: `Use ${name}`,
        site_category: "Lifecycle",
        site_example: `/${name}`,
        site_best_for: `${name} best for`,
      },
    },
    body: `# ${name}`,
  };
}

test("buildAgentRoutingPrompt includes the utterance and available skills", () => {
  const prompt = buildAgentRoutingPrompt(
    [
      makeSkill("minutes-search", ["what did we discuss about X"]),
      makeSkill("minutes-brief", ["brief me"]),
    ],
    "What did we discuss about pricing?",
  );

  assert.match(prompt, /User request: What did we discuss about pricing\?/);
  assert.match(prompt, /minutes-search:/);
  assert.match(prompt, /minutes-brief:/);
  assert.match(prompt, /SKILL: <skill-id> or CLARIFY/);
});

test("extractSkillChoice accepts JSON output", () => {
  const parsed = extractSkillChoice(
    '{"skill":"minutes-search"}',
    new Set(["minutes-search"]),
  );
  assert.equal(parsed.skill, "minutes-search");
  assert.equal(parsed.clarify, false);
  assert.equal(parsed.reason, null);
});

test("extractSkillChoice accepts SKILL line output", () => {
  const parsed = extractSkillChoice(
    "SKILL: minutes-brief",
    new Set(["minutes-brief"]),
  );
  assert.equal(parsed.skill, "minutes-brief");
  assert.equal(parsed.clarify, false);
});

test("extractSkillChoice finds SKILL markers inside noisy output", () => {
  const parsed = extractSkillChoice(
    "MCP issues detected. Run /mcp list for status.SKILL: minutes-brief",
    new Set(["minutes-brief"]),
  );
  assert.equal(parsed.skill, "minutes-brief");
  assert.equal(parsed.clarify, false);
});

test("extractSkillChoice accepts clarification output", () => {
  const parsed = extractSkillChoice("CLARIFY", new Set(["minutes-copilot"]));
  assert.equal(parsed.skill, null);
  assert.equal(parsed.clarify, true);
  assert.equal(parsed.reason, null);
});

test("extractSkillChoice rejects unknown skills", () => {
  const parsed = extractSkillChoice(
    "SKILL: not-a-real-skill",
    new Set(["minutes-brief"]),
  );
  assert.equal(parsed.skill, null);
  assert.equal(parsed.clarify, false);
  assert.equal(parsed.reason, "unparseable_output");
});

test("classifyAgentUnavailable catches rate-limit text on stdout", () => {
  assert.equal(
    classifyAgentUnavailable(
      "You've hit your limit · resets tomorrow",
      "",
      1,
    ),
    true,
  );
});

test("classifyAgentUnavailable catches capacity exhaustion even with exit code 0", () => {
  assert.equal(
    classifyAgentUnavailable(
      "",
      "status 429 RESOURCE_EXHAUSTED No capacity available for model",
      0,
    ),
    true,
  );
});
