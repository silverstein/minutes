/**
 * CLI/MCP version compatibility.
 *
 * Replaces the historical strict-equality check with same-major semver
 * compatibility. See issue #183 for the rationale: hosted `.mcpb` bundles
 * ship a frozen MCP server version, while users' CLI versions advance
 * independently via brew/cargo/auto-install. Strict equality turned every
 * version skew into scary user-facing warnings and broke auto-install when
 * the pinned GitHub release tag no longer matched.
 */

export type VersionParts = {
  major: number;
  minor: number;
  patch: number;
  /** Semver prerelease component ("rc1", "beta.2"), if present (#185). */
  prerelease?: string;
  /** Semver build metadata ("git.abc123"), if present. Never affects
   *  compatibility per semver, surfaced for honest logging only (#185). */
  build?: string;
};

export type CompatibilitySeverity = "ok" | "info" | "error";

export type CompatibilityResult = {
  ok: boolean;
  severity: CompatibilitySeverity;
  message: string;
};

export function parseVersion(raw: string): VersionParts | null {
  // Core triple plus optional semver prerelease (-rc1, -beta.2) and build
  // metadata (+git.abc123). Pre-#185 this matched only the triple, so
  // "0.14.0-rc1" silently passed as GA 0.14.0.
  const match = raw
    .trim()
    .match(
      /(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?/
    );
  if (!match) return null;
  const [, major, minor, patch, prerelease, build] = match;
  const parts: VersionParts = {
    major: Number.parseInt(major, 10),
    minor: Number.parseInt(minor, 10),
    patch: Number.parseInt(patch, 10),
  };
  if (prerelease) parts.prerelease = prerelease;
  if (build) parts.build = build;
  return parts;
}

const UPGRADE_ADVICE =
  "Update with: brew upgrade minutes (or cargo install minutes-cli)";

/**
 * Decide whether a CLI version is compatible with the running MCP server.
 *
 * Rules (Phase 1 of #183):
 * - Unparseable version string: proceed, log informationally. Old CLIs may
 *   not emit a parseable `--version` but usually still work.
 * - Major-version mismatch: not compatible. Emit one clear error with an
 *   upgrade command.
 * - Same major, same version: ok, one-line info log.
 * - Same major, different minor/patch: ok. Older CLI with newer MCP, or
 *   vice-versa, is backward-compatible within a major per our contract.
 */
export function isCliCompatible(
  cliVersion: string,
  serverVersion: string
): CompatibilityResult {
  const cli = parseVersion(cliVersion);
  const server = parseVersion(serverVersion);

  if (!cli || !server) {
    return {
      ok: true,
      severity: "info",
      message: `CLI reported version '${cliVersion}' (unparseable), proceeding`,
    };
  }

  if (cli.major !== server.major) {
    return {
      ok: false,
      severity: "error",
      message:
        `CLI major-version mismatch: installed ${cliVersion}, ` +
        `MCP server expects ${server.major}.x. ${UPGRADE_ADVICE}`,
    };
  }

  if (
    cli.minor === server.minor &&
    cli.patch === server.patch
  ) {
    // A prerelease is NOT the GA release (#185): still compatible within
    // the major, but say what it actually is instead of "up to date".
    if (cli.prerelease || server.prerelease) {
      return {
        ok: true,
        severity: "info",
        message:
          `CLI v${cliVersion} and MCP server v${serverVersion} share ` +
          `${cli.major}.${cli.minor}.${cli.patch} but differ in prerelease; proceeding`,
      };
    }
    return {
      ok: true,
      severity: "ok",
      message: `CLI v${cliVersion}, up to date`,
    };
  }

  return {
    ok: true,
    severity: "info",
    message:
      `CLI v${cliVersion} against MCP server v${serverVersion} ` +
      `(same major, compatible)`,
  };
}
