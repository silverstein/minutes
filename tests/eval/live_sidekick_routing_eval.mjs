#!/usr/bin/env node
/** Replay the public routing fixture through the compiled canonical router. */

import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "../..");
const fixturePath = path.join(
  repoRoot,
  "crates/core/tests/fixtures/live_sidekick_eval/v1/routing_disambiguation.json",
);
const routingModule = path.join(repoRoot, "tooling/skills/dist/compiler/routing.js");
const discoverModule = path.join(repoRoot, "tooling/skills/dist/compiler/discover.js");

const fixture = JSON.parse(await fs.readFile(fixturePath, "utf8"));
const { routeUtteranceToSkill } = await import(pathToFileURL(routingModule));
const { discoverCanonicalSkills } = await import(pathToFileURL(discoverModule));
const skills = await discoverCanonicalSkills(path.join(repoRoot, "tooling/skills"));

const eventsByRequest = new Map(
  fixture.events
    .filter((event) => event.kind === "surface_request")
    .map((event) => [event.payload.request_id, event]),
);

function replay() {
  return fixture.execution.cases.map((expected) => {
    const event = eventsByRequest.get(expected.request_id);
    if (!event) throw new Error(`missing surface_request ${expected.request_id}`);
    const decision = routeUtteranceToSkill(skills, event.payload.text);
    return {
      request_id: expected.request_id,
      outcome: decision.outcome,
      skill_id: decision.match?.skillId ?? null,
      ambiguous_skill_ids: decision.ambiguous.map((match) => match.skillId),
    };
  });
}

const first = replay();
const second = replay();
if (JSON.stringify(first) !== JSON.stringify(second)) {
  throw new Error("routing fixture produced nondeterministic decisions");
}

const expected = fixture.execution.cases.map((item) => ({
  request_id: item.request_id,
  outcome: item.outcome,
  skill_id: item.skill_id,
  ambiguous_skill_ids: [],
}));
if (JSON.stringify(first) !== JSON.stringify(expected)) {
  console.error(JSON.stringify({ expected, actual: first }, null, 2));
  process.exit(1);
}

console.log(
  JSON.stringify({
    status: "ok",
    runner: "skill_routing",
    deterministic_replays: 2,
    cases: first.length,
  }),
);
