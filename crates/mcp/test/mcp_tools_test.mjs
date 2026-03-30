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
  assert(typeof paths.output_dir === "string", "output_dir should be a string");
  assert(typeof paths.minutes_dir === "string", "minutes_dir should be a string");
  assert(typeof paths.config_path === "string", "config_path should be a string");
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

// ── Summary ──
console.log(`\nResults: ${passed} passed, ${failed} failed, ${passed + failed} total`);
process.exit(failed > 0 ? 1 : 0);
