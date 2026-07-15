import path from "node:path";
import { fileURLToPath } from "node:url";
import { argv, cwd, exit } from "node:process";
import type { CanonicalSkillSource } from "../schema.js";
import { discoverCanonicalSkills } from "./discover.js";
import type { RoutingFixture } from "./routing.fixtures.js";
import { ROUTING_FIXTURES } from "./routing.fixtures.js";

const PLACEHOLDER_TOKENS = new Set(["x", "y", "z", "slot"]);

export interface RoutingMatch {
  skillId: string;
  trigger: string;
  score: number;
  consumedTokens: number;
  matchType: "exact" | "contains";
}

export interface RoutingDecision {
  utterance: string;
  normalizedUtterance: string;
  outcome: "skill" | "clarify" | "none";
  match: RoutingMatch | null;
  ambiguous: RoutingMatch[];
}

export interface RoutingFixtureFailure {
  utterance: string;
  expectedSkill: string | null;
  expectedOutcome: "skill" | "clarify";
  actualOutcome: "skill" | "clarify" | "none";
  actualSkill: string | null;
  matchedTrigger: string | null;
  ambiguousSkills: string[];
}

export interface RoutingEvalReport {
  ok: boolean;
  fixtureCount: number;
  passCount: number;
  failures: RoutingFixtureFailure[];
}

function getRootDir(): string {
  return cwd().endsWith(path.join("tooling", "skills"))
    ? cwd()
    : path.join(cwd(), "tooling", "skills");
}

export function normalizeRoutingText(raw: string): string {
  return raw
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function requiresLiveAssistanceSurfaceClarification(normalized: string): boolean {
  const tokens = normalized.split(" ");
  const requestsCoaching =
    normalized.includes("coach me") ||
    normalized.includes("coach this") ||
    normalized.includes("coaching me") ||
    normalized.includes("help me live");
  const isLiveContext = tokens.some((token) =>
    ["live", "meeting", "call"].includes(token),
  );
  if (!requestsCoaching || !isLiveContext) return false;

  const explicitHudSurface = [
    "minutes coach",
    "coach hud",
    "coaching hud",
    "copilot hud",
  ].some((phrase) => normalized.includes(phrase));
  const explicitTerminalSurface = [
    "terminal",
    "agent",
    "codex",
    "claude",
    "opencode",
    "sidekick",
    "strategist",
    "watch the live transcript",
    "watch this meeting",
  ].some((phrase) => normalized.includes(phrase));

  return !explicitHudSurface && !explicitTerminalSurface;
}

function tokenizeRoutingText(raw: string): string[] {
  const normalized = normalizeRoutingText(raw);
  return normalized.length === 0 ? [] : normalized.split(" ");
}

function isPlaceholder(token: string): boolean {
  return PLACEHOLDER_TOKENS.has(token);
}

function minTokensRequired(pattern: string[], startIndex: number): number {
  let total = 0;
  for (let index = startIndex; index < pattern.length; index += 1) {
    total += 1;
  }
  return total;
}

function matchPatternRecursive(
  pattern: string[],
  utterance: string[],
  patternIndex: number,
  utteranceIndex: number,
): number | null {
  if (patternIndex === pattern.length) {
    return utteranceIndex;
  }

  if (utteranceIndex >= utterance.length) {
    return null;
  }

  const token = pattern[patternIndex];
  if (!isPlaceholder(token)) {
    if (utterance[utteranceIndex] !== token) {
      return null;
    }
    return matchPatternRecursive(pattern, utterance, patternIndex + 1, utteranceIndex + 1);
  }

  const remainingMin = minTokensRequired(pattern, patternIndex + 1);
  const maxAdvance = utterance.length - remainingMin;
  for (let nextIndex = utteranceIndex + 1; nextIndex <= maxAdvance; nextIndex += 1) {
    const result = matchPatternRecursive(pattern, utterance, patternIndex + 1, nextIndex);
    if (result !== null) {
      return result;
    }
  }

  return null;
}

function findPatternMatch(pattern: string[], utterance: string[]): {
  start: number;
  end: number;
} | null {
  if (pattern.length === 0 || utterance.length === 0) {
    return null;
  }

  for (let start = 0; start < utterance.length; start += 1) {
    const end = matchPatternRecursive(pattern, utterance, 0, start);
    if (end !== null) {
      return { start, end };
    }
  }

  return null;
}

function compareMatches(a: RoutingMatch, b: RoutingMatch): number {
  return (
    b.score - a.score ||
    b.consumedTokens - a.consumedTokens ||
    a.skillId.localeCompare(b.skillId) ||
    a.trigger.localeCompare(b.trigger)
  );
}

export function routeUtteranceToSkill(
  skills: CanonicalSkillSource[],
  utterance: string,
): RoutingDecision {
  const utteranceTokens = tokenizeRoutingText(utterance);
  const normalizedUtterance = utteranceTokens.join(" ");

  if (requiresLiveAssistanceSurfaceClarification(normalizedUtterance)) {
    return {
      utterance,
      normalizedUtterance,
      outcome: "clarify",
      match: null,
      ambiguous: [],
    };
  }

  const candidates: RoutingMatch[] = [];

  for (const skill of skills) {
    for (const trigger of skill.frontmatter.triggers) {
      const triggerTokens = tokenizeRoutingText(trigger);
      const match = findPatternMatch(triggerTokens, utteranceTokens);
      if (!match) continue;

      const consumedTokens = match.end - match.start;
      const isExact = match.start === 0 && match.end === utteranceTokens.length;
      candidates.push({
        skillId: skill.id,
        trigger,
        score: (isExact ? 1000 : 500) + triggerTokens.length * 10 + consumedTokens,
        consumedTokens,
        matchType: isExact ? "exact" : "contains",
      });
    }
  }

  candidates.sort(compareMatches);
  const top = candidates[0] ?? null;
  if (!top) {
    return {
      utterance,
      normalizedUtterance,
      outcome: "none",
      match: null,
      ambiguous: [],
    };
  }

  const ambiguous = candidates.filter(
    (candidate) =>
      candidate.skillId !== top.skillId &&
      candidate.score === top.score &&
      candidate.consumedTokens === top.consumedTokens,
  );

  return {
    utterance,
    normalizedUtterance,
    outcome: "skill",
    match: top,
    ambiguous,
  };
}

export function evaluateRoutingFixtures(
  skills: CanonicalSkillSource[],
  fixtures: RoutingFixture[] = ROUTING_FIXTURES,
): RoutingEvalReport {
  const failures: RoutingFixtureFailure[] = [];

  for (const fixture of fixtures) {
    const decision = routeUtteranceToSkill(skills, fixture.utterance);
    const matchedSkill = decision.match?.skillId ?? null;
    const ambiguousSkills = decision.ambiguous.map((match) => match.skillId);
    const expectedOutcome = fixture.expectedOutcome ?? "skill";

    if (
      decision.outcome !== expectedOutcome ||
      matchedSkill !== fixture.expectedSkill ||
      ambiguousSkills.length > 0
    ) {
      failures.push({
        utterance: fixture.utterance,
        expectedSkill: fixture.expectedSkill,
        expectedOutcome,
        actualOutcome: decision.outcome,
        actualSkill: matchedSkill,
        matchedTrigger: decision.match?.trigger ?? null,
        ambiguousSkills,
      });
    }
  }

  return {
    ok: failures.length === 0,
    fixtureCount: fixtures.length,
    passCount: fixtures.length - failures.length,
    failures,
  };
}

async function main(): Promise<void> {
  const rootDir = getRootDir();
  const skills = await discoverCanonicalSkills(rootDir);
  const report = evaluateRoutingFixtures(skills);

  if (report.ok) {
    console.log(
      JSON.stringify({
        status: "ok",
        fixtureCount: report.fixtureCount,
        passCount: report.passCount,
      }),
    );
    return;
  }

  console.error(
    JSON.stringify(
      {
        status: "error",
        fixtureCount: report.fixtureCount,
        passCount: report.passCount,
        failures: report.failures,
      },
      null,
      2,
    ),
  );
  exit(1);
}

const invokedPath = argv[1] ? path.resolve(argv[1]) : null;
const modulePath = fileURLToPath(import.meta.url);

if (invokedPath === modulePath) {
  main().catch((error) => {
    console.error(
      JSON.stringify({
        status: "error",
        message: error instanceof Error ? error.message : String(error),
      }),
    );
    exit(1);
  });
}
