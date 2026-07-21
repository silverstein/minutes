#!/usr/bin/env node

import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { defaultFixturePath } from "../tests/eval/sidekick_rehearsal_golden.mjs";

function parseArgs(argv) {
  const options = {
    fixture: defaultFixturePath,
    workspace: path.join(os.homedir(), ".minutes", "assistant"),
  };
  for (let index = 2; index < argv.length; index += 1) {
    if (argv[index] === "--fixture") options.fixture = argv[++index];
    else if (argv[index] === "--workspace") options.workspace = argv[++index];
    else throw new Error(`unknown argument: ${argv[index]}`);
  }
  return options;
}

function briefMarkdown(fixture) {
  const context = fixture.prepared_context;
  return `# Sidekick brief\n\nThis is user-authored prepared context for a synthetic demo. It is context, not observed meeting evidence.\n\n- User role: ${context.user_role}\n- Assistance posture: ${context.posture}\n- Goal: ${context.demo_goal}\n- Known facts:\n${context.known_facts.map((fact) => `  - ${fact}`).join("\n")}\n`;
}

function fallbackMarkdown(fixture) {
  const transcript = fixture.transcript
    .map((item) => `${item.speaker}: ${item.text}`)
    .join("\n\n");
  return `# Synthetic rehearsal fallback\n\nDo not read or use this as meeting evidence unless the user explicitly says: "Use the Meridian fallback transcript."\n\n${transcript}\n\n## Typed turns\n\n1. ${fixture.turns[0].typed_prompt}\n2. ${fixture.turns[1].typed_prompt}\n`;
}

const options = parseArgs(process.argv);
const fixture = JSON.parse(await fs.readFile(options.fixture, "utf8"));
if (fixture.schema_version !== 1 || fixture.content_origin !== "synthetic") {
  throw new Error("rehearsal preparation accepts only schema-v1 synthetic fixtures");
}

await fs.mkdir(options.workspace, { recursive: true });
const briefPath = path.join(options.workspace, "SIDEKICK_BRIEF.md");
const fallbackPath = path.join(options.workspace, "MERIDIAN_FALLBACK_TRANSCRIPT.md");
await fs.writeFile(briefPath, briefMarkdown(fixture), { mode: 0o600 });
await fs.writeFile(fallbackPath, fallbackMarkdown(fixture), { mode: 0o600 });
process.stdout.write(
  `${JSON.stringify({ fixture_id: fixture.id, brief: briefPath, fallback: fallbackPath })}\n`,
);

