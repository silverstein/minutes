import { spawn, type ChildProcess } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { dictationStateDirectory, requestDictationStop, supportsDictationStopProtocol, waitForDictationStartup } from "./dictation-control.js";

const children: ChildProcess[] = [];
const roots: string[] = [];
function cli(script: string) {
  const child = spawn(process.execPath, ["-e", script], { stdio: ["ignore", "ignore", "pipe"] });
  children.push(child);
  return child;
}
afterEach(async () => {
  for (const child of children.splice(0)) {
    if (child.exitCode === null && child.signalCode === null) child.kill();
  }
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

describe("dictation microphone startup", () => {
  it("waits for a complete readiness message, including split pipe chunks", async () => {
    const child = cli("process.stderr.write('[minutes] Starting dictation\\n'); setTimeout(() => { process.stderr.write('[minutes] Lis'); setTimeout(() => process.stderr.write('tening...\\n'), 10); }, 20); setTimeout(() => {}, 1000)");
    expect(await waitForDictationStartup(child, 1000)).toEqual({ status: "listening", pid: child.pid });
  });
  it("reports actual microphone failure instead of claiming started", async () => {
    const child = cli("process.stderr.write('Microphone permission denied\\n'); process.exitCode = 1");
    expect(await waitForDictationStartup(child, 1000)).toEqual({ status: "error", message: "Microphone permission denied" });
  });
  it("keeps a slow startup pending instead of claiming microphone readiness", async () => {
    const child = cli("setTimeout(() => {}, 1000)");
    expect(await waitForDictationStartup(child, 20)).toEqual({ status: "starting", pid: child.pid });
  });
  it("handles executable launch errors", async () => {
    const child = spawn(join(tmpdir(), "minutes-nonexistent-test-executable"), [], { stdio: ["ignore", "ignore", "pipe"] });
    children.push(child);
    expect(await waitForDictationStartup(child, 1000)).toMatchObject({ status: "error" });
  });
  it("drains later progress after readiness rather than breaking the child pipe", async () => {
    const child = cli("process.stderr.write('[minutes] Listening...\\n'); setTimeout(() => process.stderr.write('x'.repeat(1000000)), 20)");
    expect((await waitForDictationStartup(child, 1000)).status).toBe("listening");
    await new Promise<void>((resolve) => child.once("exit", (code) => { expect(code).toBe(0); resolve(); }));
  });
});

describe("dictation stop profile and addressing", () => {
  it("rejects an older CLI even when its legacy dictation flags are true", () => {
    expect(supportsDictationStopProtocol({ kind: "report", report: {
      version: "0.28.1", api_version: 1, features: { start_dictation: true, stop_dictation: true },
    } })).toBe(false);
  });
  it("requires the explicit graceful-stop protocol", () => {
    expect(supportsDictationStopProtocol({ kind: "report", report: {
      version: "0.28.1", api_version: 1, features: { dictation_addressed_stop_v1: true },
    } })).toBe(true);
  });
  it("fails closed when the capture protocol cannot be discovered", () => {
    expect(supportsDictationStopProtocol({ kind: "missing-cli" })).toBe(false);
    expect(supportsDictationStopProtocol({ kind: "unsupported-cli" })).toBe(false);
  });
  it("matches native absolute data-root rules and ignores MINUTES_HOME", () => {
    const home = tmpdir();
    const isolated = join(home, "minutes-isolated");
    expect(dictationStateDirectory({ MINUTES_DATA_DIR: isolated }, home)).toBe(isolated);
    for (const value of [undefined, "", "relative"]) {
      expect(dictationStateDirectory({ MINUTES_DATA_DIR: value, MINUTES_HOME: isolated }, home)).toBe(join(home, ".minutes"));
    }
  });
  it("addresses the selected dictation PID and preserves a separate profile", async () => {
    const selected = await mkdtemp(join(tmpdir(), "minutes-stop-selected-"));
    const other = await mkdtemp(join(tmpdir(), "minutes-stop-other-"));
    roots.push(selected, other);
    await writeFile(join(selected, "dictation.pid"), "12345\n");
    await writeFile(join(other, "dictation.pid"), "67890");
    await writeFile(join(other, "recording.stop"), "preserve");
    expect(await requestDictationStop(selected)).toBe("requested");
    expect(await readFile(join(selected, "recording.stop"), "utf8")).toBe("stop-pid:12345");
    expect(await readFile(join(other, "recording.stop"), "utf8")).toBe("preserve");
  });
  it("leaves unrelated recording untouched when dictation is absent", async () => {
    const root = await mkdtemp(join(tmpdir(), "minutes-stop-inactive-"));
    roots.push(root);
    await writeFile(join(root, "recording.pid"), "12345");
    expect(await requestDictationStop(root)).toBe("inactive");
    await expect(readFile(join(root, "recording.stop"))).rejects.toMatchObject({ code: "ENOENT" });
  });
  it("rejects malformed or overflowing PIDs without writing a stop", async () => {
    const root = await mkdtemp(join(tmpdir(), "minutes-stop-invalid-"));
    roots.push(root);
    for (const pid of ["1234oops", "-1", "0", "", "1.5", "9007199254740992", "2147483648"]) {
      await writeFile(join(root, "dictation.pid"), pid);
      await expect(requestDictationStop(root)).rejects.toThrow("invalid");
      await expect(readFile(join(root, "recording.stop"))).rejects.toMatchObject({ code: "ENOENT" });
    }
  });
});
