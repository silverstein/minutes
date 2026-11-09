import { afterEach, describe, expect, it, vi } from "vitest";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const { resolveRuntime } = vi.hoisted(() => ({ resolveRuntime: vi.fn() }));
vi.mock("./node-child.js", async (importOriginal) => ({
  ...await importOriginal<typeof import("./node-child.js")>(),
  resolveNodeHelperRuntime: resolveRuntime,
}));

import { NodeHelperRuntimeError } from "./node-child.js";
import { withStableCorpusLease } from "./corpus-lease.js";
import { readTextFileFromBoundParent, retireBoundReadersForProcessShutdown } from "./secure-read.js";
import { handleMcpProcessAudioRequest, runIsolatedMcpProcessAudio } from "./index.js";

afterEach(async () => {
  await retireBoundReadersForProcessShutdown();
  resolveRuntime.mockReset();
});

describe("helper runtime preflight at all three call sites", () => {
  it.skipIf(process.platform !== "darwin" && process.platform !== "linux")("reports missing audio runtime repeatedly without poisoning isolation or reading a path", async () => {
    const root = vi.fn(async () => { throw new Error("root resolution reached"); });
    const run = () => runIsolatedMcpProcessAudio("/never-read.wav", [], [".wav"], root, undefined, "memo");
    resolveRuntime.mockRejectedValue(new NodeHelperRuntimeError());
    await expect(run()).rejects.toThrow(NodeHelperRuntimeError);
    await expect(run()).rejects.toThrow(NodeHelperRuntimeError);
    expect(root).not.toHaveBeenCalled();
    resolveRuntime.mockResolvedValue(process.execPath);
    await expect(run()).rejects.toThrow("root resolution reached");
    expect(root).toHaveBeenCalledOnce();
  });

  it("returns actionable runtime guidance in the process_audio tool result", async () => {
    const error = new NodeHelperRuntimeError();
    const result = await handleMcpProcessAudioRequest({ file_path: "/private.wav", type: "memo" }, {
      isCliAvailable: async () => true,
      execute: async () => { throw error; },
    }, "darwin");
    expect(result.isError).toBe(true);
    expect(result.content).toEqual([{ type: "text", text: error.message }]);
    expect(result.structuredContent).toMatchObject({ error: "node-helper-runtime-unavailable" });
  });

  it("preflights corpus workers before any corpus authorization and preserves the specific error", async () => {
    resolveRuntime.mockRejectedValue(new NodeHelperRuntimeError());
    const operation = vi.fn();
    await expect(withStableCorpusLease("/never-read", operation)).rejects.toThrow(NodeHelperRuntimeError);
    expect(operation).not.toHaveBeenCalled();
    expect(resolveRuntime).toHaveBeenCalledOnce();
  });

  it("preflights bounded readers without leaking admission, then reads with the resolved runtime", async () => {
    const directory = realpathSync(mkdtempSync(join(tmpdir(), "minutes-runtime-reader-")));
    try {
      const path = join(directory, "test.md");
      writeFileSync(path, "synthetic bounded content");
      resolveRuntime.mockRejectedValue(new NodeHelperRuntimeError());
      await expect(readTextFileFromBoundParent(path)).rejects.toThrow(NodeHelperRuntimeError);
      await expect(readTextFileFromBoundParent(path)).rejects.toThrow(NodeHelperRuntimeError);
      resolveRuntime.mockResolvedValue(process.execPath);
      expect((await readTextFileFromBoundParent(path)).toString()).toBe("synthetic bounded content");
      expect(resolveRuntime).toHaveBeenCalledTimes(3);
    } finally {
      await retireBoundReadersForProcessShutdown();
      rmSync(directory, { recursive: true, force: true });
    }
  });

  it.skipIf(process.platform !== "darwin" && process.platform !== "linux")("launches all three helpers through the resolved executable, preserving audio fd and policy", async () => {
    const directory = realpathSync(mkdtempSync(join(tmpdir(), "minutes-resolved-runtime-")));
    const shellQuote = (value: string) => "'" + value.replaceAll("'", "'\"'\"'") + "'";
    try {
      const launches = join(directory, "launches");
      const runtime = join(directory, "node-wrapper");
      writeFileSync(runtime, `#!/bin/sh\nprintf 'helper\\n' >> ${shellQuote(launches)}\nexec ${shellQuote(process.execPath)} "$@"\n`);
      chmodSync(runtime, 0o700);
      resolveRuntime.mockResolvedValue(runtime);

      const text = join(directory, "synthetic.md");
      writeFileSync(text, "synthetic text");
      expect((await readTextFileFromBoundParent(text)).toString()).toBe("synthetic text");

      const meetings = join(directory, "meetings");
      mkdirSync(meetings);
      expect(await withStableCorpusLease(meetings, (snapshot) => snapshot.files.length)).toBe(0);

      const inbox = join(directory, "inbox");
      mkdirSync(inbox);
      const audio = join(inbox, "synthetic.wav");
      writeFileSync(audio, "synthetic audio");
      const cli = join(directory, "cli.cjs");
      writeFileSync(cli, `#!${process.execPath}\nconst fs = require('node:fs');\nprocess.stdout.write(JSON.stringify({ bytes: fs.readFileSync(3).length, policy: process.env.MINUTES_CLI_RESTRICTED_POLICY, args: process.argv.slice(2) }));\n`);
      chmodSync(cli, 0o700);
      const result = await runIsolatedMcpProcessAudio(audio, [inbox], [".wav"], async () => meetings, undefined, "memo", undefined, {
        binary: cli,
        extraEnv: { MINUTES_CLI_RESTRICTED_POLICY: "allow" },
      });
      const payload = JSON.parse(result.stdout);
      expect(payload.bytes).toBe(Buffer.byteLength("synthetic audio"));
      expect(payload.policy).toBe("deny");
      expect(payload.args.slice(0, 2)).toEqual(["process", "authorized-input.wav"]);
      expect(result.stdout).not.toContain(audio);
      expect(readFileSync(launches, "utf8").trim().split("\n")).toHaveLength(3);
    } finally {
      await retireBoundReadersForProcessShutdown();
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
