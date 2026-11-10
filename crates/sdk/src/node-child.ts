import { execFile } from "node:child_process";
import { readdir } from "node:fs/promises";
import { homedir } from "node:os";
import { posix, win32 } from "node:path";

/**
 * Environment for a helper that must run as Node.
 *
 * Electron hosts such as Claude Desktop expose their app executable as
 * process.execPath. Spawning that executable with a JavaScript file does not
 * start Node unless ELECTRON_RUN_AS_NODE is set for the child. Keep the switch
 * scoped to helpers whose argv is a Node program; ordinary Minutes CLI children
 * must continue to receive their existing policy environment unchanged.
 */
export function nodeChildEnvironment(
  base: NodeJS.ProcessEnv = process.env,
  electronVersion: string | undefined = process.versions.electron
): NodeJS.ProcessEnv {
  const environment = { ...base };
  if (electronVersion) {
    environment.ELECTRON_RUN_AS_NODE = "1";
  }
  return environment;
}

export const MINIMUM_HELPER_NODE_MAJOR = 20;
const PROBE_SOURCE = 'process.stdout.write("minutes-node-helper:" + process.versions.node)';
const PROBE_TIMEOUT_MS = 1_000;
const DISCOVERY_TIMEOUT_MS = 5_000;

/** Path-free guidance, safe to return through otherwise opaque tool errors. */
export class NodeHelperRuntimeError extends Error {
  constructor(override = false) {
    super(
      "Claude Desktop cannot run Minutes helper processes because its Electron Node runtime is unavailable. " +
      (override
        ? "The Node.js path setting (MINUTES_NODE_PATH) must name an absolute path to a working Node.js executable. "
        : "No working system Node.js runtime was found. ") +
      `Install Node.js ${MINIMUM_HELPER_NODE_MAJOR}+ from nodejs.org or with Homebrew, or set Node.js path in the Minutes extension settings in Claude Desktop to its absolute executable path, then quit and reopen Claude Desktop.`
    );
    this.name = "NodeHelperRuntimeError";
  }
}

type RuntimeProbe = (
  binary: string,
  environment: NodeJS.ProcessEnv,
  timeoutMs: number
) => Promise<string | null>;

type RuntimeResolverOptions = {
  execPath: string;
  electronVersion: string | undefined;
  platform: NodeJS.Platform;
  home: string;
  environment: () => NodeJS.ProcessEnv;
  probe: RuntimeProbe;
  readDirectory: (directory: string) => Promise<string[]>;
};

const probeNode: RuntimeProbe = (binary, environment, timeoutMs) =>
  new Promise((resolve) => {
    execFile(binary, ["-e", PROBE_SOURCE], {
      env: environment,
      timeout: timeoutMs,
      killSignal: "SIGKILL",
      maxBuffer: 1024,
      windowsHide: true,
    }, (error, stdout) => resolve(error ? null : stdout));
  });

/** Probes receive no meeting paths or authority, and never load ambient Node hooks. */
function probeEnvironment(base: NodeJS.ProcessEnv, electronVersion: string | undefined) {
  const environment = nodeChildEnvironment(base, electronVersion);
  delete environment.NODE_OPTIONS;
  delete environment.NODE_PATH;
  return environment;
}

/** Cache the host probe and successful resolution, never a missing runtime. */
export function createNodeHelperRuntimeResolver(options: Partial<RuntimeResolverOptions> = {}) {
  const settings: RuntimeResolverOptions = {
    execPath: process.execPath,
    electronVersion: process.versions.electron,
    platform: process.platform,
    home: homedir(),
    environment: () => process.env,
    probe: probeNode,
    readDirectory: async (directory) => {
      try {
        return await readdir(directory);
      } catch {
        return [];
      }
    },
    ...options,
  };
  const paths = settings.platform === "win32" ? win32 : posix;
  let hostProbe: Promise<boolean> | undefined;
  let resolved: { key: string; binary: string } | undefined;
  let pending: { key: string; promise: Promise<string> } | undefined;

  async function valid(binary: string, environment: NodeJS.ProcessEnv, timeoutMs: number) {
    const output = await settings.probe(binary, environment, timeoutMs);
    const match = /^minutes-node-helper:(\d+)\.\d+\.\d+$/.exec(output ?? "");
    return !!match && Number(match[1]) >= MINIMUM_HELPER_NODE_MAJOR;
  }

  async function* candidates(environment: NodeJS.ProcessEnv): AsyncGenerator<string> {
    const node = settings.platform === "win32" ? "node.exe" : "node";
    const searchPath = Object.entries(environment).find(([name]) =>
      settings.platform === "win32" ? name.toUpperCase() === "PATH" : name === "PATH"
    )?.[1] ?? "";
    for (const directory of searchPath.split(paths.delimiter).slice(0, 32)) {
      // Empty/relative PATH entries must not select a program from the server's cwd.
      if (paths.isAbsolute(directory)) yield paths.join(directory, node);
    }
    if (settings.platform === "win32") {
      for (const directory of [
        environment.ProgramFiles && paths.join(environment.ProgramFiles, "nodejs"),
        environment.LOCALAPPDATA && paths.join(environment.LOCALAPPDATA, "Programs", "nodejs"),
        paths.join(environment.VOLTA_HOME ?? paths.join(settings.home, "AppData", "Local", "Volta"), "bin"),
        paths.join(settings.home, "scoop", "apps", "nodejs", "current"),
      ]) {
        if (directory && paths.isAbsolute(directory)) yield paths.join(directory, node);
      }
      return;
    }
    for (const directory of [
      "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin",
      paths.join(environment.VOLTA_HOME ?? paths.join(settings.home, ".volta"), "bin"),
      paths.join(settings.home, ".asdf", "shims"),
      // Manual user-space installs (no package or version manager).
      paths.join(settings.home, ".local", "bin"),
      paths.join(settings.home, ".local", "node", "bin"),
    ]) yield paths.join(directory, node);

    for (const [directory, suffix] of [
      [paths.join(environment.NVM_DIR ?? paths.join(settings.home, ".nvm"), "versions", "node"), ["bin", node]],
      [paths.join(environment.FNM_DIR ?? paths.join(settings.home, "Library", "Application Support", "fnm"), "node-versions"), ["installation", "bin", node]],
      [paths.join(settings.home, ".local", "share", "fnm", "node-versions"), ["installation", "bin", node]],
      [paths.join(settings.home, ".asdf", "installs", "nodejs"), ["bin", node]],
    ] as const) {
      const versions = (await settings.readDirectory(directory))
        .filter((entry) => /^v?\d+\.\d+\.\d+$/.test(entry))
        .sort((a, b) => b.localeCompare(a, "en", { numeric: true }))
        .slice(0, 32);
      for (const version of versions) yield paths.join(directory, version, ...suffix);
    }
  }

  async function discover(environment: NodeJS.ProcessEnv, override: string | undefined) {
    const probeEnv = probeEnvironment(environment, settings.electronVersion);
    hostProbe ??= valid(settings.execPath, probeEnv, PROBE_TIMEOUT_MS);
    if (await hostProbe) return settings.execPath;
    if (override !== undefined) {
      if (paths.isAbsolute(override) && await valid(override, probeEnv, PROBE_TIMEOUT_MS)) return override;
      throw new NodeHelperRuntimeError(true);
    }
    const deadline = Date.now() + DISCOVERY_TIMEOUT_MS;
    const seen = new Set([settings.execPath]);
    for await (const candidate of candidates(environment)) {
      if (Date.now() >= deadline) break;
      if (seen.has(candidate) || !paths.isAbsolute(candidate)) continue;
      seen.add(candidate);
      if (await valid(candidate, probeEnv, Math.max(1, Math.min(PROBE_TIMEOUT_MS, deadline - Date.now())))) return candidate;
    }
    throw new NodeHelperRuntimeError();
  }

  return async (): Promise<string> => {
    if (!settings.electronVersion) return settings.execPath;
    const environment = settings.environment();
    const configuredPath = environment.MINUTES_NODE_PATH;
    // A path pasted into the extension settings can carry stray whitespace.
    const override = configuredPath?.trim() || undefined;
    const key = JSON.stringify([override, environment.PATH, environment.Path]);
    if (resolved?.key === key) return resolved.binary;
    if (pending?.key === key) return pending.promise;
    const promise = discover(environment, override);
    pending = { key, promise };
    try {
      const binary = await promise;
      resolved = { key, binary };
      return binary;
    } finally {
      if (pending?.promise === promise) pending = undefined;
    }
  };
}

export const resolveNodeHelperRuntime = createNodeHelperRuntimeResolver();
