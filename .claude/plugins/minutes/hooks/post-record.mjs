#!/usr/bin/env node

/**
 * PostToolUse hook: auto-tag meetings with current project context.
 *
 * When `minutes stop` or `minutes process` completes (detected by Bash tool
 * running a minutes command), this hook reads the last-result.json and adds
 * the current git repo name as a `project:` tag in the meeting's frontmatter.
 *
 * This connects meetings to the codebase the user was working on when they
 * recorded — enabling queries like "what meetings relate to my-project?"
 *
 * Hook event: PostToolUse
 * Tool: Bash
 * Matcher: minutes stop|minutes process
 */

import { execFileSync } from "child_process";
import { readFileSync, writeFileSync, existsSync } from "fs";
import { join } from "path";
import { homedir } from "os";

// Check if this was a minutes command
const input = JSON.parse(process.argv[2] || "{}");
const toolName = input.tool_name || "";
const toolInput = input.tool_input || {};

if (toolName !== "Bash") process.exit(0);

const command = toolInput.command || "";
if (!command.includes("minutes stop") && !command.includes("minutes process")) {
  process.exit(0);
}

// Get the current git repo name
let projectName = null;
try {
  projectName = execFileSync("git", ["rev-parse", "--show-toplevel"], {
    encoding: "utf-8",
    timeout: 5000,
  })
    .trim()
    .split("/")
    .pop();
} catch {
  // Not in a git repo — that's fine
  process.exit(0);
}

if (!projectName) process.exit(0);

// Find the most recently modified meeting file
const lastResult = join(homedir(), ".minutes", "last-result.json");
if (!existsSync(lastResult)) process.exit(0);

try {
  const result = JSON.parse(readFileSync(lastResult, "utf-8"));
  const meetingPath = result.file;

  if (!meetingPath || !existsSync(meetingPath)) process.exit(0);

  // Read the meeting file and add project tag
  let content = readFileSync(meetingPath, "utf-8");

  // Check if project tag already exists
  if (content.includes(`project: ${projectName}`)) process.exit(0);

  // Add project field to frontmatter
  if (content.startsWith("---")) {
    const endIdx = content.indexOf("\n---", 3);
    if (endIdx > 0) {
      const before = content.slice(0, endIdx);
      const after = content.slice(endIdx);
      content = `${before}\nproject: ${projectName}${after}`;
      writeFileSync(meetingPath, content, { mode: 0o600 });
    }
  }
} catch {
  // Silently fail — hook errors shouldn't block the user
}
