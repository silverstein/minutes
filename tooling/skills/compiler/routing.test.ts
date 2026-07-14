import test from "node:test";
import assert from "node:assert/strict";
import type { CanonicalSkillSource } from "../schema.js";
import { ROUTING_FIXTURES } from "./routing.fixtures.js";
import {
  evaluateRoutingFixtures,
  normalizeRoutingText,
  routeUtteranceToSkill,
} from "./routing.js";

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

test("normalizeRoutingText collapses case and punctuation", () => {
  assert.equal(
    normalizeRoutingText("Please, Review this Walkthrough!!"),
    "please review this walkthrough",
  );
});

test("routeUtteranceToSkill matches parameterized triggers with concrete values", () => {
  const decision = routeUtteranceToSkill(
    [makeSkill("minutes-search", ["what did we discuss about X"])],
    "What did we discuss about pricing?",
  );

  assert.equal(decision.match?.skillId, "minutes-search");
  assert.equal(decision.match?.matchType, "exact");
  assert.equal(decision.ambiguous.length, 0);
});

test("routeUtteranceToSkill prefers the longest matching trigger", () => {
  const decision = routeUtteranceToSkill(
    [
      makeSkill("minutes-brief", ["brief me"]),
      makeSkill("minutes-prep", ["brief me on Sarah"]),
    ],
    "Can you brief me on Sarah before my next meeting?",
  );

  assert.equal(decision.match?.skillId, "minutes-prep");
});

test("routeUtteranceToSkill reports ambiguity on equal top matches", () => {
  const decision = routeUtteranceToSkill(
    [
      makeSkill("minutes-a", ["show me X"]),
      makeSkill("minutes-b", ["show me Y"]),
    ],
    "show me pricing",
  );

  assert.equal(decision.match?.skillId, "minutes-a");
  assert.deepEqual(
    decision.ambiguous.map((match) => match.skillId),
    ["minutes-b"],
  );
});

test("evaluateRoutingFixtures passes on the real Minutes skill corpus shape", () => {
  const skills = [
    makeSkill("minutes-brief", ["brief me", "brief me on Sarah"]),
    makeSkill("minutes-cleanup", ["clean up recordings"]),
    makeSkill("minutes-copilot", ["start Coach", "coach this live meeting"]),
    makeSkill("minutes-debrief", ["debrief that call"]),
    makeSkill("minutes-graph", ["show me everyone who mentioned X", "across all meetings"]),
    makeSkill("minutes-ideas", ["what ideas did I have?"]),
    makeSkill("minutes-ingest", ["backfill knowledge"]),
    makeSkill("minutes-lint", ["check for stale action items"]),
    makeSkill("minutes-list", ["show my recent recordings"]),
    makeSkill("minutes-mirror", ["how did I do"]),
    makeSkill("minutes-note", ["note that"]),
    makeSkill("minutes-prep", ["prep me for my call with"]),
    makeSkill("minutes-recap", ["what happened in my meetings today"]),
    makeSkill("minutes-record", ["start recording"]),
    makeSkill("minutes-search", ["what did we discuss about X"]),
    makeSkill("minutes-setup", ["how do I start using minutes"]),
    makeSkill("minutes-tag", ["mark that as a win"]),
    makeSkill("minutes-verify", ["why isn't minutes working"]),
    makeSkill("minutes-video-review", ["review this walkthrough", "review this video"]),
    makeSkill("minutes-weekly", ["weekly summary", "what happened this week"]),
  ];

  const report = evaluateRoutingFixtures(skills, ROUTING_FIXTURES);
  assert.equal(report.ok, true);
  assert.equal(report.failures.length, 0);
});
