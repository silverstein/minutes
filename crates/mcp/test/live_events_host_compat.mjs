#!/usr/bin/env node
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { ResourceUpdatedNotificationSchema } from "@modelcontextprotocol/sdk/types.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, "../../..");
const defaultServer = {
  command: "node",
  args: [join(repoRoot, "crates", "mcp", "dist", "index.js")],
};
const LIVE_URI = "minutes://events/live";

function loadClaudeDesktopMinutesConfig() {
  const path = join(homedir(), "Library", "Application Support", "Claude", "claude_desktop_config.json");
  if (!existsSync(path)) {
    return { status: "skipped", reason: `${path} does not exist` };
  }
  const config = JSON.parse(readFileSync(path, "utf8"));
  const server = config?.mcpServers?.minutes;
  if (!server?.command) {
    return { status: "skipped", reason: "Claude Desktop config has no mcpServers.minutes command" };
  }
  return { status: "ready", command: server.command, args: server.args ?? [] };
}

function loadCodexMinutesConfig() {
  const path = join(homedir(), ".codex", "config.toml");
  if (!existsSync(path)) {
    return { status: "skipped", reason: `${path} does not exist` };
  }
  const raw = readFileSync(path, "utf8");
  const section = raw.match(/\[mcp_servers\.minutes\]([\s\S]*?)(?=\n\[|$)/);
  if (!section) {
    return { status: "skipped", reason: "Codex config has no [mcp_servers.minutes] section" };
  }
  const body = section[1];
  const command = body.match(/command\s*=\s*"([^"]+)"/)?.[1];
  const argsRaw = body.match(/args\s*=\s*\[([\s\S]*?)\]/)?.[1] ?? "";
  const args = [...argsRaw.matchAll(/"([^"]*)"/g)].map((match) => match[1]);
  if (!command) {
    return { status: "skipped", reason: "Codex minutes server has no command" };
  }
  return { status: "ready", command, args };
}

const HOSTS = [
  ["claude-desktop-config", loadClaudeDesktopMinutesConfig],
  ["codex-cli-config", loadCodexMinutesConfig],
];

function normalizeServerConfig(raw) {
  if (raw.status !== "ready") return raw;
  return {
    ...raw,
    command: raw.command ?? defaultServer.command,
    args: raw.args?.length ? raw.args : defaultServer.args,
  };
}

function appendCompatEvent(home, seq, body) {
  const event = {
    v: 1,
    seq,
    timestamp: new Date().toISOString(),
    event_type: "NoteAdded",
    meeting_path: "/tmp/mcp-host-compat.md",
    text: body,
  };
  const minutesDir = join(home, ".minutes");
  writeFileSync(join(minutesDir, "events.jsonl"), `${JSON.stringify(event)}\n`, { flag: "a" });
  writeFileSync(join(minutesDir, "events.seq"), `${seq}\n`);
  return event;
}

async function waitFor(predicate, label) {
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    const value = predicate();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(`timed out waiting for ${label}`);
}

async function runHostSmoke(host, serverConfig) {
  const tempHome = mkdtempSync(join(tmpdir(), `minutes-${host}-`));
  const minutesDir = join(tempHome, ".minutes");
  mkdirSync(minutesDir, { recursive: true });
  writeFileSync(join(minutesDir, "agents.allow"), "compat-agent: agent.annotation\n");

  const notifications = [];
  const stderrChunks = [];
  const transport = new StdioClientTransport({
    command: serverConfig.command,
    args: serverConfig.args,
    cwd: repoRoot,
    env: {
      ...process.env,
      HOME: tempHome,
      USERPROFILE: tempHome,
      MINUTES_MCP_EVENT_POLL_MS: "50",
      RUST_LOG: "info",
    },
    stderr: "pipe",
  });
  transport.stderr?.on("data", (chunk) => stderrChunks.push(String(chunk)));

  const client = new Client(
    { name: `minutes-${host}-compat`, version: "0.0.0" },
    { capabilities: {} }
  );
  client.setNotificationHandler(ResourceUpdatedNotificationSchema, (notification) => {
    notifications.push(notification.params.uri);
  });

  try {
    await client.connect(transport);
    await client.subscribeResource({ uri: LIVE_URI });
    const appended = appendCompatEvent(tempHome, 1, `compat smoke for ${host}`);
    await waitFor(() => notifications.includes(LIVE_URI), `${host} resource update`);

    const read = await client.readResource({
      uri: `${LIVE_URI}?since_seq=0&limit=10`,
    });
    const text = read.contents?.[0]?.text ?? "";
    const payload = JSON.parse(text);
    const seen = payload.events.some((event) => event.seq === appended.seq);
    if (!seen) {
      throw new Error(`read_resource did not include appended seq ${appended.seq}`);
    }
    if (payload.reconnect?.cursor < appended.seq) {
      throw new Error(`reconnect cursor ${payload.reconnect?.cursor} is behind seq ${appended.seq}`);
    }

    await client.unsubscribeResource({ uri: LIVE_URI });
    return {
      host,
      status: "passed",
      command: serverConfig.command,
      args: serverConfig.args,
      subscribed_uri: LIVE_URI,
      notification_uri: LIVE_URI,
      appended_seq: appended.seq,
      read_uri: `${LIVE_URI}?since_seq=0&limit=10`,
      reconnect_cursor: payload.reconnect.cursor,
    };
  } catch (error) {
    return {
      host,
      status: "failed",
      command: serverConfig.command,
      args: serverConfig.args,
      error: error instanceof Error ? error.message : String(error),
      stderr: stderrChunks.join("").slice(-2000),
    };
  } finally {
    await client.close().catch(() => {});
    await transport.close().catch(() => {});
    rmSync(tempHome, { recursive: true, force: true });
  }
}

const results = [];
for (const [host, loader] of HOSTS) {
  const config = normalizeServerConfig(loader());
  if (config.status !== "ready") {
    results.push({ host, ...config });
    continue;
  }
  results.push(await runHostSmoke(host, config));
}

console.log(JSON.stringify({ checked_at: new Date().toISOString(), results }, null, 2));

const failed = results.filter((result) => result.status === "failed");
if (failed.length > 0) {
  process.exitCode = 1;
}
