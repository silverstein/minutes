import type { ChildProcess } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { isAbsolute, join } from "node:path";
import type { CapabilityProbeResult } from "./capabilities.js";

export function supportsDictationStopProtocol(capabilities: CapabilityProbeResult): boolean {
  return capabilities.kind === "report"
    && capabilities.report.features.dictation_addressed_stop_v1 === true;
}

export const DICTATION_CONTROL_UPGRADE_REQUIRED =
  "This Minutes CLI does not support graceful dictation control from chat. Update Minutes before using chat dictation. Use the Minutes desktop app in the meantime.";

// Match Config::minutes_dir: only an absolute MINUTES_DATA_DIR overrides home.
export function dictationStateDirectory(
  env: NodeJS.ProcessEnv = process.env,
  home = homedir(),
): string {
  const override = env.MINUTES_DATA_DIR;
  return override && isAbsolute(override) ? override : join(home, ".minutes");
}

export type DictationStartup =
  | { status: "listening"; pid: number }
  | { status: "starting"; pid: number }
  | { status: "error"; message: string };

// The CLI emits this only after opening its audio stream. A live PID or a
// completed model preflight alone does not establish microphone readiness.
export function waitForDictationStartup(
  child: ChildProcess,
  timeoutMs = 10_000,
): Promise<DictationStartup> {
  return new Promise((resolve) => {
    let diagnostics = "";
    let settled = false;
    const finish = (result: DictationStartup) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.stderr?.removeListener("data", onData);
      child.removeListener("exit", onExit);
      // Keep an error handler after returning, and keep draining stderr so
      // later CLI progress cannot block or break an otherwise healthy capture.
      child.stderr?.resume();
      resolve(result);
    };
    const onData = (chunk: Buffer | string) => {
      diagnostics = (diagnostics + chunk.toString()).slice(-16_384);
      if (/(?:^|\r?\n)\[minutes\] Listening\.\.\.(?:\r?\n|$)/.test(diagnostics)
        && child.pid !== undefined) {
        finish({ status: "listening", pid: child.pid });
      }
    };
    const onExit = (code: number | null, signal: NodeJS.Signals | null) => {
      finish({
        status: "error",
        message: diagnostics.trim() || `Dictation exited before microphone readiness (${signal || code}).`,
      });
    };
    const timer = setTimeout(() => {
      finish(child.pid === undefined
        ? { status: "error", message: "Dictation did not launch." }
        : { status: "starting", pid: child.pid });
    }, timeoutMs);
    child.stderr?.on("data", onData);
    child.on("exit", onExit);
    child.on("error", (error) => finish({ status: "error", message: error.message }));
  });
}

export async function requestDictationStop(
  stateDir = dictationStateDirectory(),
): Promise<"requested" | "inactive"> {
  let content: string;
  try {
    content = await readFile(join(stateDir, "dictation.pid"), "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return "inactive";
    throw error;
  }
  // Reject partial numeric strings rather than signaling an unintended PID.
  const text = content.trim();
  const pid = Number(text);
  if (!/^[1-9]\d*$/.test(text) || !Number.isSafeInteger(pid) || pid > 2_147_483_647) {
    throw new Error("Dictation PID file is invalid; no stop request was sent.");
  }
  // The core reader consumes only messages addressed to its process. SIGTERM
  // is ignored by the dictation CLI and an unaddressed stop could end recording.
  await writeFile(join(stateDir, "recording.stop"), `stop-pid:${pid}`);
  return "requested";
}
