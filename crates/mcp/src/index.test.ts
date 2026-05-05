import { mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { ResourceUpdatedNotificationSchema } from "@modelcontextprotocol/sdk/types.js";
import { describe, expect, it } from "vitest";

import {
  buildLiveEventsResourcePayload,
  LIVE_EVENTS_RESOURCE_URI,
  meetingDetailPayload,
  meetingListItem,
  meetingSearchItem,
  MEETING_INSIGHT_KINDS,
  parseKnowledgeConfig,
  parseLiveEventsResourceUri,
  registerLiveEventsSubscriptionHandlers,
  shouldRunMainEntry,
} from "./index.js";

describe("meeting insight contract", () => {
  it("exports only the insight kinds the pipeline emits today", () => {
    expect(MEETING_INSIGHT_KINDS).toEqual(["decision", "commitment", "question"]);
  });
});

describe("meeting shape contract", () => {
  const meeting = {
    path: "/tmp/meeting.md",
    frontmatter: {
      date: "2026-05-05T10:00:00-07:00",
      title: "Capture Health Review",
      type: "meeting",
      duration: "12m",
      recording_health: {
        capture_warnings: [
          {
            kind: "silent",
            source: "system",
            message: "System audio was silent.",
            diagnostic_confidence: "inferred",
          },
        ],
        diarization_path: "ml-bleed-degraded",
      },
    },
  };

  it("omits recording_health from list and search results", () => {
    expect(meetingListItem(meeting)).toEqual({
      date: "2026-05-05T10:00:00-07:00",
      title: "Capture Health Review",
      content_type: "meeting",
      path: "/tmp/meeting.md",
      duration: "12m",
    });
    expect(meetingSearchItem(meeting)).toEqual({
      date: "2026-05-05T10:00:00-07:00",
      title: "Capture Health Review",
      content_type: "meeting",
      path: "/tmp/meeting.md",
    });
  });

  it("surfaces recording_health in detail payloads", () => {
    expect(
      meetingDetailPayload({
        path: meeting.path,
        speaker_map: [],
        recording_health: meeting.frontmatter.recording_health,
        overlay_applied: false,
      })
    ).toEqual({
      path: "/tmp/meeting.md",
      view: "detail",
      speaker_map: [],
      recording_health: meeting.frontmatter.recording_health,
      overlay_applied: false,
    });
  });
});

describe("parseKnowledgeConfig", () => {
  it("only treats enabled=true inside the knowledge section as enabling the knowledge base", () => {
    const parsed = parseKnowledgeConfig(`
[recording]
enabled = true

[knowledge]
enabled = false
path = "~/kb"
`);

    expect(parsed).toEqual({
      enabled: false,
      path: "~/kb",
      adapter: "wiki",
      engine: "none",
    });
  });

  it("reads knowledge settings from the knowledge section", () => {
    const parsed = parseKnowledgeConfig(`
[knowledge]
enabled = true
path = "~/kb"
adapter = "para"
engine = "agent"
`);

    expect(parsed).toEqual({
      enabled: true,
      path: "~/kb",
      adapter: "para",
      engine: "agent",
    });
  });
});

describe("shouldRunMainEntry", () => {
  it("accepts npm .bin shims that realpath to the module file", () => {
    const tempRoot = mkdtempSync(join(tmpdir(), "minutes-mcp-entry-"));
    const packageDir = join(tempRoot, "node_modules", "minutes-mcp", "dist");
    const binDir = join(tempRoot, "node_modules", ".bin");
    const modulePath = join(packageDir, "index.js");
    const shimPath = join(binDir, "minutes-mcp");

    mkdirSync(packageDir, { recursive: true });
    mkdirSync(binDir, { recursive: true });
    writeFileSync(modulePath, "export {};\n");
    symlinkSync(modulePath, shimPath);

    try {
      expect(shouldRunMainEntry(shimPath, modulePath)).toBe(true);
    } finally {
      rmSync(tempRoot, { recursive: true, force: true });
    }
  });

  it("accepts equivalent paths once symlinks are resolved", () => {
    expect(shouldRunMainEntry(import.meta.filename, import.meta.filename)).toBe(true);
  });

  it("rejects unrelated worker entrypoints", () => {
    expect(
      shouldRunMainEntry(
        "/Users/dev/project/node_modules/vitest/dist/workers/forks.js",
        "/Users/dev/project/crates/mcp/src/index.ts"
      )
    ).toBe(false);
  });
});

describe("live event MCP resource", () => {
  it("parses the base resource and cursor read URIs", () => {
    expect(parseLiveEventsResourceUri("minutes://events/live")).toMatchObject({
      uri: "minutes://events/live",
      sinceSeq: null,
      limit: 20,
    });
    expect(parseLiveEventsResourceUri("minutes://events/live?since_seq=42&limit=7")).toMatchObject({
      uri: "minutes://events/live?since_seq=42&limit=7",
      sinceSeq: 42,
      limit: 7,
    });
    expect(parseLiveEventsResourceUri("minutes://events/recent")).toBeNull();
  });

  it("builds a reconnect cursor from the highest delivered sequence", () => {
    const payload = buildLiveEventsResourcePayload(
      { uri: "minutes://events/live?since_seq=10", sinceSeq: 10, limit: 100 },
      [{ seq: 11 }, { seq: 14 }],
      12
    );

    expect(payload.latest_seq).toBe(14);
    expect(payload.reconnect).toEqual({
      cursor: 14,
      read_uri: "minutes://events/live?since_seq=14",
    });
  });

  it("keeps the reconnect cursor on the delivered page boundary", () => {
    const payload = buildLiveEventsResourcePayload(
      { uri: "minutes://events/live?since_seq=10&limit=1", sinceSeq: 10, limit: 1 },
      [{ seq: 11 }],
      14
    );

    expect(payload.latest_seq).toBe(14);
    expect(payload.reconnect).toEqual({
      cursor: 11,
      read_uri: "minutes://events/live?since_seq=11",
    });
  });

  it("does not move a future reconnect cursor backward", () => {
    const payload = buildLiveEventsResourcePayload(
      { uri: "minutes://events/live?since_seq=99", sinceSeq: 99, limit: 100 },
      [],
      14
    );

    expect(payload.latest_seq).toBe(14);
    expect(payload.reconnect).toEqual({
      cursor: 99,
      read_uri: "minutes://events/live?since_seq=99",
    });
  });

  it("sends resource updated notifications over an MCP client subscription", async () => {
    const mcpServer = new McpServer({ name: "minutes-test", version: "0.0.0" });
    const updates: string[] = [];
    let readCursor = 4;
    const controller = registerLiveEventsSubscriptionHandlers(mcpServer, {
      pollIntervalMs: 5,
      latestEventSeq: async () => 4,
      readEventsSinceSeq: async (sinceSeq) => {
        if (sinceSeq >= readCursor) {
          readCursor = 9;
          return [{ seq: 9, event_type: "live.utterance.final" }];
        }
        return [];
      },
    });

    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "test-client", version: "0.0.0" }, { capabilities: {} });
    client.setNotificationHandler(ResourceUpdatedNotificationSchema, (notification) => {
      updates.push(notification.params.uri);
    });

    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      await client.subscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI });

      await waitFor(() => updates.length > 0);
      expect(updates).toEqual([LIVE_EVENTS_RESOURCE_URI]);

      await client.unsubscribeResource({ uri: LIVE_EVENTS_RESOURCE_URI });
      expect(controller.subscriptionCount()).toBe(0);
    } finally {
      controller.stop();
      await client.close();
      await mcpServer.close();
    }
  });
});

async function waitFor(predicate: () => boolean): Promise<void> {
  const deadline = Date.now() + 1000;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error("timed out waiting for condition");
}
