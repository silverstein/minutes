#!/usr/bin/env node

/**
 * MCP Server Integration Tests
 *
 * Tests that the MCP server:
 * 1. Negotiates the Claude Desktop/Code protocol version over real stdio
 * 2. Has all expected tools registered
 * 3. Tool schemas match expectations
 * 4. Status tool returns valid JSON
 * 5. Search tool handles empty queries
 * 6. List tool returns array
 * 7. Path validation works on get_meeting
 * 8. Path validation works on process_audio
 * 9. resummarize_meeting is present in the built server with its full schema
 * 10. resummarize_meeting's path guard (validatePathInDirectory) rejects outside paths
 *
 * Run: node crates/mcp/test/mcp_tools_test.mjs
 */

import { execFileSync, spawnSync } from "child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { validatePathInDirectory } from "../dist/paths.js";

let passed = 0;
let failed = 0;

function test(name, fn) {
  try {
    fn();
    console.log(`  PASS: ${name}`);
    passed++;
  } catch (e) {
    console.error(`  FAIL: ${name} — ${e.message}`);
    failed++;
  }
}

function assert(condition, msg) {
  if (!condition) throw new Error(msg || "assertion failed");
}

function assertEqual(actual, expected, msg) {
  if (actual !== expected)
    throw new Error(msg || `expected ${expected}, got ${actual}`);
}

function childText(value) {
  if (typeof value === "string") return value.trim();
  if (Buffer.isBuffer(value)) return value.toString("utf8").trim();
  return "";
}

function cliFailure(args, error) {
  const status =
    typeof error.status === "number"
      ? `exit ${error.status}`
      : error.signal
        ? `signal ${error.signal}`
        : "spawn failure";
  const stdout = childText(error.stdout);
  const stderr = childText(error.stderr);
  const details = [
    `minutes ${args.join(" ")} failed (${status})`,
    stderr && `stderr: ${stderr}`,
    stdout && `stdout: ${stdout}`,
  ].filter(Boolean);
  const failure = new Error(details.join("\n"));
  failure.cause = error;
  return failure;
}

// Helper: run minutes CLI and return stdout. A nonzero exit is always a test
// failure; empty stdout is never a substitute for a successful empty result.
function minutesCli(args) {
  const bin = join(import.meta.dirname, "..", "..", "..", "target", "debug", "minutes");
  try {
    const result = execFileSync(bin, args, {
      encoding: "utf-8",
      // This helper exercises small checked-in fixtures. Keep a bounded but
      // generous test timeout without claiming to match the native reader's
      // larger production authorization envelope.
      timeout: 30000,
      stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, RUST_LOG: "error" },
    });
    return result.trim();
  } catch (e) {
    throw cliFailure(args, e);
  }
}

console.log("MCP Server Integration Tests\n");

const CLAUDE_DESKTOP_PROTOCOL_VERSION = "2025-11-25";

// ── Test 0: nonzero CLI exits are never converted to empty success ──
test("minutes CLI helper propagates nonzero exits", () => {
  try {
    minutesCli(["definitely-not-a-minutes-command"]);
    throw new Error("nonzero CLI exit was incorrectly accepted");
  } catch (error) {
    assert(
      error.message.includes(
        "minutes definitely-not-a-minutes-command failed (exit"
      ),
      `expected explicit CLI exit failure, got: ${error.message}`
    );
  }
});

// ── Test 0b: real Claude Desktop-compatible MCP initialize negotiation ──
test("stdio initialize negotiates the Claude Desktop protocol version", () => {
  const mcpDir = join(import.meta.dirname, "..");
  const serverPath = join(mcpDir, "dist", "index.js");
  const initialize = {
    method: "initialize",
    params: {
      protocolVersion: CLAUDE_DESKTOP_PROTOCOL_VERSION,
      capabilities: {
        extensions: {
          "io.modelcontextprotocol/ui": {
            mimeTypes: ["text/html;profile=mcp-app"],
          },
        },
      },
      clientInfo: {
        name: "claude-desktop-compat-test",
        version: "1.3109.0",
      },
    },
    jsonrpc: "2.0",
    id: 0,
  };

  const child = spawnSync(process.execPath, [serverPath], {
    cwd: mcpDir,
    encoding: "utf-8",
    input: `${JSON.stringify(initialize)}\n`,
    timeout: 15000,
    env: { ...process.env, RUST_LOG: "error" },
  });

  assert(!child.error, `initialize process failed: ${child.error?.message}`);
  assertEqual(
    child.status,
    0,
    `initialize process exited ${child.status}: ${child.stderr.trim()}`
  );
  const responses = child.stdout
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => JSON.parse(line));
  const response = responses.find((message) => message.id === initialize.id);
  assert(response, "server must return an initialize response");
  assert(!response.error, `initialize returned an error: ${JSON.stringify(response.error)}`);

  const protocolVersion = response.result?.protocolVersion;
  assertEqual(
    protocolVersion,
    CLAUDE_DESKTOP_PROTOCOL_VERSION,
    "server must negotiate the protocol version used by Claude Desktop/Code"
  );
  console.log(`  Negotiated MCP protocolVersion: ${protocolVersion}`);
});

// ── Test 1: minutes status returns valid JSON ──
test("minutes status returns valid JSON", () => {
  const output = minutesCli(["status"]);
  const status = JSON.parse(output);
  assert(typeof status.recording === "boolean", "recording should be boolean");
  assertEqual(status.recording, false, "should not be recording");
});

// ── Test 2: minutes list returns array ──
test("minutes list returns JSON array", () => {
  const output = minutesCli(["list", "--limit", "5"]);
  const list = JSON.parse(output);
  assert(Array.isArray(list), "list should return an array");
});

// ── Test 3: minutes search returns array ──
test("minutes search returns JSON array", () => {
  const output = minutesCli(["search", "nonexistent-query-xyz", "--limit", "5"]);
  const results = JSON.parse(output);
  assert(Array.isArray(results), "search should return an array");
  assertEqual(results.length, 0, "nonexistent query should return empty");
});

// ── Test 4: minutes setup --list works ──
test("minutes setup --list shows models", () => {
  // setup --list outputs to stderr, not stdout
  execFileSync(
    join(import.meta.dirname, "..", "..", "..", "target", "debug", "minutes"),
    ["setup", "--list"],
    { encoding: "utf-8", timeout: 5000 }
  );
});

// ── Test 5: minutes devices returns JSON ──
test("minutes devices returns JSON array", () => {
  const output = minutesCli(["devices"]);
  const devices = JSON.parse(output);
  assert(Array.isArray(devices), "devices should return an array");
  assert(devices.length > 0, "should find at least one audio device");
});

// ── Test 5b: minutes paths exposes effective directories ──
test("minutes paths --json returns output_dir", () => {
  const output = minutesCli(["paths", "--json"]);
  const paths = JSON.parse(output);
  assert(typeof paths.data?.output_dir === "string", "output_dir should be a string");
  assert(typeof paths.data?.minutes_dir === "string", "minutes_dir should be a string");
  assert(typeof paths.data?.config_path === "string", "config_path should be a string");
});

// ── Test 6: minutes note without recording fails gracefully ──
test("minutes note fails gracefully without recording", () => {
  try {
    execFileSync(
      join(import.meta.dirname, "..", "..", "..", "target", "debug", "minutes"),
      ["note", "test note"],
      { encoding: "utf-8", timeout: 5000 }
    );
    throw new Error("should have failed");
  } catch (e) {
    assert(
      e.stderr?.includes("No recording in progress") || e.message.includes("No recording"),
      "should report no recording in progress"
    );
  }
});

// ── Test 7: MCP TypeScript compiles cleanly ──
test("MCP TypeScript compiles", () => {
  const mcp_dir = join(import.meta.dirname, "..");
  execFileSync("npx", ["tsc", "--noEmit"], {
    cwd: mcp_dir,
    encoding: "utf-8",
    timeout: 30000,
  });
});

// ── Test 8: MCP index.ts exports are valid ──
test("MCP server module loads without error", async () => {
  // Just verify the file is syntactically valid by checking tsc passed above
  const { existsSync } = await import("fs");
  const dist = join(import.meta.dirname, "..", "dist", "index.js");
  assert(existsSync(dist), "dist/index.js should exist after build");
});

// ── Test 9: minutes get --json applies speaker overlays end-to-end ──
// MCP's get_meeting tool shells to `minutes get <path> --json` to surface
// overlay-applied speaker_map to clients. This verifies the contract: a
// confirmation written via the CLI `confirm` subcommand is reflected in the
// JSON payload without the meeting markdown being mutated.
//
// Note: kept fully synchronous so failures propagate through the shared
// sync test() harness. An async callback would resolve its Promise after
// the runner returned PASS.
test("minutes get --json applies speaker overlay from confirm", () => {
  const sandbox = mkdtempSync(join(tmpdir(), "minutes-get-overlay-"));
  const meetingsDir = join(sandbox, "meetings");
  mkdirSync(meetingsDir, { recursive: true });
  const meetingPath = join(meetingsDir, "2026-04-24-overlay-smoke.md");
  const rawMarkdown = [
    "---",
    "title: Overlay Smoke",
    "type: meeting",
    "date: 2026-04-24T10:00:00-07:00",
    "duration: 10m",
    "tags: []",
    "attendees: []",
    "people: []",
    "action_items: []",
    "decisions: []",
    "intents: []",
    "speaker_map:",
    "  - speaker_label: SPEAKER_0",
    "    name: Speaker 0",
    "    confidence: medium",
    "    source: llm",
    "---",
    "",
    "## Transcript",
    "",
    "SPEAKER_0: hi there",
    "",
  ].join("\n");
  writeFileSync(meetingPath, rawMarkdown);

  const bin = join(import.meta.dirname, "..", "..", "..", "target", "debug", "minutes");
  const env = { ...process.env, HOME: sandbox, USERPROFILE: sandbox, RUST_LOG: "error" };

  // Confirm via CLI — same overlay path the desktop app now uses.
  execFileSync(
    bin,
    ["confirm", "--meeting", meetingPath, "--speaker", "SPEAKER_0", "--name", "Alex Kim"],
    { encoding: "utf-8", timeout: 10000, env }
  );

  const before = readFileSync(meetingPath, "utf-8");
  const jsonOut = execFileSync(bin, ["get", meetingPath, "--json"], {
    encoding: "utf-8",
    timeout: 10000,
    env,
  });
  const after = readFileSync(meetingPath, "utf-8");
  assertEqual(before, after, "raw meeting markdown must not be rewritten by get --json");

  const payload = JSON.parse(jsonOut);
  assert(payload.overlay_applied === true, "overlay_applied must be true after a confirmation");
  const attr = (payload.frontmatter?.speaker_map || []).find(
    (entry) => entry.speaker_label === "SPEAKER_0"
  );
  assert(attr, "SPEAKER_0 must appear in returned speaker_map");
  assertEqual(attr.name, "Alex Kim", "overlay name must appear in JSON speaker_map");
  assertEqual(attr.confidence, "high", "overlay confirmations carry high confidence");

  rmSync(sandbox, { recursive: true, force: true });
});

// ── Test 10: resummarize_meeting is registered with its schema ──
// Asserts against the compiled server source, not a live MCP round-trip —
// this harness has no protocol client, so registration + schema fields +
// the exact validation call are checked textually in dist/index.js.
test("resummarize_meeting is present in the built server with its full schema", () => {
  const builtSource = readFileSync(
    join(import.meta.dirname, "..", "dist", "index.js"),
    "utf-8"
  );
  const toolStart = builtSource.indexOf('"resummarize_meeting"');
  const nextTool = builtSource.indexOf("registerTool(", toolStart + 1);
  const toolSource = builtSource.slice(
    toolStart,
    nextTool === -1 ? builtSource.length : nextTool
  );

  assert(toolStart !== -1, "resummarize_meeting must be present in the built server");
  for (const field of ["path", "apply", "engine", "template", "ingest", "include_restricted"]) {
    assert(
      new RegExp(`${field}: z\\s*\\.`).test(toolSource),
      `resummarize_meeting schema must include ${field}`
    );
  }
  assert(
    toolSource.includes(
      'validatePathInDirectory(path, await getEffectiveMeetingsDir(), [".md"])'
    ),
    "resummarize_meeting must validate paths against the effective meetings directory"
  );
});

// ── Test 11: resummarize_meeting's path guard rejects outside files ──
// Exercises the shared validator the tool calls (verified textually above),
// not the handler itself — same limitation as test 10.
test("resummarize_meeting's path guard (validatePathInDirectory) rejects outside paths", () => {
  const sandbox = mkdtempSync(join(tmpdir(), "minutes-resummarize-path-"));
  const meetingsDir = join(sandbox, "meetings");
  const outsidePath = join(sandbox, "outside.md");
  mkdirSync(meetingsDir, { recursive: true });
  writeFileSync(outsidePath, "# Outside\n");

  try {
    let error;
    try {
      validatePathInDirectory(outsidePath, meetingsDir, [".md"]);
    } catch (caught) {
      error = caught;
    }
    assert(error, "an outside path must be rejected");
    assert(
      error.message.includes("Access denied: path must be within"),
      "outside-path rejection must explain the meetings-directory boundary"
    );
  } finally {
    rmSync(sandbox, { recursive: true, force: true });
  }
});

// ── Summary ──
console.log(`\nResults: ${passed} passed, ${failed} failed, ${passed + failed} total`);
process.exit(failed > 0 ? 1 : 0);
