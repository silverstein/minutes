import assert from "node:assert/strict";
import test from "node:test";
import process from "node:process";
import { once } from "node:events";
import {
  CodexAppServerClient,
  mcpDisableArgsFromConfig,
} from "../lib/codex_app_server.mjs";

const fakeServer = String.raw`
const readline = require("node:readline");
const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const message = JSON.parse(line);
  if (message.method === "initialize") {
    send({ id: message.id, result: { userAgent: "fake" } });
  } else if (message.method === "thread/start") {
    send({ id: message.id, result: { thread: { id: "thread-1" }, model: "fake-model", serviceTier: "priority" } });
  } else if (message.method === "turn/start") {
    send({ id: message.id, result: { turn: { id: "turn-1", status: "inProgress" } } });
    send({ method: "item/agentMessage/delta", params: { threadId: "thread-1", turnId: "turn-1", itemId: "item-1", delta: "{\"decision\":" } });
    send({ method: "item/agentMessage/delta", params: { threadId: "thread-1", turnId: "turn-1", itemId: "item-1", delta: "\"silent\"}" } });
    send({ method: "turn/completed", params: { threadId: "thread-1", turn: { id: "turn-1", status: "completed", error: null } } });
  } else if (message.method === "turn/steer") {
    send({ id: message.id, result: { turnId: message.params.expectedTurnId } });
  } else if (message.method === "turn/interrupt") {
    send({ id: message.id, result: {} });
  }
});
function send(value) { process.stdout.write(JSON.stringify(value) + "\n"); }
`;

test("correlates app-server requests and assembles streamed turn output", async (t) => {
  const client = new CodexAppServerClient({
    command: process.execPath,
    args: ["-e", fakeServer],
    requestTimeoutMs: 2_000,
  });
  t.after(() => client.close());

  const initialized = await client.start();
  assert.equal(initialized.userAgent, "fake");

  const { threadId, result } = await client.startThread({ ephemeral: true });
  assert.equal(threadId, "thread-1");
  assert.equal(result.model, "fake-model");

  const turn = await client.runTurn({ threadId, input: "evidence" });
  assert.equal(turn.status, "completed");
  assert.equal(turn.text, '{"decision":"silent"}');
  assert.ok(turn.firstDeltaMs >= 0);
  assert.ok(turn.totalMs >= turn.firstDeltaMs);

  const steered = await client.steerTurn({
    threadId,
    turnId: turn.turnId,
    input: "new evidence",
  });
  assert.equal(steered.turnId, "turn-1");
  await client.interruptTurn({ threadId, turnId: turn.turnId });
});

test("exposes a nonblocking turn handle so foreground work can preempt background work", async (t) => {
  const client = new CodexAppServerClient({
    command: process.execPath,
    args: ["-e", fakeServer],
    requestTimeoutMs: 2_000,
    experimentalApi: true,
  });
  t.after(() => client.close());

  await client.start();
  const { threadId } = await client.startThread({ ephemeral: true });
  const deltas = [];
  client.on("turn-delta", (event) => deltas.push(event.delta));

  const active = await client.startTurn({ threadId, input: "evidence" });
  assert.equal(active.turnId, "turn-1");
  const completed = await active.completion;
  assert.equal(completed.text, '{"decision":"silent"}');
  assert.deepEqual(deltas, ['{"decision":', '"silent"}']);
});

test("fails closed when the server emits invalid JSON and exits", async () => {
  const client = new CodexAppServerClient({
    command: process.execPath,
    args: ["-e", 'process.stdout.write("not-json\\n"); setTimeout(() => process.exit(3), 5);'],
    requestTimeoutMs: 500,
  });
  let warning = false;
  client.on("protocol-warning", () => {
    warning = true;
  });
  await assert.rejects(client.start(), /exited/);
  assert.equal(warning, true);
});

test("builds deterministic disable overrides for every configured MCP server", () => {
  assert.deepEqual(
    mcpDisableArgsFromConfig(`
[mcp_servers.minutes]
command = "minutes-mcp"
[mcp_servers.google_workspace.env]
TOKEN = "ignored"
[mcp_servers."odd.name"]
url = "https://example.invalid"
[mcp_servers.minutes]
command = "duplicate"
`),
    [
      "--config",
      "mcp_servers.google_workspace.enabled=false",
      "--config",
      "mcp_servers.minutes.enabled=false",
      "--config",
      'mcp_servers."odd.name".enabled=false',
    ],
  );
});

test("close escalates a wedged app-server and does not retain its stdio handles", async () => {
const stubbornServer = String.raw`
process.on("SIGTERM", () => {});
setInterval(() => {}, 1000);
const readline = require("node:readline");
readline.createInterface({ input: process.stdin }).on("line", (line) => {
  const message = JSON.parse(line);
  if (message.method === "initialize") {
    process.stdout.write(JSON.stringify({ id: message.id, result: { userAgent: "stubborn" } }) + "\n");
  }
});
`;
  const client = new CodexAppServerClient({
    command: process.execPath,
    args: ["-e", stubbornServer],
    requestTimeoutMs: 500,
    closeGraceMs: 20,
  });
  await client.start();
  const child = client.child;
  const exited = once(child, "exit");
  client.close();
  let deadline;
  const [, signal] = await Promise.race([
    exited,
    new Promise((_, reject) => {
      deadline = setTimeout(
        () => reject(new Error("wedged app-server did not exit")),
        500,
      );
    }),
  ]);
  clearTimeout(deadline);
  assert.equal(signal, "SIGKILL");
  assert.equal(child.stdin.destroyed, true);
  assert.equal(child.stdout.destroyed, true);
  assert.equal(child.stderr.destroyed, true);
});
