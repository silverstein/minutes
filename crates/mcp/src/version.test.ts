import { describe, expect, it } from "vitest";

import { isCliCompatible, parseVersion } from "./version.js";

describe("parseVersion", () => {
  it("parses a bare semver string", () => {
    expect(parseVersion("0.14.0")).toEqual({ major: 0, minor: 14, patch: 0 });
  });

  it("parses `minutes X.Y.Z` output", () => {
    expect(parseVersion("minutes 0.13.3")).toEqual({
      major: 0,
      minor: 13,
      patch: 3,
    });
  });

  it("parses trailing whitespace and leading labels", () => {
    expect(parseVersion("  minutes-cli v0.14.0\n")).toEqual({
      major: 0,
      minor: 14,
      patch: 0,
    });
  });

  it("returns null on garbage input", () => {
    expect(parseVersion("not a version")).toBeNull();
    expect(parseVersion("")).toBeNull();
  });
});

describe("isCliCompatible", () => {
  it("flags same version as ok with no warning severity", () => {
    const result = isCliCompatible("0.14.0", "0.14.0");
    expect(result.ok).toBe(true);
    expect(result.severity).toBe("ok");
    expect(result.message).toContain("up to date");
  });

  it("accepts older CLI within same major (hosted MCPB + newer server)", () => {
    const result = isCliCompatible("0.13.3", "0.14.0");
    expect(result.ok).toBe(true);
    expect(result.severity).toBe("info");
    expect(result.message).toContain("same major");
  });

  it("accepts newer CLI within same major (hosted MCPB + user brew upgrade)", () => {
    const result = isCliCompatible("0.14.0", "0.13.3");
    expect(result.ok).toBe(true);
    expect(result.severity).toBe("info");
  });

  it("rejects major-version drift with an upgrade hint", () => {
    const result = isCliCompatible("1.0.0", "0.14.0");
    expect(result.ok).toBe(false);
    expect(result.severity).toBe("error");
    expect(result.message).toContain("major-version mismatch");
    expect(result.message).toContain("brew upgrade minutes");
  });

  it("rejects reverse major-version drift too", () => {
    const result = isCliCompatible("0.14.0", "1.0.0");
    expect(result.ok).toBe(false);
    expect(result.severity).toBe("error");
  });

  it("proceeds when the CLI reports an unparseable version", () => {
    const result = isCliCompatible("unknown-build", "0.14.0");
    expect(result.ok).toBe(true);
    expect(result.severity).toBe("info");
    expect(result.message).toContain("unparseable");
  });

  it("proceeds when the server version itself fails to parse", () => {
    const result = isCliCompatible("0.14.0", "weird");
    expect(result.ok).toBe(true);
    expect(result.severity).toBe("info");
  });
});

describe("parseVersion semver suffixes (#185)", () => {
  it("captures prerelease", () => {
    expect(parseVersion("0.14.0-rc1")).toEqual({
      major: 0, minor: 14, patch: 0, prerelease: "rc1",
    });
    expect(parseVersion("0.14.0-rc.2")).toEqual({
      major: 0, minor: 14, patch: 0, prerelease: "rc.2",
    });
  });

  it("captures build metadata", () => {
    expect(parseVersion("0.14.0+git.abc123")).toEqual({
      major: 0, minor: 14, patch: 0, build: "git.abc123",
    });
  });

  it("captures both prerelease and build", () => {
    expect(parseVersion("0.14.0-rc1+build.3")).toEqual({
      major: 0, minor: 14, patch: 0, prerelease: "rc1", build: "build.3",
    });
  });

  it("still parses prefixed forms", () => {
    expect(parseVersion("minutes 0.14.0")).toEqual({ major: 0, minor: 14, patch: 0 });
    expect(parseVersion("v0.14.0")).toEqual({ major: 0, minor: 14, patch: 0 });
  });
});

describe("isCliCompatible prerelease honesty (#185)", () => {
  it("prerelease vs GA on the same triple is info, not 'up to date'", () => {
    const result = isCliCompatible("0.14.0-rc1", "0.14.0");
    expect(result.ok).toBe(true);
    expect(result.severity).toBe("info");
    expect(result.message).toContain("prerelease");
  });

  it("GA vs GA on the same triple stays ok", () => {
    const result = isCliCompatible("0.14.0", "0.14.0");
    expect(result.severity).toBe("ok");
  });
});
