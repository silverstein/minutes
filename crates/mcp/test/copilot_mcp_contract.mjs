#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const repoRoot = resolve(import.meta.dirname, "../../..");
const serverPath = join(repoRoot, "crates", "mcp", "dist", "index.js");
const minutesBin = join(repoRoot, "target", "debug", "minutes");
const tempHome = mkdtempSync(join(tmpdir(), "minutes-copilot-mcp-"));
mkdirSync(join(tempHome, ".minutes"), { recursive: true });

const stderrChunks = [];
const transport = new StdioClientTransport({
  command: process.execPath,
  args: [serverPath],
  cwd: repoRoot,
  env: {
    ...process.env,
    HOME: tempHome,
    USERPROFILE: tempHome,
    RUST_LOG: "error",
  },
  stderr: "pipe",
});
transport.stderr?.on("data", (chunk) => stderrChunks.push(String(chunk)));

const client = new Client(
  { name: "minutes-copilot-contract-test", version: "0.0.0" },
  { capabilities: {} }
);

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

try {
  await client.connect(transport);

  const tools = await client.listTools();
  const toolNames = new Set(tools.tools.map((tool) => tool.name));
  const manifest = JSON.parse(readFileSync(join(repoRoot, "manifest.json"), "utf8"));
  const manifestToolNames = new Set(manifest.tools.map((tool) => tool.name));
  assert(toolNames.size === manifestToolNames.size, "manifest and tools/list counts differ");
  for (const name of manifestToolNames) {
    assert(toolNames.has(name), `tools/list is missing manifest tool ${name}`);
  }
  for (const name of [
    "start_copilot",
    "stop_copilot",
    "copilot_status",
    "read_copilot_nudges",
  ]) {
    assert(toolNames.has(name), `tools/list is missing ${name}`);
  }

  const resources = await client.listResources();
  assert(
    resources.resources.some((resource) => resource.uri === "minutes://live/copilot"),
    "resources/list is missing minutes://live/copilot"
  );

  const status = await client.callTool({ name: "copilot_status", arguments: {} });
  assert(status.isError !== true, "inactive copilot_status must not be an error");
  assert(status.structuredContent?.active === false, "copilot_status must report active=false");
  assert(status.structuredContent?.state === "Off", "copilot_status must report state=Off");

  const nudges = await client.callTool({
    name: "read_copilot_nudges",
    arguments: { cursor: 0 },
  });
  assert(nudges.isError !== true, "inactive read_copilot_nudges must not be an error");
  assert(nudges.structuredContent?.active === false, "inactive nudge read must report active=false");
  assert(nudges.structuredContent?.nudges?.length === 0, "inactive nudge read must be empty");

  const stopped = await client.callTool({ name: "stop_copilot", arguments: {} });
  assert(stopped.isError !== true, "inactive stop_copilot must not be an error");
  assert(stopped.structuredContent?.active === false, "inactive stop must report active=false");

  const resource = await client.readResource({ uri: "minutes://live/copilot" });
  const payload = JSON.parse(resource.contents[0]?.text ?? "{}");
  assert(payload.active === false, "inactive live copilot resource must report active=false");
  assert(payload.state === "Off", "inactive live copilot resource must report state=Off");
  assert(payload.latest_nudge === null, "inactive live copilot resource must not expose a stale nudge");

  const started = await client.callTool({
    name: "start_copilot",
    arguments: { goal: "verify MCP control boundary", surface: "stdout" },
  });
  assert(started.isError !== true, "start_copilot must launch the real CLI engine");
  assert(started.structuredContent?.active === true, "start_copilot must observe active=true");
  assert(
    started.structuredContent?.nudge_stream?.attached === true,
    "start_copilot must attach the CLI observation stream"
  );

  const activeResource = await client.readResource({ uri: "minutes://live/copilot" });
  const activePayload = JSON.parse(activeResource.contents[0]?.text ?? "{}");
  assert(activePayload.active === true, "live copilot resource must observe the started engine");
  assert(activePayload.latest_nudge === null, "a new engine with no evidence must have no nudge");

  const stoppedActive = await client.callTool({ name: "stop_copilot", arguments: {} });
  assert(stoppedActive.isError !== true, "stop_copilot must stop the active CLI engine");
  assert(stoppedActive.structuredContent?.active === false, "stop_copilot must observe active=false");

  console.log("PASS: copilot MCP tools/resource registration and inactive degradation");
} catch (error) {
  console.error(`FAIL: ${error instanceof Error ? error.message : String(error)}`);
  const stderr = stderrChunks.join("").trim();
  if (stderr) console.error(stderr.slice(-4000));
  process.exitCode = 1;
} finally {
  try {
    execFileSync(minutesBin, ["copilot", "stop"], {
      env: { ...process.env, HOME: tempHome, USERPROFILE: tempHome, RUST_LOG: "error" },
      stdio: "ignore",
      timeout: 5000,
    });
  } catch {
    // Best-effort cleanup if the contract failed after starting the detached engine.
  }
  await new Promise((resolveWait) => setTimeout(resolveWait, 300));
  await client.close().catch(() => {});
  await transport.close().catch(() => {});
  rmSync(tempHome, { recursive: true, force: true });
}
