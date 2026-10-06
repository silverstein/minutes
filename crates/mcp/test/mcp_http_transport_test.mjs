#!/usr/bin/env node

/**
 * MCP HTTP Transport Integration Tests
 *
 * Starts the built server in `--transport http` mode and drives it over real
 * HTTP. Covers:
 *  1. initialize + tools/list round trip via the MCP SDK client
 *  2. the HTTP surface matches stdio (tools, resources) — the
 *     per-session server factory is what could silently drop registrations,
 *     so parity against a stdio round trip is the check that matters. Eight
 *     tools and two resources sit behind CLI capability gates, and the two
 *     processes probe those independently, so the comparison is split: core
 *     parity is exact between processes, and the gated part is checked
 *     against each process's own declared probe outcome. See
 *     test/lib/surface-parity.mjs for why, and test/surface_parity_test.mjs
 *     for the proof that this still fails on a real mismatch
 *  3. two concurrent sessions in one process, each with its own session id
 *  4. session teardown releases server-side state
 *  4b. overlapping tool *execution* from two sessions against the shared
 *     module scope: results must equal single-session baselines exactly, so a
 *     response delivered to the wrong session fails rather than passing as
 *     "it returned something". See the test's own comment for what this
 *     separates from corpus memory admission and CLI availability caching)
 *  5. localhost hardening: cross-origin POSTs, non-JSON bodies, unknown
 *     session ids, and non-initialize requests without a session are refused
 *  6. --help lists the transport flags and exits 0
 *
 * Requires the debug CLI at target/debug/minutes (the server refuses to start
 * without it, on either transport).
 *
 * Run: node crates/mcp/test/mcp_http_transport_test.mjs
 */

import { spawn } from "child_process";
import { join } from "path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import {
  CAPABILITY_GATED_RESOURCES,
  CAPABILITY_GATED_TOOLS,
  assertCoreParity,
  assertGateMapMatchesSource,
  assertOwnStateGatedSurface,
  assertSameCli,
  capabilityStatesAgree,
  describeStateDivergence,
  probeCapabilityReport,
  readDeclaredCapabilityState,
  resolveMinutesBinary,
} from "./lib/surface-parity.mjs";

const MCP_DIR = join(import.meta.dirname, "..");
const SERVER_ENTRY = join(MCP_DIR, "dist", "index.js");
const STARTUP_TIMEOUT_MS = 30000;

let passed = 0;
let failed = 0;

// Async-aware runner. The sibling suite (mcp_tools_test.mjs) is deliberately
// synchronous; reusing its helper here would resolve the callback's promise
// after PASS was already printed, swallowing every failure.
const pending = [];
function test(name, fn) {
  pending.push({ name, fn });
}

async function runTests() {
  for (const { name, fn } of pending) {
    try {
      await fn();
      console.log(`  PASS: ${name}`);
      passed++;
    } catch (e) {
      console.error(`  FAIL: ${name} — ${e.message}`);
      failed++;
    }
  }
}

function assert(condition, msg) {
  if (!condition) throw new Error(msg || "assertion failed");
}

function assertEqual(actual, expected, msg) {
  if (actual !== expected)
    throw new Error(msg || `expected ${expected}, got ${actual}`);
}

/** Start the server on an OS-assigned port and read the port back from stderr. */
function startHttpServer() {
  const child = spawn(process.execPath, [SERVER_ENTRY, "--transport", "http", "--port", "0"], {
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, RUST_LOG: "error" },
  });

  return new Promise((resolvePromise, rejectPromise) => {
    let stderr = "";
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      rejectPromise(
        new Error(`server did not report a listening port in ${STARTUP_TIMEOUT_MS}ms:\n${stderr}`)
      );
    }, STARTUP_TIMEOUT_MS);

    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
      const match = stderr.match(/listening on (http:\/\/[^\s]+)/);
      if (match) {
        clearTimeout(timer);
        resolvePromise({ child, url: match[1], stderrSoFar: () => stderr });
      }
    });
    child.once("exit", (code) => {
      clearTimeout(timer);
      rejectPromise(new Error(`server exited early (code ${code}):\n${stderr}`));
    });
  });
}

async function stopChild(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  await new Promise((resolvePromise) => {
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      resolvePromise();
    }, 5000);
    child.once("exit", () => {
      clearTimeout(timer);
      resolvePromise();
    });
  });
}

async function connectHttpClient(url, name) {
  const client = new Client({ name, version: "1.0.0" });
  await client.connect(new StreamableHTTPClientTransport(new URL(url)));
  return client;
}

/** POST a raw JSON-RPC body, bypassing the SDK client, to probe the guards. */
async function rawPost(url, { body, headers = {}, method = "POST" } = {}) {
  const response = await fetch(url, {
    method,
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json, text/event-stream",
      ...headers,
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  return { status: response.status, text };
}

const INITIALIZE_BODY = {
  jsonrpc: "2.0",
  id: 1,
  method: "initialize",
  params: {
    protocolVersion: "2025-06-18",
    capabilities: {},
    clientInfo: { name: "raw-probe", version: "1.0.0" },
  },
};

console.log("MCP HTTP Transport Integration Tests\n");

// Every capability gate in index.ts must be modelled by the parity helper.
// An unmodelled gate would land in the "core" set and make the cross-process
// comparison timing-dependent again, so fail before spawning anything.
assertGateMapMatchesSource();

// Warm the binary the *server* will resolve — release is tried before debug,
// so warming target/debug unconditionally can warm a file nothing will run —
// and read its capability report with a budget the server's 2s probe does not
// have. Warming shrinks the odds of a child's probe timing out; it does not
// eliminate them, which is why the parity assertions below never depend on
// the two children agreeing.
const MINUTES_BIN = resolveMinutesBinary();
const CAPABILITY_REPORT = probeCapabilityReport(MINUTES_BIN);
const TEST_START_ISO = new Date().toISOString();
console.log(
  `CLI: ${MINUTES_BIN}\nCapability report: ${
    CAPABILITY_REPORT
      ? `v${CAPABILITY_REPORT.version} api${CAPABILITY_REPORT.api_version}, ${
          Object.keys(CAPABILITY_REPORT.features).length
        } features`
      : "unavailable (older CLI)"
  }\n`
);

const server = await startHttpServer();
const baseUrl = server.url.replace(/\/mcp$/, "");

let httpClient;
let httpTools;
let httpResources;

try {
  test("initialize + tools/list round trip over HTTP", async () => {
    httpClient = await connectHttpClient(server.url, "http-primary");
    const info = httpClient.getServerVersion();
    assertEqual(info?.name, "minutes", "server name should be minutes");

    httpTools = (await httpClient.listTools()).tools;
    assert(Array.isArray(httpTools), "tools should be an array");
    assert(httpTools.length > 0, "server should expose tools over HTTP");

    httpResources = (await httpClient.listResources()).resources;
    assert(Array.isArray(httpResources), "resources should be an array");
  });

  test("HTTP surface matches stdio, per each process's own capability state", async () => {
    const stdioClient = new Client({ name: "stdio-parity", version: "1.0.0" });
    const stdioTransport = new StdioClientTransport({
      command: process.execPath,
      args: [SERVER_ENTRY],
      env: { ...process.env, RUST_LOG: "error" },
      stderr: "ignore",
    });
    await stdioClient.connect(stdioTransport);
    try {
      const stdioTools = (await stdioClient.listTools()).tools.map((t) => t.name);
      const stdioResources = (await stdioClient.listResources()).resources.map((r) => r.uri);
      const overHttpTools = httpTools.map((t) => t.name);
      const overHttpResources = httpResources.map((r) => r.uri);

      // What each process's own probe decided, read from the line that
      // process wrote with its own pid. This is the witness that separates
      // "the probe timed out here" from "the factory dropped registrations",
      // which are indistinguishable from the advertised surface alone.
      const httpState = await readDeclaredCapabilityState({
        pid: server.child.pid,
        sinceIso: TEST_START_ISO,
        label: "http server",
      });
      const stdioState = await readDeclaredCapabilityState({
        pid: stdioTransport.pid,
        sinceIso: TEST_START_ISO,
        label: "stdio server",
      });
      assertSameCli("http server", httpState, CAPABILITY_REPORT);
      assertSameCli("stdio server", stdioState, CAPABILITY_REPORT);

      // 1. Core parity: exact, between processes. No capability gate can move
      //    a core name, so this comparison is timing-independent.
      const coreTools = assertCoreParity({
        label: "tool surface",
        aLabel: "http",
        aNames: overHttpTools,
        bLabel: "stdio",
        bNames: stdioTools,
        gatedMap: CAPABILITY_GATED_TOOLS,
        requiredAnchors: [
          "get_status",
          "list_meetings",
          "resummarize_meeting",
          "knowledge_status",
        ],
      });
      assertCoreParity({
        label: "resource surface",
        aLabel: "http",
        aNames: overHttpResources,
        bLabel: "stdio",
        bNames: stdioResources,
        gatedMap: CAPABILITY_GATED_RESOURCES,
        requiredAnchors: ["minutes://status", "minutes://meetings/recent"],
      });

      // 2. Gated surface: each process against its own declared state. This is
      //    what keeps the eight optional tools covered rather than merely
      //    excluded — a tool the factory replay dropped is missing from a
      //    process whose own probe says it should be present, and that fails
      //    here even though it never disturbs core parity.
      const httpGated = assertOwnStateGatedSurface({
        label: "http tools",
        names: overHttpTools,
        gatedMap: CAPABILITY_GATED_TOOLS,
        state: httpState,
        report: CAPABILITY_REPORT,
      });
      assertOwnStateGatedSurface({
        label: "stdio tools",
        names: stdioTools,
        gatedMap: CAPABILITY_GATED_TOOLS,
        state: stdioState,
        report: CAPABILITY_REPORT,
      });
      assertOwnStateGatedSurface({
        label: "http resources",
        names: overHttpResources,
        gatedMap: CAPABILITY_GATED_RESOURCES,
        state: httpState,
        report: CAPABILITY_REPORT,
      });
      assertOwnStateGatedSurface({
        label: "stdio resources",
        names: stdioResources,
        gatedMap: CAPABILITY_GATED_RESOURCES,
        state: stdioState,
        report: CAPABILITY_REPORT,
      });

      // Together, 1 and 2 pin each process's complete advertised surface:
      // core is identical across processes, and every gated name is exactly
      // what that process's own probe predicts. Nothing can silently vanish
      // over HTTP — it would drop out of core, or out of its own state's
      // expected set.
      if (capabilityStatesAgree(httpState, stdioState)) {
        assertEqual(
          [...overHttpTools].sort().join(","),
          [...stdioTools].sort().join(","),
          "with identical capability states the full tool surfaces must match"
        );
        assertEqual(
          [...overHttpResources].sort().join(","),
          [...stdioResources].sort().join(","),
          "with identical capability states the full resource surfaces must match"
        );
      } else {
        console.log(`  ${describeStateDivergence("http", httpState, "stdio", stdioState)}`);
      }

      console.log(
        `  (core tools ${coreTools.length}, gated ${httpGated.length}/${Object.keys(CAPABILITY_GATED_TOOLS).length}, ` +
          `states http=${httpState.kind} stdio=${stdioState.kind})`
      );
    } finally {
      await stdioClient.close();
    }
  });

  test("two concurrent clients get independent sessions", async () => {
    const second = await connectHttpClient(server.url, "http-secondary");
    try {
      const secondTools = (await second.listTools()).tools;
      assertEqual(
        secondTools.length,
        httpTools.length,
        "a second session should see the same tool count"
      );
      assert(
        second.transport.sessionId &&
          second.transport.sessionId !== httpClient.transport.sessionId,
        "each client should get its own session id"
      );
      const health = await rawPost(`${baseUrl}/health`, { method: "GET" });
      const parsed = JSON.parse(health.text);
      assertEqual(parsed.sessions, 2, "server should report two live sessions");
    } finally {
      // close() only tears down the client side; DELETE is what ends the
      // server-side session.
      await second.transport.terminateSession();
      await second.close();
    }
  });

  test("closing a session releases it server-side", async () => {
    // Teardown is driven by the transport's onclose, so poll rather than
    // assume the DELETE response and the server-side cleanup are ordered.
    let sessions = -1;
    for (let attempt = 0; attempt < 20; attempt++) {
      const health = await rawPost(`${baseUrl}/health`, { method: "GET" });
      sessions = JSON.parse(health.text).sessions;
      if (sessions === 1) break;
      await new Promise((r) => setTimeout(r, 100));
    }
    assertEqual(sessions, 1, "only the primary session should remain");
  });

  test("get_status is callable over HTTP", async () => {
    const result = await httpClient.callTool({ name: "get_status", arguments: {} });
    assert(Array.isArray(result.content), "tool result should carry content");
    assert(result.content.length > 0, "get_status should return text content");
  });

  // Each session must get its own tool's response, including when both clients
  // reuse JSON-RPC request IDs. Keep one corpus read in flight alongside a local
  // status call: a default corpus reservation alone exceeds 128 MiB, so the
  // 256 MiB process budget correctly refuses a second simultaneous corpus read.
  // Exact baselines in both directions detect swapped responses without asking
  // the transport test to bypass that admission boundary. Memory reservation
  // and refusal behavior are covered by corpus-lease.test.ts.
  test("overlapping tool execution from two sessions stays independent", async () => {
    const CORPUS_TOOL = "list_meetings"; // routes through withStableCorpusLease
    const LOCAL_TOOL = "get_status"; // no corpus lease

    const corpusBaseline = JSON.stringify(
      await httpClient.callTool({ name: CORPUS_TOOL, arguments: {} })
    );
    const localBaseline = JSON.stringify(
      await httpClient.callTool({ name: LOCAL_TOOL, arguments: {} })
    );
    assert(
      corpusBaseline !== localBaseline,
      "the two baseline tools must be distinguishable, or a swapped response would pass"
    );

    const second = await connectHttpClient(server.url, "http-concurrent");
    try {
      for (let round = 1; round <= 3; round++) {
        for (const reverse of [false, true]) {
          const primaryTool = reverse ? CORPUS_TOOL : LOCAL_TOOL;
          const secondaryTool = reverse ? LOCAL_TOOL : CORPUS_TOOL;
          const [fromPrimary, fromSecondary] = await Promise.all([
            httpClient.callTool({ name: primaryTool, arguments: {} }),
            second.callTool({ name: secondaryTool, arguments: {} }),
          ]);
          assertEqual(
            JSON.stringify(fromPrimary),
            reverse ? corpusBaseline : localBaseline,
            `round ${round}: primary session asked for ${primaryTool} and did not get its response`
          );
          assertEqual(
            JSON.stringify(fromSecondary),
            reverse ? localBaseline : corpusBaseline,
            `round ${round}: secondary session asked for ${secondaryTool} and did not get its response`
          );
        }
      }
    } finally {
      await second.transport.terminateSession();
      await second.close();
    }
  });

  test("cross-origin POST is rejected", async () => {
    const { status, text } = await rawPost(server.url, {
      body: INITIALIZE_BODY,
      headers: { Origin: "http://evil.example" },
    });
    assertEqual(status, 403, "a non-loopback Origin must be refused");
    assert(text.includes("cross-origin"), `expected a cross-origin error, got: ${text}`);
  });

  test("loopback Origin is accepted", async () => {
    const { status } = await rawPost(server.url, {
      body: INITIALIZE_BODY,
      headers: { Origin: "http://localhost:5173" },
    });
    assertEqual(status, 200, "a loopback Origin must still be served");
  });

  test("non-JSON content type is rejected", async () => {
    const response = await fetch(server.url, {
      method: "POST",
      headers: { "Content-Type": "text/plain", Accept: "application/json, text/event-stream" },
      body: JSON.stringify(INITIALIZE_BODY),
    });
    assertEqual(response.status, 415, "text/plain POSTs must be refused");
  });

  test("unknown session id is rejected", async () => {
    const { status } = await rawPost(server.url, {
      body: { jsonrpc: "2.0", id: 2, method: "tools/list", params: {} },
      headers: { "Mcp-Session-Id": "00000000-0000-0000-0000-000000000000" },
    });
    assertEqual(status, 404, "an unknown session id must be refused");
  });

  test("non-initialize request without a session is rejected", async () => {
    const { status, text } = await rawPost(server.url, {
      body: { jsonrpc: "2.0", id: 3, method: "tools/list", params: {} },
    });
    assertEqual(status, 400, "a sessionless tools/list must be refused");
    assert(text.includes("Mcp-Session-Id"), `expected a session hint, got: ${text}`);
  });

  test("unknown path is not served", async () => {
    const { status } = await rawPost(`${baseUrl}/`, { method: "GET" });
    assertEqual(status, 404, "only /mcp and /health are served");
  });

  test("--help documents the transport flags and exits 0", async () => {
    const { code, stdout } = await new Promise((resolvePromise) => {
      const child = spawn(process.execPath, [SERVER_ENTRY, "--help"], {
        stdio: ["ignore", "pipe", "pipe"],
      });
      let out = "";
      child.stdout.on("data", (chunk) => (out += chunk.toString()));
      child.once("exit", (exitCode) => resolvePromise({ code: exitCode, stdout: out }));
    });
    assertEqual(code, 0, "--help should exit 0");
    for (const fragment of ["--transport <stdio|http>", "--port", "127.0.0.1"]) {
      assert(stdout.includes(fragment), `--help output should mention ${fragment}`);
    }
    assert(
      !stdout.includes("--host"),
      "--help should not advertise a bind-address flag"
    );
  });

  await runTests();
} finally {
  if (httpClient) {
    try {
      await httpClient.close();
    } catch {
      // The server may already be gone; teardown is best-effort.
    }
  }
  await stopChild(server.child);
}

console.log(`\nResults: ${passed} passed, ${failed} failed, ${passed + failed} total`);
process.exit(failed > 0 ? 1 : 0);
