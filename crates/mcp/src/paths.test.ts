import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "fs";
import { homedir } from "os";
import { join } from "path";
import { afterEach, describe, expect, it } from "vitest";

import { expandHomeLikePath, validatePathInDirectory } from "./paths.js";

const tempRoots: string[] = [];

afterEach(() => {
  for (const root of tempRoots.splice(0)) {
    rmSync(root, { recursive: true, force: true });
  }
});

describe("path normalization", () => {
  it("expands shell-style home roots", () => {
    expect(expandHomeLikePath("~/meetings")).toBe(join(homedir(), "meetings"));
    expect(expandHomeLikePath("$HOME/meetings")).toBe(join(homedir(), "meetings"));
    expect(expandHomeLikePath("${HOME}/meetings")).toBe(join(homedir(), "meetings"));
  });

  it("accepts a meeting file when the configured root uses ${HOME}", () => {
    const tempRoot = mkdtempSync(join(homedir(), "minutes-mcp-paths-"));
    tempRoots.push(tempRoot);

    const meetingsDir = join(tempRoot, "meetings");
    mkdirSync(meetingsDir, { recursive: true });

    const meetingPath = join(meetingsDir, "2026-03-28-home-expansion.md");
    writeFileSync(meetingPath, "# test meeting\n");

    const configuredRoot = `\${HOME}/${tempRoot.slice(homedir().length + 1)}/meetings`;

    expect(validatePathInDirectory(meetingPath, configuredRoot, [".md"])).toBe(meetingPath);
  });
});
