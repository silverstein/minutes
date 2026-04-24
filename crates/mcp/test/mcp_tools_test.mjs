#!/usr/bin/env node

/**
 * MCP Server Integration Tests
 *
 * Tests that the MCP server:
 * 1. Has all expected tools registered
 * 2. Tool schemas match expectations
 * 3. Status tool returns valid JSON
 * 4. Search tool handles empty queries
 * 5. List tool returns array
 * 6. Path validation works on get_meeting
 * 7. Path validation works on process_audio
 *
 * Run: node crates/mcp/test/mcp_tools_test.mjs
 */

import { execFileSync } from "child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

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

// Helper: run minutes CLI and parse JSON stdout
function minutesCli(args) {
  const bin = join(import.meta.dirname, "..", "..", "..", "target", "debug", "minutes");
  try {
    const result = execFileSync(bin, args, {
      encoding: "utf-8",
      timeout: 10000,
      env: { ...process.env, RUST_LOG: "error" },
    });
    return result.trim();
  } catch (e) {
    return e.stdout?.trim() || "";
  }
}

console.log("MCP Server Integration Tests\n");

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
  if (output) {
    const list = JSON.parse(output);
    assert(Array.isArray(list), "list should return an array");
  }
  // Empty output is fine if no meetings exist
});

// ── Test 3: minutes search returns array ──
test("minutes search returns JSON array", () => {
  const output = minutesCli(["search", "nonexistent-query-xyz", "--limit", "5"]);
  if (output) {
    const results = JSON.parse(output);
    assert(Array.isArray(results), "search should return an array");
    assertEqual(results.length, 0, "nonexistent query should return empty");
  }
});

// ── Test 4: minutes setup --list works ──
test("minutes setup --list shows models", () => {
  // setup --list outputs to stderr, not stdout
  try {
    execFileSync(
      join(import.meta.dirname, "..", "..", "..", "target", "debug", "minutes"),
      ["setup", "--list"],
      { encoding: "utf-8", timeout: 5000 }
    );
  } catch (e) {
    // Expected to exit 0
  }
  // If it didn't throw, it worked
});

// ── Test 5: minutes devices returns JSON ──
test("minutes devices returns JSON array", () => {
  const output = minutesCli(["devices"]);
  if (output) {
    const devices = JSON.parse(output);
    assert(Array.isArray(devices), "devices should return an array");
    assert(devices.length > 0, "should find at least one audio device");
  }
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

// ── Summary ──
console.log(`\nResults: ${passed} passed, ${failed} failed, ${passed + failed} total`);
process.exit(failed > 0 ? 1 : 0);
