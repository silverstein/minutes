import { readFile, stat } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createConnection, type Socket } from "node:net";

const RELAY_PROTOCOL_VERSION = 1;
const HEARTBEAT_STALE_MS = 5_000;

export interface CaptureRelayCursor {
  session_id?: string;
  transcript_seq: number;
  nudge_seq: number;
}

export interface CaptureRelayDiscovery {
  v: number;
  session_id: string;
  transport: "unix_socket" | "windows_named_pipe";
  endpoint: string;
  owner_pid: number;
  evidence_mode: string;
  auth_token: string;
  started_at: string;
  heartbeat_at: string;
}

export interface CaptureRelayFrame {
  type: string;
  seq?: number;
  session_id?: string;
  message?: string;
  [key: string]: unknown;
}

export interface CaptureRelaySnapshot {
  discovery: Omit<CaptureRelayDiscovery, "auth_token">;
  cursor: CaptureRelayCursor;
  frames: CaptureRelayFrame[];
}

export interface CaptureRelayAttachOptions {
  discoveryPath?: string;
  timeoutMs?: number;
  replayQuietMs?: number;
}

export function captureRelayDiscoveryPath(): string {
  return join(homedir(), ".minutes", "capture-relay.json");
}

export async function attachCaptureRelay(
  cursor: CaptureRelayCursor = { transcript_seq: 0, nudge_seq: 0 },
  options: CaptureRelayAttachOptions = {},
): Promise<CaptureRelaySnapshot> {
  const discoveryPath = options.discoveryPath ?? captureRelayDiscoveryPath();
  const discovery = await readAndValidateDiscovery(discoveryPath);
  const timeoutMs = options.timeoutMs ?? 1_000;
  const replayQuietMs = options.replayQuietMs ?? 40;

  return await new Promise<CaptureRelaySnapshot>((resolvePromise, rejectPromise) => {
    let socket: Socket | undefined;
    let settled = false;
    let acknowledged = false;
    let buffer = "";
    let quietTimer: NodeJS.Timeout | undefined;
    const frames: CaptureRelayFrame[] = [];
    const nextCursor: CaptureRelayCursor = {
      session_id: cursor.session_id,
      transcript_seq: cursor.transcript_seq,
      nudge_seq: cursor.nudge_seq,
    };

    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeoutTimer);
      if (quietTimer) clearTimeout(quietTimer);
      socket?.destroy();
      if (error) {
        rejectPromise(error);
        return;
      }
      resolvePromise({
        discovery: redactDiscovery(discovery),
        cursor: nextCursor,
        frames,
      });
    };
    const scheduleReplayComplete = () => {
      if (!acknowledged) return;
      if (quietTimer) clearTimeout(quietTimer);
      quietTimer = setTimeout(() => finish(), replayQuietMs);
    };
    const timeoutTimer = setTimeout(
      () => finish(new Error("the capture attachment relay did not respond in time")),
      timeoutMs,
    );

    socket = createConnection(discovery.endpoint);
    socket.setEncoding("utf8");
    socket.once("connect", () => {
      socket?.write(`${JSON.stringify({
        v: RELAY_PROTOCOL_VERSION,
        auth_token: discovery.auth_token,
        action: { type: "observe", cursor },
      })}\n`);
    });
    socket.on("data", (chunk: string) => {
      buffer += chunk;
      while (true) {
        const newline = buffer.indexOf("\n");
        if (newline < 0) break;
        const line = buffer.slice(0, newline).trim();
        buffer = buffer.slice(newline + 1);
        if (!line) continue;

        let frame: CaptureRelayFrame;
        try {
          frame = JSON.parse(line) as CaptureRelayFrame;
        } catch {
          finish(new Error("the capture attachment relay sent invalid data"));
          return;
        }
        if (frame.type === "error") {
          finish(new Error(String(frame.message ?? "capture attachment failed")));
          return;
        }
        if (!acknowledged) {
          if (frame.type !== "attached" && frame.type !== "cursor_reset") {
            finish(new Error("the capture attachment relay did not acknowledge this client"));
            return;
          }
          acknowledged = true;
          nextCursor.session_id = discovery.session_id;
        }
        if (frame.type === "cursor_reset") {
          nextCursor.session_id = String(frame.session_id ?? discovery.session_id);
          nextCursor.transcript_seq = 0;
          nextCursor.nudge_seq = 0;
        } else if (frame.type === "transcript" && typeof frame.seq === "number") {
          nextCursor.transcript_seq = Math.max(nextCursor.transcript_seq, frame.seq);
        } else if (frame.type === "nudge" && typeof frame.seq === "number") {
          nextCursor.nudge_seq = Math.max(nextCursor.nudge_seq, frame.seq);
        }
        frames.push(frame);
      }
      scheduleReplayComplete();
    });
    socket.once("error", (error) => finish(error));
    socket.once("close", () => {
      if (!settled && !acknowledged) {
        finish(new Error("the capture owner closed the attachment relay"));
      } else if (!settled) {
        finish();
      }
    });
  });
}

async function readAndValidateDiscovery(path: string): Promise<CaptureRelayDiscovery> {
  let raw: string;
  try {
    raw = await readFile(path, "utf8");
  } catch {
    throw new Error("no active capture attachment relay was found");
  }
  let discovery: CaptureRelayDiscovery;
  try {
    discovery = JSON.parse(raw) as CaptureRelayDiscovery;
  } catch {
    throw new Error("the capture attachment discovery file is invalid");
  }
  if (discovery.v !== RELAY_PROTOCOL_VERSION) {
    throw new Error(`capture attachment protocol ${discovery.v} is not supported`);
  }
  if (!Number.isSafeInteger(discovery.owner_pid) || discovery.owner_pid <= 0) {
    throw new Error("the capture attachment owner PID is invalid");
  }
  const heartbeat = Date.parse(discovery.heartbeat_at);
  if (!Number.isFinite(heartbeat) || Date.now() - heartbeat > HEARTBEAT_STALE_MS) {
    throw new Error("the capture attachment heartbeat is stale");
  }
  if (!discovery.auth_token || discovery.auth_token.length < 32) {
    throw new Error("the capture attachment token is invalid");
  }
  validateLocalEndpoint(path, discovery);
  if (process.platform !== "win32") {
    const mode = (await stat(path)).mode & 0o777;
    if ((mode & 0o077) !== 0) {
      throw new Error("the capture attachment discovery file is not private to this user");
    }
  }
  try {
    process.kill(discovery.owner_pid, 0);
  } catch (error: any) {
    if (error?.code === "ESRCH") {
      throw new Error(`capture owner process ${discovery.owner_pid} is no longer running`);
    }
  }
  return discovery;
}

function validateLocalEndpoint(
  discoveryPath: string,
  discovery: CaptureRelayDiscovery,
): void {
  if (discovery.transport === "windows_named_pipe") {
    if (!discovery.endpoint.toLowerCase().startsWith("\\\\.\\pipe\\minutes-capture-")) {
      throw new Error("the capture attachment named pipe is not a Minutes local endpoint");
    }
    return;
  }
  if (discovery.transport !== "unix_socket") {
    throw new Error("the capture attachment transport is invalid");
  }
  const expected = resolve(dirname(discoveryPath), "capture-relay.sock");
  if (resolve(discovery.endpoint) !== expected) {
    throw new Error("the capture attachment socket is outside the private Minutes directory");
  }
}

function redactDiscovery(
  discovery: CaptureRelayDiscovery,
): Omit<CaptureRelayDiscovery, "auth_token"> {
  const { auth_token: _authToken, ...safe } = discovery;
  return safe;
}
