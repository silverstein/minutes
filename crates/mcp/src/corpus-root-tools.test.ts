import { describe, expect, it } from "vitest";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

// Drive the built registration wrappers and exact-file handler over real stdio.
// A disposable profile prevents CLI readiness checks from touching user data.
describe("connector meetings root guidance", () => {
  it.each(["missing", "file"])("explains a %s configured folder on every read surface", async (kind) => {
    const profile = mkdtempSync(join(tmpdir(), "minutes-root-tools-"));
    const meetings = join(profile, "PRIVATE_CONFIGURED_ROOT");
    if (kind === "file") writeFileSync(meetings, "PRIVATE_CONTENT");
    mkdirSync(join(profile, ".minutes"));
    mkdirSync(join(profile, ".config", "minutes"), { recursive: true });
    const client = new Client({ name: "root-guidance-test", version: "0.0.0" });
    const transport = new StdioClientTransport({
      command: process.execPath,
      args: [resolve("dist/index.js")],
      env: {
        ...process.env as Record<string, string>,
        HOME: profile,
        USERPROFILE: profile,
        XDG_CONFIG_HOME: join(profile, ".config"),
        MINUTES_CONFIG_PATH: join(profile, ".config", "minutes", "config.toml"),
        MINUTES_HOME: join(profile, ".minutes"),
        MINUTES_DATA_DIR: join(profile, ".minutes"),
        MINUTES_MCP_AUTO_SETUP: "0",
        MEETINGS_DIR: meetings,
      },
      stderr: "pipe",
    });
    try {
      await client.connect(transport);
      for (const [name, args] of [
        ["list_meetings", {}],
        ["search_meetings", { query: "synthetic" }],
        ["get_meeting", { path: join(meetings, "PRIVATE_MEETING.md") }],
        ["research_topic", { query: "synthetic" }],
      ] as const) {
        const result = await client.callTool({ name, arguments: args });
        expect(result.isError, name).toBe(true);
        const text = JSON.stringify(result.content);
        expect(text, name).toContain("Minutes can't find your meetings folder.");
        expect(text, name).toContain("Meetings Directory setting");
        expect(text, name).not.toContain(profile);
        expect(text, name).not.toContain("PRIVATE");
      }
    } finally {
      await client.close();
      rmSync(profile, { recursive: true, force: true });
    }
  });
});
