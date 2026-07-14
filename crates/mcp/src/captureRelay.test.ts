import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { attachCaptureRelay } from "./captureRelay.js";

const cleanup: Array<() => void> = [];

afterEach(() => {
  while (cleanup.length > 0) cleanup.pop()?.();
});

describe("capture attachment relay", () => {
  it("authenticates and reconnects from transcript and nudge cursors", async () => {
    const dir = mkdtempSync(join(tmpdir(), "minutes-mcp-relay-"));
    cleanup.push(() => rmSync(dir, { force: true, recursive: true }));
    const endpoint = process.platform === "win32"
      ? `\\\\.\\pipe\\minutes-capture-test-${process.pid}-${Date.now()}`
      : join(dir, "capture-relay.sock");
    const discoveryPath = join(dir, "capture-relay.json");
    const token = "a".repeat(64);
    const seenCursors: unknown[] = [];
    const server = createServer((socket) => {
      socket.setEncoding("utf8");
      let input = "";
      socket.on("data", (chunk: string) => {
        input += chunk;
        const newline = input.indexOf("\n");
        if (newline < 0) return;
        const hello = JSON.parse(input.slice(0, newline));
        expect(hello.auth_token).toBe(token);
        seenCursors.push(hello.action.cursor);
        socket.write(`${JSON.stringify({
          type: "attached",
          v: 1,
          session_id: "session-a",
          owner_pid: process.pid,
          evidence_mode: "capture_relay_partials",
          transcript_seq: 2,
          nudge_seq: 4,
        })}\n`);
        if (hello.action.cursor.transcript_seq < 2) {
          socket.write(`${JSON.stringify({ type: "transcript", seq: 2, update: {} })}\n`);
        }
        if (hello.action.cursor.nudge_seq < 4) {
          socket.write(`${JSON.stringify({ type: "nudge", seq: 4, nudge: {} })}\n`);
        }
      });
    });
    cleanup.push(() => server.close());
    await new Promise<void>((resolve, reject) => {
      server.once("error", reject);
      server.listen(endpoint, resolve);
    });

    writeFileSync(discoveryPath, JSON.stringify({
      v: 1,
      session_id: "session-a",
      transport: process.platform === "win32" ? "windows_named_pipe" : "unix_socket",
      endpoint,
      owner_pid: process.pid,
      evidence_mode: "capture_relay_partials",
      auth_token: token,
      started_at: new Date().toISOString(),
      heartbeat_at: new Date().toISOString(),
    }));
    if (process.platform !== "win32") chmodSync(discoveryPath, 0o600);

    const first = await attachCaptureRelay(undefined, {
      discoveryPath,
      replayQuietMs: 10,
    });
    expect(first.cursor).toEqual({
      session_id: "session-a",
      transcript_seq: 2,
      nudge_seq: 4,
    });
    expect(first.discovery).not.toHaveProperty("auth_token");

    const second = await attachCaptureRelay(first.cursor, {
      discoveryPath,
      replayQuietMs: 10,
    });
    expect(second.frames.map((frame) => frame.type)).toEqual(["attached"]);
    expect(seenCursors).toEqual([
      { transcript_seq: 0, nudge_seq: 0 },
      { session_id: "session-a", transcript_seq: 2, nudge_seq: 4 },
    ]);
  });

  it("rejects a stale relay before connecting", async () => {
    const dir = mkdtempSync(join(tmpdir(), "minutes-mcp-relay-stale-"));
    cleanup.push(() => rmSync(dir, { force: true, recursive: true }));
    const discoveryPath = join(dir, "capture-relay.json");
    writeFileSync(discoveryPath, JSON.stringify({
      v: 1,
      session_id: "stale",
      transport: "unix_socket",
      endpoint: join(dir, "capture-relay.sock"),
      owner_pid: process.pid,
      evidence_mode: "final_only",
      auth_token: "b".repeat(64),
      started_at: new Date(0).toISOString(),
      heartbeat_at: new Date(0).toISOString(),
    }));
    if (process.platform !== "win32") chmodSync(discoveryPath, 0o600);

    await expect(attachCaptureRelay(undefined, { discoveryPath })).rejects.toThrow("stale");
  });
});
