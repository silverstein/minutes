import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

export async function attestSidekickProviderExecutable(executablePath) {
  if (!path.isAbsolute(String(executablePath ?? ""))) {
    throw new Error("Sidekick provider attestation requires an absolute executable path");
  }
  const canonicalPath = await fs.realpath(executablePath);
  const [bytes, versionResult] = await Promise.all([
    fs.readFile(canonicalPath),
    execFileAsync(canonicalPath, ["--version"], {
      encoding: "utf8",
      timeout: 10_000,
      maxBuffer: 64 * 1024,
    }),
  ]);
  const version = versionResult.stdout.trim();
  if (!version) throw new Error("Sidekick provider returned an empty version");
  return {
    path: canonicalPath,
    sha256: sha256(bytes),
    version,
  };
}

export function sidekickProviderAttestationMatches(actual, expected) {
  return path.isAbsolute(String(actual?.path ?? "")) &&
    actual?.path === expected?.path &&
    /^[a-f0-9]{64}$/.test(actual?.sha256 ?? "") &&
    actual?.sha256 === expected?.sha256 &&
    typeof actual?.version === "string" &&
    actual.version.length > 0 &&
    actual.version === expected?.version;
}
