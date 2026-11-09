import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";

import { createNodeHelperRuntimeResolver, MINIMUM_HELPER_NODE_MAJOR, NodeHelperRuntimeError, nodeChildEnvironment } from "./node-child.js";

describe("Node helper environment", () => {
  it("forces Electron hosts into Node mode", () => {
    const environment = nodeChildEnvironment(
      { PATH: "synthetic", ELECTRON_RUN_AS_NODE: "0" },
      "42.9.2"
    );
    expect(environment).toEqual({
      PATH: "synthetic",
      ELECTRON_RUN_AS_NODE: "1",
    });
  });

  it("does not invent Electron mode for an ordinary Node host", () => {
    expect(nodeChildEnvironment({ PATH: "synthetic" }, undefined)).toEqual({
      PATH: "synthetic",
    });
  });

  it("does not mutate the caller's environment object", () => {
    const base = { ELECTRON_RUN_AS_NODE: "0" };
    nodeChildEnvironment(base, "42.9.2");
    expect(base.ELECTRON_RUN_AS_NODE).toBe("0");
  });
});

const marker = (major = 24) => `minutes-node-helper:${major}.1.0`;

function fixture(environment: NodeJS.ProcessEnv = {}) {
  const probe = vi.fn(async (_binary: string, _env: NodeJS.ProcessEnv, _timeout: number): Promise<string | null> => null);
  const readDirectory = vi.fn(async (_directory: string): Promise<string[]> => []);
  const options = {
    execPath: "/Applications/Claude Helper",
    electronVersion: "42.0.0",
    platform: "darwin" as NodeJS.Platform,
    home: "/home/test",
    environment: () => environment,
    probe,
    readDirectory,
  };
  return { options, probe, readDirectory };
}

describe("Node helper runtime resolution", () => {
  it("keeps ordinary Node unchanged without probes, filesystem reads, or overrides", async () => {
    const { options, probe, readDirectory } = fixture({ MINUTES_NODE_PATH: "/override" });
    const resolve = createNodeHelperRuntimeResolver({ ...options, electronVersion: undefined });
    expect(await resolve()).toBe(options.execPath);
    expect(probe).not.toHaveBeenCalled();
    expect(readDirectory).not.toHaveBeenCalled();
  });

  it("probes a working Electron runtime once, including concurrent requests", async () => {
    const { options, probe, readDirectory } = fixture({ NODE_OPTIONS: "--require=untrusted", NODE_PATH: "/untrusted" });
    probe.mockResolvedValue(marker());
    const resolve = createNodeHelperRuntimeResolver(options);
    expect(await Promise.all([resolve(), resolve(), resolve()])).toEqual(Array(3).fill(options.execPath));
    expect(await resolve()).toBe(options.execPath);
    expect(probe).toHaveBeenCalledExactlyOnceWith(options.execPath, { ELECTRON_RUN_AS_NODE: "1" }, 1000);
    expect(readDirectory).not.toHaveBeenCalled();
  });

  it("uses a validated absolute override before PATH when the fuse blocks Node mode", async () => {
    const { options, probe } = fixture({ PATH: "/path", MINUTES_NODE_PATH: "/custom/node" });
    probe.mockImplementation(async (binary) => binary === "/custom/node" ? marker(20) : null);
    const resolve = createNodeHelperRuntimeResolver(options);
    expect(await resolve()).toBe("/custom/node");
    expect(await resolve()).toBe("/custom/node");
    expect(probe.mock.calls.map(([binary]) => binary)).toEqual([options.execPath, "/custom/node"]);
  });

  it.each(["", " ", "\t\n"])("discovers Node when the extension's optional path is blank (%j)", async (override) => {
    const { options, probe } = fixture({ PATH: "/system", MINUTES_NODE_PATH: override });
    probe.mockImplementation(async (binary) => binary === "/system/node" ? marker() : null);
    expect(await createNodeHelperRuntimeResolver(options)()).toBe("/system/node");
    expect(probe.mock.calls.map(([binary]) => binary)).toEqual([options.execPath, "/system/node"]);
  });

  it.each(["node", "/old-node", "/not-executable"])("rejects an unusable override (%s) with path-free guidance", async (override) => {
    const { options, probe } = fixture({ MINUTES_NODE_PATH: override });
    probe.mockImplementation(async (binary) => binary === "/old-node" ? marker(18) : null);
    await expect(createNodeHelperRuntimeResolver(options)()).rejects.toThrow(NodeHelperRuntimeError);
    await expect(createNodeHelperRuntimeResolver(options)()).rejects.toThrow("Node.js path setting (MINUTES_NODE_PATH) must name an absolute path");
  });

  it("validates PATH candidates, rejects bad output/old versions, and deduplicates", async () => {
    const { options, probe } = fixture({ PATH: ":relative:/old:/bad:/old:/good" });
    probe.mockImplementation(async (binary) => {
      if (binary === "/old/node") return marker(18);
      if (binary === "/bad/node") return "24.1.0";
      if (binary === "/good/node") return marker();
      return null;
    });
    expect(await createNodeHelperRuntimeResolver(options)()).toBe("/good/node");
    expect(probe.mock.calls.map(([binary]) => binary)).toEqual([options.execPath, "/old/node", "/bad/node", "/good/node"]);
  });

  it.each([
    "/opt/homebrew/bin/node",
    "/usr/local/bin/node",
    "/home/test/.volta/bin/node",
    "/home/test/.asdf/shims/node",
    "/home/test/.nvm/versions/node/v24.1.0/bin/node",
    "/home/test/Library/Application Support/fnm/node-versions/v24.1.0/installation/bin/node",
    "/home/test/.local/share/fnm/node-versions/v24.1.0/installation/bin/node",
    "/home/test/.asdf/installs/nodejs/24.1.0/bin/node",
  ])("finds a GUI installation at %s with an empty PATH", async (binary) => {
    const { options, probe, readDirectory } = fixture();
    readDirectory.mockResolvedValue(["v18.0.0", "v24.1.0", "24.1.0", "../escape"]);
    probe.mockImplementation(async (candidate) => candidate === binary ? marker() : null);
    expect(await createNodeHelperRuntimeResolver(options)()).toBe(binary);
  });

  it("honors manager installation directories", async () => {
    const { options, probe, readDirectory } = fixture({ NVM_DIR: "/custom-nvm" });
    readDirectory.mockResolvedValue(["v24.1.0"]);
    probe.mockImplementation(async (binary) => binary === "/custom-nvm/versions/node/v24.1.0/bin/node" ? marker() : null);
    expect(await createNodeHelperRuntimeResolver(options)()).toBe("/custom-nvm/versions/node/v24.1.0/bin/node");
  });

  it("looks for node.exe with Windows PATH casing and delimiters", async () => {
    const { options, probe } = fixture({ Path: "C:\\old;C:\\Program Files\\nodejs" });
    const binary = "C:\\Program Files\\nodejs\\node.exe";
    probe.mockImplementation(async (candidate) => candidate === binary ? marker() : null);
    expect(await createNodeHelperRuntimeResolver({ ...options, platform: "win32", home: "C:\\Users\\test" })()).toBe(binary);
  });

  it("finds a Windows installation outside PATH", async () => {
    const { options, probe } = fixture({ ProgramFiles: "C:\\Program Files" });
    const binary = "C:\\Program Files\\nodejs\\node.exe";
    probe.mockImplementation(async (candidate) => candidate === binary ? marker() : null);
    expect(await createNodeHelperRuntimeResolver({ ...options, platform: "win32", home: "C:\\Users\\test" })()).toBe(binary);
  });

  it("retries discovery after failure without repeating the failed Electron probe", async () => {
    const { options, probe } = fixture({ PATH: "/later" });
    const resolve = createNodeHelperRuntimeResolver(options);
    await expect(resolve()).rejects.toThrow("Install Node.js 20+ from nodejs.org or with Homebrew, or set Node.js path in the Minutes extension settings in Claude Desktop");
    probe.mockImplementation(async (binary) => binary === "/later/node" ? marker() : null);
    expect(await resolve()).toBe("/later/node");
    expect(probe.mock.calls.filter(([binary]) => binary === options.execPath)).toHaveLength(1);
  });

  it("revalidates an override changed after a successful resolution", async () => {
    const environment = { MINUTES_NODE_PATH: "/first" };
    const { options, probe } = fixture(environment);
    probe.mockImplementation(async (binary) => binary === options.execPath ? null : marker());
    const resolve = createNodeHelperRuntimeResolver(options);
    expect(await resolve()).toBe("/first");
    environment.MINUTES_NODE_PATH = "/second";
    expect(await resolve()).toBe("/second");
  });

  it("matches the package's minimum Node version", () => {
    const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
    expect(pkg.engines.node).toBe(`>=${MINIMUM_HELPER_NODE_MAJOR}`);
  });

  it("can validate a real Node child when the Electron host probe fails", async () => {
    const resolve = createNodeHelperRuntimeResolver({
      execPath: "/missing-electron-helper",
      electronVersion: "42.0.0",
      environment: () => ({ ...process.env, MINUTES_NODE_PATH: process.execPath }),
    });
    expect(await resolve()).toBe(process.execPath);
  });
});
