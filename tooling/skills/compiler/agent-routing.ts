import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import { tmpdir } from "node:os";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { argv, cwd, exit } from "node:process";
import type { CanonicalSkillSource } from "../schema.js";
import { discoverCanonicalSkills } from "./discover.js";
import type { RoutingFixture } from "./routing.fixtures.js";
import { ROUTING_FIXTURES } from "./routing.fixtures.js";

export type AgentId = "claude" | "codex" | "gemini" | "opencode" | "pi";

export interface AgentRoutingOptions {
  agent: AgentId;
  fixtures: RoutingFixture[];
  timeoutMs: number;
}

export interface AgentRoutingResult {
  utterance: string;
  expectedSkill: string | null;
  expectedOutcome: "skill" | "clarify";
  actualSkill: string | null;
  actualOutcome: "skill" | "clarify" | null;
  status:
    | "passed"
    | "mismatch"
    | "parse_error"
    | "command_error"
    | "unavailable";
  stdout: string;
  stderr: string;
  exitCode: number | null;
  command: string[];
}

export interface AgentRoutingReport {
  agent: AgentId;
  ok: boolean;
  total: number;
  passed: number;
  unavailable: number;
  results: AgentRoutingResult[];
}

interface AgentInvocation {
  command: string[];
  cwd: string;
  outputFile?: string | null;
}

function getRootDir(): string {
  return cwd().endsWith(path.join("tooling", "skills"))
    ? cwd()
    : path.join(cwd(), "tooling", "skills");
}

function getRepoRoot(rootDir: string): string {
  return path.join(rootDir, "..", "..");
}

function parseArgs(rawArgs: string[]): {
  agent: AgentId;
  fixtureIds: Set<number> | null;
  limit: number | null;
  dryRun: boolean;
  timeoutMs: number;
} {
  let agent: AgentId | null = null;
  let limit: number | null = null;
  let dryRun = false;
  let timeoutMs = 90_000;
  const fixtureIds = new Set<number>();

  for (let index = 0; index < rawArgs.length; index += 1) {
    const arg = rawArgs[index];
    if (arg === "--agent" && rawArgs[index + 1]) {
      agent = rawArgs[index + 1] as AgentId;
      index += 1;
      continue;
    }
    if (arg === "--fixture" && rawArgs[index + 1]) {
      fixtureIds.add(Number(rawArgs[index + 1]));
      index += 1;
      continue;
    }
    if (arg === "--limit" && rawArgs[index + 1]) {
      limit = Number(rawArgs[index + 1]);
      index += 1;
      continue;
    }
    if (arg === "--timeout-ms" && rawArgs[index + 1]) {
      timeoutMs = Number(rawArgs[index + 1]);
      index += 1;
      continue;
    }
    if (arg === "--dry-run") {
      dryRun = true;
      continue;
    }
  }

  if (!agent || !["claude", "codex", "gemini", "opencode", "pi"].includes(agent)) {
    throw new Error("Usage: --agent <claude|codex|gemini|opencode|pi> [--fixture N] [--limit N] [--timeout-ms N] [--dry-run]");
  }

  return {
    agent,
    fixtureIds: fixtureIds.size > 0 ? fixtureIds : null,
    limit,
    dryRun,
    timeoutMs,
  };
}

export function buildAgentRoutingPrompt(
  skills: CanonicalSkillSource[],
  utterance: string,
): string {
  const skillLines = skills
    .map((skill) => {
      const triggerSummary = skill.frontmatter.triggers
        .slice(0, 2)
        .map((trigger) => `"${trigger}"`)
        .join(", ");
      const summary =
        skill.frontmatter.metadata?.site_best_for ??
        skill.frontmatter.metadata?.short_description ??
        skill.frontmatter.description;
      return `- ${skill.id}: ${summary} Example triggers: ${triggerSummary}`;
    })
    .join("\n");

  return [
    "You are evaluating which Minutes skill should handle a user request.",
    "Choose exactly one skill from the provided list, unless the request is ambiguous about which product surface the user wants.",
    "In particular, a generic request for live coaching without saying whether the current terminal agent or the Minutes Coach HUD should help requires clarification.",
    "Do not explain your reasoning.",
    'Respond in exactly one line using either: SKILL: <skill-id> or CLARIFY',
    "",
    "Available skills:",
    skillLines,
    "",
    `User request: ${utterance}`,
  ].join("\n");
}

export function extractSkillChoice(
  rawOutput: string,
  validSkillIds: Set<string>,
): { skill: string | null; clarify: boolean; reason: string | null } {
  const trimmed = rawOutput.trim();
  if (trimmed.length === 0) {
    return { skill: null, clarify: false, reason: "empty_output" };
  }

  try {
    const parsed = JSON.parse(trimmed) as { skill?: unknown; action?: unknown };
    if (typeof parsed.skill === "string" && validSkillIds.has(parsed.skill)) {
      return { skill: parsed.skill, clarify: false, reason: null };
    }
    if (parsed.action === "clarify") {
      return { skill: null, clarify: true, reason: null };
    }
  } catch {
    // Fall through to text parsing.
  }

  if (/^CLARIFY\.?$/i.test(trimmed)) {
    return { skill: null, clarify: true, reason: null };
  }

  const skillLine = trimmed.match(/SKILL:\s*([a-z0-9-]+)/i);
  if (skillLine && validSkillIds.has(skillLine[1])) {
    return { skill: skillLine[1], clarify: false, reason: null };
  }

  if (validSkillIds.has(trimmed)) {
    return { skill: trimmed, clarify: false, reason: null };
  }

  return { skill: null, clarify: false, reason: "unparseable_output" };
}

async function buildAgentInvocation(
  agent: AgentId,
  prompt: string,
  repoRoot: string,
): Promise<AgentInvocation> {
  if (agent === "claude") {
    return {
      command: [
        "claude",
        "-p",
        prompt,
        "--output-format",
        "text",
        "--bare",
        "--no-session-persistence",
        "--dangerously-skip-permissions",
      ],
      cwd: repoRoot,
    };
  }

  if (agent === "codex") {
    const outputDir = await mkdtemp(path.join(tmpdir(), "minutes-routing-codex-"));
    const outputFile = path.join(outputDir, "last-message.txt");
    return {
      command: [
        "codex",
        "exec",
        prompt,
        "--cd",
        repoRoot,
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                "--ephemeral",
                "--ignore-user-config",
                "--ignore-rules",
                "-o",
                outputFile,
            ],
            cwd: repoRoot,
      outputFile,
    };
  }

  if (agent === "gemini") {
    return {
      command: [
        "gemini",
                "-p",
                prompt,
                "--output-format",
                "text",
                "--approval-mode",
                "plan",
                "--extensions",
                "",
            ],
            cwd: repoRoot,
        };
  }

  if (agent === "pi") {
    return {
      command: [
        "pi",
        "--no-session",
        "--no-tools",
        "--no-extensions",
        "--no-skills",
        "--no-prompt-templates",
        "--no-context-files",
        "-p",
        prompt,
      ],
      cwd: repoRoot,
    };
  }

  return {
    command: [
      "opencode",
      "run",
      prompt,
            "--dir",
            repoRoot,
            "--format",
            "default",
            "--pure",
            "--dangerously-skip-permissions",
        ],
        cwd: repoRoot,
  };
}

async function runCommand(
  invocation: AgentInvocation,
  timeoutMs: number,
): Promise<{ stdout: string; stderr: string; exitCode: number | null }> {
  return await new Promise((resolvePromise) => {
    const child = spawn(invocation.command[0], invocation.command.slice(1), {
      cwd: invocation.cwd,
      stdio: ["ignore", "pipe", "pipe"],
    });

    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      resolvePromise({
        stdout,
        stderr: `${stderr}\nTIMEOUT after ${timeoutMs}ms`.trim(),
        exitCode: null,
      });
    }, timeoutMs);

    child.stdout.on("data", (chunk) => {
      stdout += String(chunk);
    });
    child.stderr.on("data", (chunk) => {
      stderr += String(chunk);
    });

    child.on("error", (error) => {
      clearTimeout(timer);
      resolvePromise({
        stdout,
        stderr: `${stderr}\n${error.message}`.trim(),
        exitCode: null,
      });
    });

    child.on("close", (code) => {
      clearTimeout(timer);
      resolvePromise({
        stdout,
        stderr,
        exitCode: code,
      });
    });
  });
}

export function classifyAgentUnavailable(
  stdout: string,
  stderr: string,
  exitCode: number | null,
): boolean {
  const combined = `${stdout}\n${stderr}`.toLowerCase();
  return (
    exitCode === null ||
    combined.includes("not found") ||
    combined.includes("no such file") ||
    combined.includes("hit your limit") ||
    combined.includes("rate limit") ||
    combined.includes("too many requests") ||
    combined.includes("resource_exhausted") ||
    combined.includes("model_capacity_exhausted") ||
    combined.includes("no capacity available") ||
    combined.includes("auth") ||
    combined.includes("login")
  );
}

export async function evaluateAgentRouting(
  skills: CanonicalSkillSource[],
  options: AgentRoutingOptions,
): Promise<AgentRoutingReport> {
  const rootDir = getRootDir();
  const repoRoot = getRepoRoot(rootDir);
  const validSkillIds = new Set(skills.map((skill) => skill.id));
  const results: AgentRoutingResult[] = [];

  for (const fixture of options.fixtures) {
    const expectedOutcome = fixture.expectedOutcome ?? "skill";
    const prompt = buildAgentRoutingPrompt(skills, fixture.utterance);
    const invocation = await buildAgentInvocation(options.agent, prompt, repoRoot);
    const raw = await runCommand(invocation, options.timeoutMs);
    let stdout = raw.stdout;

    if (invocation.outputFile && existsSync(invocation.outputFile)) {
      stdout = await readFile(invocation.outputFile, "utf8");
      await rm(path.dirname(invocation.outputFile), { recursive: true, force: true });
    }

    if (classifyAgentUnavailable(stdout, raw.stderr, raw.exitCode)) {
      results.push({
        utterance: fixture.utterance,
        expectedSkill: fixture.expectedSkill,
        expectedOutcome,
        actualSkill: null,
        actualOutcome: null,
        status: "unavailable",
        stdout,
        stderr: raw.stderr,
        exitCode: raw.exitCode,
        command: invocation.command,
      });
      continue;
    }

    if (raw.exitCode !== 0) {
      results.push({
        utterance: fixture.utterance,
        expectedSkill: fixture.expectedSkill,
        expectedOutcome,
        actualSkill: null,
        actualOutcome: null,
        status: "command_error",
        stdout,
        stderr: raw.stderr,
        exitCode: raw.exitCode,
        command: invocation.command,
      });
      continue;
    }

    const parsed = extractSkillChoice(stdout, validSkillIds);
    if (!parsed.skill) {
      if (parsed.clarify) {
        results.push({
          utterance: fixture.utterance,
          expectedSkill: fixture.expectedSkill,
          expectedOutcome,
          actualSkill: null,
          actualOutcome: "clarify",
          status: expectedOutcome === "clarify" ? "passed" : "mismatch",
          stdout,
          stderr: raw.stderr,
          exitCode: raw.exitCode,
          command: invocation.command,
        });
        continue;
      }
      results.push({
        utterance: fixture.utterance,
        expectedSkill: fixture.expectedSkill,
        expectedOutcome,
        actualSkill: null,
        actualOutcome: null,
        status: "parse_error",
        stdout,
        stderr: raw.stderr,
        exitCode: raw.exitCode,
        command: invocation.command,
      });
      continue;
    }

    results.push({
      utterance: fixture.utterance,
      expectedSkill: fixture.expectedSkill,
      expectedOutcome,
      actualSkill: parsed.skill,
      actualOutcome: "skill",
      status:
        expectedOutcome === "skill" && parsed.skill === fixture.expectedSkill
          ? "passed"
          : "mismatch",
      stdout,
      stderr: raw.stderr,
      exitCode: raw.exitCode,
      command: invocation.command,
    });
  }

  const passed = results.filter((result) => result.status === "passed").length;
  const unavailable = results.filter((result) => result.status === "unavailable").length;
  const blockingResults = results.filter(
    (result) => result.status !== "passed" && result.status !== "unavailable",
  );

  return {
    agent: options.agent,
    ok: blockingResults.length === 0,
    total: results.length,
    passed,
    unavailable,
    results,
  };
}

async function main(): Promise<void> {
  const args = parseArgs(argv.slice(2));
  const rootDir = getRootDir();
  const skills = await discoverCanonicalSkills(rootDir);
  let fixtures = ROUTING_FIXTURES;

  if (args.fixtureIds) {
    fixtures = fixtures.filter((_, index) => args.fixtureIds!.has(index + 1));
  }
  if (args.limit !== null) {
    fixtures = fixtures.slice(0, args.limit);
  }

  if (args.dryRun) {
    const repoRoot = getRepoRoot(rootDir);
    const preview = [];
    for (const fixture of fixtures) {
      const prompt = buildAgentRoutingPrompt(skills, fixture.utterance);
      const invocation = await buildAgentInvocation(args.agent, prompt, repoRoot);
      preview.push({
        utterance: fixture.utterance,
        expectedSkill: fixture.expectedSkill,
        expectedOutcome: fixture.expectedOutcome ?? "skill",
        command: invocation.command,
      });
      if (invocation.outputFile) {
        await rm(path.dirname(invocation.outputFile), { recursive: true, force: true });
      }
    }
    console.log(JSON.stringify({ status: "dry-run", agent: args.agent, total: preview.length, preview }, null, 2));
    return;
  }

  const report = await evaluateAgentRouting(skills, {
    agent: args.agent,
    fixtures,
    timeoutMs: args.timeoutMs,
  });

  const output = {
    status: report.ok ? "ok" : "error",
    agent: report.agent,
    total: report.total,
    passed: report.passed,
    unavailable: report.unavailable,
    results: report.results,
  };

  if (report.ok) {
    console.log(JSON.stringify(output, null, 2));
    return;
  }

  console.error(JSON.stringify(output, null, 2));
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
