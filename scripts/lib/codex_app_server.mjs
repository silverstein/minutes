import { EventEmitter } from "node:events";
import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";

const DEFAULT_TIMEOUT_MS = 30_000;

export function mcpDisableArgsFromConfig(contents) {
  const names = new Set();
  for (const rawLine of String(contents).split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line.startsWith("[mcp_servers.") || !line.endsWith("]")) continue;
    let remainder = line.slice("[mcp_servers.".length, -1);
    let name;
    if (remainder.startsWith('"')) {
      const closing = remainder.indexOf('"', 1);
      if (closing < 1) continue;
      name = remainder.slice(1, closing);
    } else {
      name = remainder.split(".", 1)[0];
    }
    if (name) names.add(name);
  }
  return [...names].sort().flatMap((name) => [
    "--config",
    `mcp_servers.${/^[A-Za-z0-9_-]+$/.test(name) ? name : JSON.stringify(name)}.enabled=false`,
  ]);
}

export async function configuredMcpDisableArgs(
  codexHome = process.env.CODEX_HOME || path.join(os.homedir(), ".codex"),
) {
  try {
    return mcpDisableArgsFromConfig(await fs.readFile(path.join(codexHome, "config.toml"), "utf8"));
  } catch {
    return [];
  }
}

/**
 * Minimal Codex app-server JSONL client used by Sidekick evaluation spikes.
 *
 * This intentionally implements only the stable protocol primitives Minutes
 * needs: initialize, thread/start, turn/start, turn/steer, turn/interrupt,
 * streamed agent-message deltas, and turn completion. Keeping this adapter
 * small makes the fake-server tests exercise the same framing and correlation
 * logic used by live model evaluations.
 */
export class CodexAppServerClient extends EventEmitter {
  constructor({
    command = "codex",
    args = ["--config", 'service_tier="fast"', "--enable", "fast_mode", "app-server"],
    cwd = process.cwd(),
    env = process.env,
    requestTimeoutMs = DEFAULT_TIMEOUT_MS,
    closeGraceMs = 2_000,
    clientInfo = {
      name: "minutes-sidekick-eval",
      title: "Minutes Sidekick Eval",
      version: "0.1.0",
    },
    experimentalApi = false,
  } = {}) {
    super();
    this.command = command;
    this.args = args;
    this.cwd = cwd;
    this.env = env;
    this.requestTimeoutMs = requestTimeoutMs;
    this.closeGraceMs = closeGraceMs;
    this.clientInfo = clientInfo;
    this.experimentalApi = experimentalApi;
    this.child = null;
    this.stdin = null;
    this.nextRequestId = 1;
    this.pendingRequests = new Map();
    this.turns = new Map();
    this.stderrTail = "";
  }

  async start() {
    if (this.child) throw new Error("Codex app-server is already running");

    const child = spawn(this.command, this.args, {
      cwd: this.cwd,
      env: this.env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child = child;
    this.stdin = child.stdin;

    readline.createInterface({ input: child.stdout }).on("line", (line) => {
      this.#handleLine(line);
    });
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk) => {
      this.stderrTail = `${this.stderrTail}${chunk}`.slice(-8_000);
    });
    child.once("error", (error) => this.#failAll(error));
    child.once("exit", (code, signal) => {
      const detail = this.stderrTail.trim();
      const suffix = detail ? `: ${detail}` : "";
      this.#failAll(
        new Error(`Codex app-server exited (${signal ?? code ?? "unknown"})${suffix}`),
      );
      this.child = null;
      this.stdin = null;
    });

    const initialized = await this.request("initialize", {
      clientInfo: this.clientInfo,
      ...(this.experimentalApi
        ? { capabilities: { experimentalApi: true } }
        : {}),
    });
    this.notify("initialized", {});
    return initialized;
  }

  request(method, params, timeoutMs = this.requestTimeoutMs) {
    if (!this.stdin) throw new Error("Codex app-server is not running");
    const id = this.nextRequestId++;
    const payload = { method, id, params };
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pendingRequests.delete(id);
        reject(new Error(`${method} timed out after ${timeoutMs} ms`));
      }, timeoutMs);
      this.pendingRequests.set(id, { method, resolve, reject, timer });
      this.stdin.write(`${JSON.stringify(payload)}\n`);
    });
  }

  notify(method, params) {
    if (!this.stdin) throw new Error("Codex app-server is not running");
    this.stdin.write(`${JSON.stringify({ method, params })}\n`);
  }

  async startThread(params = {}) {
    const result = await this.request("thread/start", params);
    const threadId = result?.thread?.id;
    if (!threadId) throw new Error("thread/start response did not include thread.id");
    return { threadId, result };
  }

  async startTurn({ threadId, input, outputSchema, ...overrides }) {
    const startedAt = performance.now();
    const result = await this.request("turn/start", {
      threadId,
      input: typeof input === "string" ? [{ type: "text", text: input }] : input,
      ...(outputSchema ? { outputSchema } : {}),
      ...overrides,
    });
    const turnId = result?.turn?.id;
    if (!turnId) throw new Error("turn/start response did not include turn.id");

    const turn = this.#turn(turnId);
    const completion = turn.completed
      ? Promise.resolve(this.#turnResult(turn, startedAt))
      : new Promise((resolve, reject) => {
          const timer = setTimeout(() => {
            turn.waiters.delete(waiter);
            reject(new Error(`turn ${turnId} timed out after ${this.requestTimeoutMs} ms`));
          }, this.requestTimeoutMs);
          const waiter = { resolve, reject, timer, startedAt };
          turn.waiters.add(waiter);
        });
    return { turnId, completion };
  }

  async runTurn(params) {
    const turn = await this.startTurn(params);
    return turn.completion;
  }

  steerTurn({ threadId, turnId, input }) {
    return this.request("turn/steer", {
      threadId,
      expectedTurnId: turnId,
      input: typeof input === "string" ? [{ type: "text", text: input }] : input,
    });
  }

  interruptTurn({ threadId, turnId }) {
    return this.request("turn/interrupt", { threadId, turnId });
  }

  close() {
    if (!this.child) return;
    const child = this.child;
    child.kill("SIGTERM");
    const forceKill = setTimeout(() => {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill("SIGKILL");
      }
    }, this.closeGraceMs);
    forceKill.unref();
    child.once("exit", () => clearTimeout(forceKill));
    child.stdin?.end();
    child.stdout?.destroy();
    child.stderr?.destroy();
    child.unref();
  }

  #turn(turnId) {
    let turn = this.turns.get(turnId);
    if (!turn) {
      turn = {
        id: turnId,
        text: "",
        firstDeltaAt: null,
        completedAt: null,
        completed: false,
        status: null,
        error: null,
        waiters: new Set(),
      };
      this.turns.set(turnId, turn);
    }
    return turn;
  }

  #handleLine(line) {
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      this.emit("protocol-warning", { reason: "invalid_json" });
      return;
    }

    if (Object.hasOwn(message, "id")) {
      const pending = this.pendingRequests.get(message.id);
      if (pending) {
        clearTimeout(pending.timer);
        this.pendingRequests.delete(message.id);
        if (message.error) {
          pending.reject(new Error(`${pending.method} failed: ${JSON.stringify(message.error)}`));
        } else {
          pending.resolve(message.result);
        }
      }
    }

    const { method, params = {} } = message;
    if (method === "item/agentMessage/delta" && params.turnId) {
      const turn = this.#turn(params.turnId);
      if (turn.firstDeltaAt === null) turn.firstDeltaAt = performance.now();
      turn.text += params.delta ?? "";
      this.emit("turn-delta", {
        threadId: params.threadId ?? null,
        turnId: params.turnId,
        delta: params.delta ?? "",
        text: turn.text,
      });
    } else if (
      method === "item/completed" &&
      params.turnId &&
      params.item?.type === "agentMessage" &&
      typeof params.item.text === "string"
    ) {
      this.#turn(params.turnId).text = params.item.text;
    } else if (method === "turn/completed" && params.turn?.id) {
      const turn = this.#turn(params.turn.id);
      turn.completed = true;
      turn.completedAt = performance.now();
      turn.status = params.turn.status;
      turn.error = params.turn.error ?? null;
      for (const waiter of turn.waiters) {
        clearTimeout(waiter.timer);
        waiter.resolve(this.#turnResult(turn, waiter.startedAt));
      }
      turn.waiters.clear();
    }

    if (method) this.emit("notification", message);
    this.emit("protocol-message", message);
  }

  #turnResult(turn, startedAt) {
    return {
      turnId: turn.id,
      status: turn.status,
      error: turn.error,
      text: turn.text,
      firstDeltaMs:
        turn.firstDeltaAt === null ? null : Math.round(turn.firstDeltaAt - startedAt),
      totalMs: turn.completedAt === null ? null : Math.round(turn.completedAt - startedAt),
    };
  }

  #failAll(error) {
    for (const pending of this.pendingRequests.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.pendingRequests.clear();
    for (const turn of this.turns.values()) {
      for (const waiter of turn.waiters) {
        clearTimeout(waiter.timer);
        waiter.reject(error);
      }
      turn.waiters.clear();
    }
  }
}
