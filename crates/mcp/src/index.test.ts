import { mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { ResourceUpdatedNotificationSchema } from "@modelcontextprotocol/sdk/types.js";
import { describe, expect, it } from "vitest";

import {
  buildLiveCopilotResourcePayload,
  buildLiveEventsResourcePayload,
  extractMarkdownSection,
  LIVE_COPILOT_RESOURCE_URI,
  LIVE_EVENTS_RESOURCE_URI,
  meetingDetailPayload,
  meetingListItem,
  meetingSearchItem,
  MEETING_INSIGHT_KINDS,
  parseCopilotNudgeLog,
  parseCopilotStatusOutput,
  parseKnowledgeConfig,
  parseDictationModelMissingError,
  parseLiveEventsResourceUri,
  registerLiveEventsSubscriptionHandlers,
  selectCopilotNudges,
  shouldRunMainEntry,
  type CopilotNudgeObservation,
} from "./index.js";

describe("dictation model preflight errors", () => {
  it("extracts the model, expected path, and interrupted-download repair command", () => {
    const error = [
      "Error: Dictation model not installed: small",
      "Expected: /Users/test/.minutes/models/ggml-small.bin",
      "Fix: rm \"/Users/test/.minutes/models/ggml-small.bin\" && minutes setup --model small",
    ].join("\n");

    expect(parseDictationModelMissingError(error)).toEqual({
      model: "small",
      expectedPath: "/Users/test/.minutes/models/ggml-small.bin",
      setupCommand:
        "rm \"/Users/test/.minutes/models/ggml-small.bin\" && minutes setup --model small",
    });
  });

  it("ignores unrelated startup errors", () => {
    expect(parseDictationModelMissingError("microphone permission denied")).toBeNull();
  });
});

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

  it("surfaces the transcript body and synthesis fields in detail payloads (issue #255)", () => {
    const actionItems = [{ assignee: "Mat", task: "Ship fix", status: "open" }];
    const decisions = [{ text: "Enrich structuredContent" }];
    const intents = [{ kind: "commitment", what: "Reply to contributor", status: "open" }];

    const payload = meetingDetailPayload({
      path: meeting.path,
      speaker_map: [],
      overlay_applied: false,
      title: "Native Call",
      summary: "We agreed to fix get_meeting.",
      action_items: actionItems,
      decisions,
      intents,
      body: "## Summary\n\nWe agreed to fix get_meeting.\n\n## Transcript\n\n[00:00] Hello.",
    });

    expect(payload).toMatchObject({
      path: "/tmp/meeting.md",
      view: "detail",
      title: "Native Call",
      summary: "We agreed to fix get_meeting.",
      action_items: actionItems,
      decisions,
      intents,
    });
    expect(payload.body).toContain("## Transcript");
  });

  it("omits synthesis fields entirely when not provided", () => {
    expect(meetingDetailPayload({ path: meeting.path })).toEqual({
      path: "/tmp/meeting.md",
      view: "detail",
    });
  });
});

describe("extractMarkdownSection", () => {
  const body = [
    "## Summary",
    "",
    "First synthesized line.",
    "Second synthesized line.",
    "",
    "## Decisions",
    "",
    "- Ship the fix.",
    "",
    "## Transcript",
    "",
    "[00:00] Hello.",
  ].join("\n");

  it("returns a section's text up to the next heading", () => {
    expect(extractMarkdownSection(body, "Summary")).toBe(
      "First synthesized line.\nSecond synthesized line."
    );
  });

  it("returns undefined for an absent section", () => {
    expect(extractMarkdownSection(body, "Commitments")).toBeUndefined();
  });

  it("returns undefined for empty or missing input", () => {
    expect(extractMarkdownSection(undefined, "Summary")).toBeUndefined();
    expect(extractMarkdownSection("## Summary\n\n", "Summary")).toBeUndefined();
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

describe("copilot MCP observation contract", () => {
  const createdMs = Date.parse("2026-07-14T12:00:00.000Z");
  const firstNudge = {
    v: 1,
    id: "nudge-41-1",
    kind: "Ask",
    text: "Ask who owns the rollout date.",
    source_chip: "rollout date",
    evidence_revision: 41,
    created_ts: "2026-07-14T12:00:00.000Z",
    ttl_ms: 12000,
  };
  const secondNudge = {
    ...firstNudge,
    id: "nudge-42-2",
    kind: "Clarify",
    text: "Clarify whether Friday means launch or handoff.",
    evidence_revision: 42,
    created_ts: "2026-07-14T12:00:05.000Z",
    supersedes: "nudge-41-1",
  };

  it("parses active and inactive CLI status without treating Off as an error", () => {
    expect(parseCopilotStatusOutput("Copilot: Off\nLast error: Ollama unavailable")).toMatchObject({
      available: true,
      active: false,
      state: "Off",
      last_error: "Ollama unavailable",
    });

    expect(parseCopilotStatusOutput([
      "Copilot: Listening",
      "PID: 4321",
      "Goal: land the decision",
      "Surface: stdout",
      "Provider: ollama / llama3.2",
      "Evidence cursor: 42",
      "Attached to the shared event cursor; capture remains independently owned.",
    ].join("\n"))).toMatchObject({
      active: true,
      state: "Listening",
      pid: 4321,
      goal: "land the decision",
      surface: "stdout",
      provider: "ollama",
      model: "llama3.2",
      evidence_cursor: 42,
      capture_attachment:
        "Attached to the shared event cursor; capture remains independently owned.",
    });
  });

  it("parses JSON nudges with cursor and TTL metadata", () => {
    const nudges = parseCopilotNudgeLog(
      `${JSON.stringify(firstNudge)}\n${JSON.stringify(secondNudge)}\n`,
      createdMs + 6000
    );

    expect(nudges).toHaveLength(2);
    expect(nudges[0]).toMatchObject({ cursor: 1, format: "json", expired: false });
    expect(nudges[1]).toMatchObject({
      cursor: 2,
      format: "json",
      expired: false,
      nudge: { id: "nudge-42-2", supersedes: "nudge-41-1" },
    });
  });

  it("returns lossless cursor pages and resets a cursor from a prior session", () => {
    const nudges = parseCopilotNudgeLog(
      `${JSON.stringify(firstNudge)}\n${JSON.stringify(secondNudge)}\n`,
      createdMs + 6000
    );
    const observation: CopilotNudgeObservation = {
      attached: true,
      cursor: 2,
      session: null,
      nudges,
      note: "attached",
    };

    expect(selectCopilotNudges(observation, { cursor: 0, limit: 1 })).toMatchObject({
      cursor: 2,
      next_cursor: 1,
      cursor_reset: false,
      has_more: true,
      nudges: [{ cursor: 1 }],
    });
    expect(selectCopilotNudges(observation, { cursor: 99 })).toMatchObject({
      cursor: 2,
      next_cursor: 2,
      cursor_reset: true,
      has_more: false,
      nudges: [{ cursor: 1 }, { cursor: 2 }],
    });
    expect(
      selectCopilotNudges(observation, { since: "2s" }, createdMs + 6000).nudges
    ).toMatchObject([{ cursor: 2 }]);
  });

  it("exposes latest but never current advice after TTL expiry", () => {
    const status = parseCopilotStatusOutput([
      "Copilot: Nudge",
      "PID: 4321",
      "Goal: land the decision",
      "Surface: stdout",
      "Provider: ollama / llama3.2",
      "Evidence cursor: 42",
    ].join("\n"));
    const nudges = parseCopilotNudgeLog(JSON.stringify(firstNudge), createdMs + 13000);
    const payload = buildLiveCopilotResourcePayload(status, {
      attached: true,
      cursor: 1,
      session: null,
      nudges,
      note: "attached",
    });

    expect(payload.latest_nudge).toMatchObject({ cursor: 1, expired: true });
    expect(payload.current_nudge).toBeNull();
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

  it("routes copilot updates through the same subscription handler", async () => {
    const mcpServer = new McpServer({ name: "minutes-copilot-test", version: "0.0.0" });
    const updates: string[] = [];
    let fingerprint = "off:0";
    const controller = registerLiveEventsSubscriptionHandlers(mcpServer, {
      pollIntervalMs: 5,
      enableLiveEvents: false,
      enableCopilot: true,
      copilotFingerprint: async () => fingerprint,
    });

    const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: "copilot-test-client", version: "0.0.0" }, { capabilities: {} });
    client.setNotificationHandler(ResourceUpdatedNotificationSchema, (notification) => {
      updates.push(notification.params.uri);
    });

    try {
      await Promise.all([
        mcpServer.connect(serverTransport),
        client.connect(clientTransport),
      ]);
      await client.subscribeResource({ uri: LIVE_COPILOT_RESOURCE_URI });
      fingerprint = "listening:1";

      await waitFor(() => updates.length > 0);
      expect(updates).toEqual([LIVE_COPILOT_RESOURCE_URI]);

      await client.unsubscribeResource({ uri: LIVE_COPILOT_RESOURCE_URI });
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
