#!/usr/bin/env node

/**
 * Minutes MCP Server
 *
 * MCP tools for Claude Desktop / Cowork / Dispatch:
 *   - start_recording: Start recording audio from the default input device
 *   - stop_recording: Stop recording and process through the pipeline
 *   - get_status: Check if a recording is in progress
 *   - list_meetings: List recent meetings and voice memos
 *   - search_meetings: Search meeting transcripts
 *   - get_meeting: Get full transcript of a specific meeting
 *   - process_audio: Process an audio file through the pipeline
 *   - add_note: Add a timestamped note to a recording or meeting
 *   - consistency_report: Flag conflicting decisions and stale commitments
 *   - get_person_profile: Rich relationship profile for a person (graph index)
 *   - track_commitments: List open/stale commitments, filter by person
 *   - relationship_map: All contacts with scores and losing-touch alerts
 *   - research_topic: Cross-meeting topic research
 *   - qmd_collection_status: Check QMD collection registration
 *   - register_qmd_collection: Register Minutes output as QMD collection
 *   - list_voices: List enrolled voice profiles for speaker identification
 *   - confirm_speaker: Confirm/correct speaker attribution in a meeting
 *
 * All tools use execFile (not exec) to shell out to the `minutes` CLI binary.
 * No shell interpolation — safe from injection.
 */

import { McpServer, ResourceTemplate } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  registerAppTool,
  registerAppResource,
  RESOURCE_MIME_TYPE,
  EXTENSION_ID,
} from "@modelcontextprotocol/ext-apps/server";
import { z } from "zod";
import { execFile, spawn } from "child_process";
import { promisify } from "util";
import { existsSync, realpathSync } from "fs";
import { readFile } from "fs/promises";
import { dirname, extname, isAbsolute, join, relative, resolve } from "path";
import { fileURLToPath } from "url";
import { homedir } from "os";

import * as reader from "minutes-sdk";

const UI_RESOURCE_URI = "ui://minutes/dashboard";

const execFileAsync = promisify(execFile);

// ── QMD semantic search (optional — falls back to CLI) ──────

let qmdAvailable: boolean | null = null;

async function runQmd(
  args: string[],
  timeoutMs: number = 15000
): Promise<{ stdout: string; stderr: string } | null> {
  try {
    const { stdout, stderr } = await execFileAsync("qmd", args, {
      timeout: timeoutMs,
      env: { ...process.env },
    });
    return { stdout: stdout.trim(), stderr: stderr.trim() };
  } catch {
    return null;
  }
}

async function isQmdAvailable(): Promise<boolean> {
  if (qmdAvailable !== null) return qmdAvailable;
  const result = await runQmd(["collection", "show", "minutes"]);
  qmdAvailable = result !== null && !result.stderr.includes("not found") && !result.stderr.includes("No collection");
  if (qmdAvailable) {
    console.error("[Minutes] QMD available — semantic search enabled for minutes collection");
  }
  return qmdAvailable;
}

async function enrichWithFrontmatter(qmdResults: any[]): Promise<any[]> {
  return Promise.all(
    qmdResults.map(async (r: any) => {
      const filePath = r.source_path || r.path;
      try {
        const meeting = await reader.getMeeting(filePath);
        return {
          date: meeting?.frontmatter.date || "",
          title: meeting?.frontmatter.title || "",
          content_type: meeting?.frontmatter.type || "meeting",
          path: filePath,
          snippet: r.snippet || "",
        };
      } catch {
        return {
          date: "",
          title: "",
          content_type: "meeting",
          path: filePath,
          snippet: r.snippet || "",
        };
      }
    })
  );
}

async function searchViaQmd(
  query: string,
  limit: number,
  contentType?: string
): Promise<any[] | null> {
  if (!(await isQmdAvailable())) return null;

  const args = ["search", query, "-c", "minutes", "-n", String(limit), "--json"];
  const result = await runQmd(args);
  if (!result) return null;

  try {
    const parsed = JSON.parse(result.stdout);
    const results = Array.isArray(parsed) ? parsed : parsed.results || [];
    if (results.length === 0) return null;

    const enriched = await enrichWithFrontmatter(results);

    // Apply content type filter if specified
    if (contentType) {
      const filtered = enriched.filter((r: any) => r.content_type === contentType);
      return filtered.length > 0 ? filtered : null;
    }

    return enriched;
  } catch {
    return null;
  }
}

async function triggerQmdIndex(): Promise<void> {
  if (!(await isQmdAvailable())) return;
  // Fire-and-forget — don't block the response
  execFileAsync("qmd", ["update", "-c", "minutes"]).catch(() => {});
}

// ESM-compatible __dirname
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// ── Find the minutes binary ─────────────────────────────────

function findMinutesBinary(): string {
  const isWindows = process.platform === "win32";
  const ext = isWindows ? ".exe" : "";
  const candidates = [
    join(__dirname, "..", "..", "..", "target", "release", `minutes${ext}`),
    join(__dirname, "..", "..", "..", "target", "debug", `minutes${ext}`),
    join(homedir(), ".cargo", "bin", `minutes${ext}`),
    ...(isWindows
      ? []
      : [
          join(homedir(), ".local", "bin", "minutes"),
          "/opt/homebrew/bin/minutes",
          "/usr/local/bin/minutes",
        ]),
  ];

  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      return candidate;
    }
  }

  // Fall back to PATH lookup
  return "minutes";
}

const MINUTES_BIN = findMinutesBinary();

// ── CLI availability detection ──────────────────────────────
// When installed via `npx minutes-mcp`, the Rust CLI may not be present.
// In that case, read-only tools use the pure-TS reader module.

let cliAvailable: boolean | null = null;
let cliCheckedAt = 0;
const CLI_CACHE_TTL_MS = 5 * 60 * 1000; // re-check every 5 minutes

async function isCliAvailable(): Promise<boolean> {
  // Cache hit: return true permanently (CLI won't disappear mid-session)
  // Cache miss (false): re-probe after TTL so installing CLI mid-session works
  if (cliAvailable === true) return true;
  if (cliAvailable === false && Date.now() - cliCheckedAt < CLI_CACHE_TTL_MS) return false;

  try {
    await execFileAsync(MINUTES_BIN, ["--version"], { timeout: 5000 });
    cliAvailable = true;
    cliCheckedAt = Date.now();
    console.error("[Minutes] CLI found — full mode (all tools enabled)");
  } catch {
    cliAvailable = false;
    cliCheckedAt = Date.now();
    console.error(
      "[Minutes] CLI not found — read-only mode. Install for recording: brew install minutes"
    );
  }
  return cliAvailable;
}

const CLI_INSTALL_MSG =
  "Recording requires the minutes CLI binary. Install it:\n" +
  "  macOS:   brew tap silverstein/tap && brew install minutes\n" +
  "  Any:     cargo install minutes-cli\n" +
  "  Source:  https://github.com/silverstein/minutes";

// ── Helper: run minutes CLI command (uses execFile, not exec) ──

async function runMinutes(
  args: string[],
  timeoutMs: number = 30000
): Promise<{ stdout: string; stderr: string }> {
  try {
    const { stdout, stderr } = await execFileAsync(MINUTES_BIN, args, {
      timeout: timeoutMs,
      env: { ...process.env, RUST_LOG: "info" },
    });
    return { stdout: stdout.trim(), stderr: stderr.trim() };
  } catch (error: any) {
    if (error.killed) {
      throw new Error(`Command timed out after ${timeoutMs}ms`);
    }
    const stderr = error.stderr?.trim() || "";
    const stdout = error.stdout?.trim() || "";
    throw new Error(stderr || stdout || error.message);
  }
}

function parseJsonOutput(stdout: string): any {
  try {
    return JSON.parse(stdout);
  } catch {
    return { raw: stdout };
  }
}

function canonicalizeFilePath(path: string): string {
  if (!existsSync(path)) {
    throw new Error(`Path does not exist: ${path}`);
  }
  return realpathSync(path);
}

function canonicalizeRoot(root: string): string {
  // Roots may not exist yet (e.g. ~/.minutes/inbox on first run).
  // Use realpath if it exists, otherwise lexical resolve.
  return existsSync(root) ? realpathSync(root) : resolve(root);
}

function isWithinDirectory(candidate: string, root: string): boolean {
  // Ensure root ends with separator to prevent prefix attacks (e.g. ~/meetings-evil)
  const rootWithSep = root.endsWith("/") ? root : root + "/";
  return candidate === root || candidate.startsWith(rootWithSep);
}

function validatePathInDirectory(path: string, root: string, allowedExts: string[]): string {
  const canonicalPath = canonicalizeFilePath(path);
  const canonicalRoot = canonicalizeRoot(root);

  if (!allowedExts.includes(extname(canonicalPath).toLowerCase())) {
    throw new Error(
      `Access denied: path must be within ${canonicalRoot} and end with ${allowedExts.join(", ")}`
    );
  }

  if (!isWithinDirectory(canonicalPath, canonicalRoot)) {
    throw new Error(`Access denied: path must be within ${canonicalRoot}`);
  }

  return canonicalPath;
}

function validatePathInDirectories(
  path: string,
  roots: string[],
  allowedExts: string[]
): string {
  const canonicalPath = canonicalizeFilePath(path);

  if (!allowedExts.includes(extname(canonicalPath).toLowerCase())) {
    throw new Error(
      `Access denied: path must end with one of ${allowedExts.join(", ")}`
    );
  }

  const canonicalRoots = roots.map((root) => canonicalizeRoot(root));
  if (!canonicalRoots.some((root) => isWithinDirectory(canonicalPath, root))) {
    throw new Error(
      `Access denied: file must be inside one of ${canonicalRoots.join(", ")}`
    );
  }

  return canonicalPath;
}

// ── MCP Server ──────────────────────────────────────────────

const server = new McpServer({
  name: "minutes",
  version: "0.8.0",
});

// Declare MCP Apps extension support so hosts classify this server as interactive.
// The `extensions` field is part of the draft MCP spec (SEP-1724) — not yet in the
// stable SDK types, so we cast through `any`.
(server.server as any).registerCapabilities({
  extensions: { [EXTENSION_ID]: {} },
} as any);

// Configurable directories — override via env vars in Claude Desktop extension settings
const MEETINGS_DIR = process.env.MEETINGS_DIR || join(homedir(), "meetings");
const MINUTES_HOME = process.env.MINUTES_HOME || join(homedir(), ".minutes");

// ── UI Resource: MCP App dashboard ──────────────────────────

registerAppResource(
  server,
  "Minutes Dashboard",
  UI_RESOURCE_URI,
  { description: "Interactive meeting dashboard and detail viewer" },
  async () => {
    const htmlPath = join(__dirname, "..", "dist-ui", "index.html");
    const html = await readFile(htmlPath, "utf-8");
    return {
      contents: [{
        uri: UI_RESOURCE_URI,
        mimeType: RESOURCE_MIME_TYPE,
        text: html,
      }],
    };
  }
);

// ── Tool: start_recording ───────────────────────────────────

server.tool(
 "start_recording",
  "Start recording audio from the default input device. The recording runs until stop_recording is called.",
  {
    title: z.string().optional().describe("Optional title for this recording"),
    mode: z
      .enum(["meeting", "quick-thought"])
      .optional()
      .default("meeting")
      .describe("Live capture mode"),
  },
  { title: "Start Recording", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async ({ title, mode }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }
    const { stdout: statusOut } = await runMinutes(["status"]);
    const status = parseJsonOutput(statusOut);
    if (status.recording) {
      return {
        content: [
          {
            type: "text" as const,
            text: `Already recording (PID: ${status.pid}). Run stop_recording first.`,
          },
        ],
      };
    }

    // Spawn detached — recording is a foreground process that blocks,
    // so we spawn it and let it run independently
    const args = ["record", "--mode", mode];
    if (title) args.push("--title", title);

    const child = spawn(MINUTES_BIN, args, {
      detached: true,
      stdio: "ignore",
      env: { ...process.env, RUST_LOG: "info" },
    });
    child.unref();

    // Wait for PID file to appear
    await new Promise((r) => setTimeout(r, 1000));

    const { stdout: newStatus } = await runMinutes(["status"]);
    const result = parseJsonOutput(newStatus);

    return {
      content: [
        {
          type: "text" as const,
          text: result.recording
            ? `${result.recording_mode === "quick-thought" ? "Quick thought" : "Recording"} started (PID: ${result.pid}). Say "stop recording" when done.`
            : "Recording failed to start. Check `minutes logs` for details.",
        },
      ],
    };
  }
);

// ── Tool: stop_recording ────────────────────────────────────

server.tool(
  "stop_recording",
  "Stop the current recording and process it (transcribe, diarize, summarize).",
  {},
  { title: "Stop Recording", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async () => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }
    try {
      const { stdout, stderr } = await runMinutes(["stop"], 180000);
      const result = parseJsonOutput(stdout);

      const message = result.file
        ? `Recording saved: ${result.file}\nTitle: ${result.title}\nWords: ${result.words}`
        : stderr || "Recording stopped.";

      // Trigger QMD re-index so new meeting is immediately searchable
      if (result.file) triggerQmdIndex();

      return { content: [{ type: "text" as const, text: message }] };
    } catch (error: any) {
      return {
        content: [{ type: "text" as const, text: `Stop failed: ${error.message}` }],
      };
    }
  }
);

// ── Tool: get_status ────────────────────────────────────────

server.tool(
  "get_status",
  "Check if a recording is currently in progress.",
  {},
  { title: "Recording Status", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async () => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: `No recording in progress (read-only mode).\n\n${CLI_INSTALL_MSG}` }] };
    }
    const { stdout } = await runMinutes(["status"]);
    const status = parseJsonOutput(stdout);
    const modeLabel = status.recording_mode === "quick-thought" ? "Quick thought" : "Recording";
    const processingLabel =
      status.recording_mode === "quick-thought" ? "Quick thought processing" : "Processing";
    const text = status.recording
      ? `${modeLabel} in progress (PID: ${status.pid})`
      : status.processing
        ? `${processingLabel}${status.processing_stage ? `: ${status.processing_stage}` : "."}`
        : "No recording in progress.";
    return { content: [{ type: "text" as const, text }] };
  }
);

// ── Tool: list_meetings ─────────────────────────────────────

registerAppTool(
  server,
  "list_meetings",
  {
    description: "List recent meetings and voice memos.",
    inputSchema: {
      limit: z.number().optional().default(10).describe("Maximum results"),
      type: z.enum(["meeting", "memo"]).optional().describe("Filter by type"),
    },
    annotations: { title: "List Meetings", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ limit, type: contentType }) => {
    // Pure-TS fallback when CLI is not available
    if (!(await isCliAvailable())) {
      const meetings = await reader.listMeetings(MEETINGS_DIR, limit);
      const filtered = contentType
        ? meetings.filter((m) => m.frontmatter.type === contentType)
        : meetings;
      const openActions = await reader.findOpenActions(MEETINGS_DIR);

      if (filtered.length === 0) {
        return {
          content: [{ type: "text" as const, text: "No meetings or memos found." }],
          structuredContent: { meetings: [], actions: [], view: "dashboard" },
          _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "dashboard" },
        };
      }

      const text = filtered
        .map((m) => `${m.frontmatter.date} — ${m.frontmatter.title} [${m.frontmatter.type}]\n  ${m.path}`)
        .join("\n\n");

      const meetingsJson = filtered.map((m) => ({
        date: m.frontmatter.date,
        title: m.frontmatter.title,
        content_type: m.frontmatter.type,
        path: m.path,
        duration: m.frontmatter.duration,
      }));

      return {
        content: [{ type: "text" as const, text }],
        structuredContent: { meetings: meetingsJson, actions: openActions.map((a) => a.item), view: "dashboard" },
        _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "dashboard" },
      };
    }

    const args = ["list", "--limit", String(limit)];
    if (contentType) args.push("-t", contentType);

    // Fetch meetings and action items in parallel
    const [meetingsResult, actionsResult] = await Promise.all([
      runMinutes(args),
      runMinutes(["search", "", "--intents-only", "--intent-kind", "action-item", "--limit", "20"]).catch(() => ({ stdout: "[]", stderr: "" })),
    ]);

    const meetings = parseJsonOutput(meetingsResult.stdout);
    let actions: any[] = [];
    const parsedActions = parseJsonOutput(actionsResult.stdout);
    if (Array.isArray(parsedActions)) actions = parsedActions;

    if (Array.isArray(meetings) && meetings.length === 0) {
      return {
        content: [{ type: "text" as const, text: "No meetings or memos found." }],
        structuredContent: { meetings: [], actions, view: "dashboard" },
        _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "dashboard" },
      };
    }

    const text = Array.isArray(meetings)
      ? meetings
          .map((m: any) => `${m.date} — ${m.title} [${m.content_type}]\n  ${m.path}`)
          .join("\n\n")
      : (meetingsResult.stderr || meetingsResult.stdout);

    return {
      content: [{ type: "text" as const, text }],
      structuredContent: { meetings: Array.isArray(meetings) ? meetings : [], actions, view: "dashboard" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "dashboard" },
    };
  }
);

// ── Tool: search_meetings ───────────────────────────────────

registerAppTool(
  server,
  "search_meetings",
  {
    description: "Search meeting transcripts and voice memos.",
    inputSchema: {
      query: z.string().describe("Text to search for"),
      type: z.enum(["meeting", "memo"]).optional().describe("Filter by type"),
      since: z.string().optional().describe("Only results after this date (ISO)"),
      limit: z.number().optional().default(10).describe("Maximum results"),
      intent_kind: z
        .enum(["action-item", "decision", "open-question", "commitment"])
        .optional()
        .describe("Filter structured intents by kind"),
      owner: z.string().optional().describe("Filter structured intents by owner / person"),
      intents_only: z
        .boolean()
        .optional()
        .default(false)
        .describe("Return structured intent records instead of transcript snippets"),
    },
    annotations: { title: "Search Meetings", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ query, type: contentType, since, limit, intent_kind, owner, intents_only }) => {
    // Pure-TS fallback when CLI is not available
    if (!(await isCliAvailable())) {
      const droppedFilters = [since && "since", intent_kind && "intent_kind", owner && "owner", intents_only && "intents_only"].filter(Boolean);
      const filterWarning = droppedFilters.length > 0
        ? `\n\n(Note: ${droppedFilters.join(", ")} filters require the CLI. Install: brew install minutes)`
        : "";

      const results = await reader.searchMeetings(MEETINGS_DIR, query, limit);
      const filtered = contentType
        ? results.filter((m) => m.frontmatter.type === contentType)
        : results;

      if (filtered.length === 0) {
        return {
          content: [{ type: "text" as const, text: `No results for "${query}".${filterWarning}` }],
          structuredContent: { results: [], view: "search" },
          _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "search" },
        };
      }

      const text = filtered
        .map((m) => `${m.frontmatter.date} — ${m.frontmatter.title} [${m.frontmatter.type}]\n  ${m.path}`)
        .join("\n\n") + filterWarning;

      return {
        content: [{ type: "text" as const, text }],
        structuredContent: {
          results: filtered.map((m) => ({
            date: m.frontmatter.date,
            title: m.frontmatter.title,
            content_type: m.frontmatter.type,
            path: m.path,
          })),
          view: "search",
        },
        _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "search" },
      };
    }

    // Intent/metadata queries always use CLI (QMD doesn't index YAML frontmatter fields)
    const useCliOnly = intents_only || intent_kind || owner || since;

    // Try QMD semantic search for text queries
    let results: any[] | null = null;
    let usedQmd = false;

    if (!useCliOnly) {
      results = await searchViaQmd(query, limit, contentType);
      if (results) usedQmd = true;
    }

    // Fall back to CLI regex search
    if (!results) {
      const args = ["search", query, "--limit", String(limit)];
      if (contentType) args.push("-t", contentType);
      if (since) args.push("--since", since);
      if (intent_kind) args.push("--intent-kind", intent_kind);
      if (owner) args.push("--owner", owner);
      if (intents_only) args.push("--intents-only");

      const { stdout, stderr } = await runMinutes(args);
      const parsed = parseJsonOutput(stdout);
      results = Array.isArray(parsed) ? parsed : [];
    }

    if (results.length === 0) {
      return {
        content: [{ type: "text" as const, text: `No results found for "${query}".` }],
        structuredContent: { meetings: [], actions: [], view: "dashboard" },
        _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "dashboard" },
      };
    }

    const text = intents_only
      ? results
          .map(
            (r: any) =>
              `${r.date} — ${r.title} [${r.content_type}]\n  ${r.kind}: ${r.what}${r.who ? ` (@${r.who})` : ""}${r.by_date ? ` by ${r.by_date}` : ""}\n  ${r.path}`
          )
          .join("\n\n")
      : results
          .map(
            (r: any) =>
              `${r.date} — ${r.title} [${r.content_type}]\n  ${r.snippet}\n  ${r.path}`
          )
          .join("\n\n");

    // Map search results to meeting-like objects for the dashboard view
    const meetings = results.map((r: any) => ({
          date: r.date,
          title: r.title,
          content_type: r.content_type,
          path: r.path,
          snippet: r.snippet || (intents_only ? `${r.kind}: ${r.what}` : undefined),
        }));

    return {
      content: [{ type: "text" as const, text }],
      structuredContent: { meetings, actions: [], view: "dashboard" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "dashboard" },
    };
  }
);

// ── Tool: consistency_report ───────────────────────────────

registerAppTool(
  server,
  "consistency_report",
  {
    description: "Flag conflicting decisions and stale commitments across meetings using structured intent data.",
    inputSchema: {
      owner: z.string().optional().describe("Filter stale commitments by owner / person"),
      stale_after_days: z
        .number()
        .optional()
        .default(7)
        .describe("Flag commitments this many days old or older"),
    },
    annotations: { title: "Consistency Report", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ owner, stale_after_days }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: `Consistency reports require the full CLI for structured intent analysis.\n\n${CLI_INSTALL_MSG}` }] };
    }
    const args = ["consistency", "--stale-after-days", String(stale_after_days)];
    if (owner) args.push("--owner", owner);

    const { stdout, stderr } = await runMinutes(args);
    const report = parseJsonOutput(stdout);

    if (!report || typeof report !== "object") {
      return { content: [{ type: "text" as const, text: stderr || stdout }] };
    }

    const decisionConflicts = Array.isArray(report.decision_conflicts)
      ? report.decision_conflicts
      : [];
    const staleCommitments = Array.isArray(report.stale_commitments)
      ? report.stale_commitments
      : [];

    if (decisionConflicts.length === 0 && staleCommitments.length === 0) {
      return {
        content: [{ type: "text" as const, text: "No consistency issues found." }],
        structuredContent: { decision_conflicts: [], stale_commitments: [], view: "report" },
        _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "report" },
      };
    }

    const sections = [];
    if (decisionConflicts.length > 0) {
      sections.push(
        "Decision conflicts:\n" +
          decisionConflicts
            .map(
              (conflict: any) =>
                `- ${conflict.topic}: latest "${conflict.latest.what}" (${conflict.latest.title})`
            )
            .join("\n")
      );
    }
    if (staleCommitments.length > 0) {
      sections.push(
        "Stale commitments:\n" +
          staleCommitments
            .map(
              (stale: any) =>
                `- ${stale.kind}: ${stale.entry.what}${stale.entry.who ? ` (@${stale.entry.who})` : ""} — ${Array.isArray(stale.reasons) ? stale.reasons.join(", ") : `${stale.age_days} days old`}${stale.latest_follow_up ? `; latest follow-up: ${stale.latest_follow_up.title}` : ""}`
            )
            .join("\n")
      );
    }

    return {
      content: [{ type: "text" as const, text: sections.join("\n\n") }],
      structuredContent: { decision_conflicts: decisionConflicts, stale_commitments: staleCommitments, view: "report" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "report" },
    };
  }
);

// ── Tool: get_person_profile ───────────────────────────────

registerAppTool(
  server,
  "get_person_profile",
  {
    description: "Get a rich relationship profile for a person: meetings, commitments, topics, relationship score, and trend. Uses the conversation graph index for instant results.",
    inputSchema: {
      name: z.string().describe("Person / attendee name to profile"),
    },
    annotations: { title: "Person Profile", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ name }) => {
    // Try graph index first (via CLI `minutes people --json`)
    if (await isCliAvailable()) {
      const { stdout } = await runMinutes(["people", "--json"]);
      const people = parseJsonOutput(stdout);

      if (Array.isArray(people)) {
        const nameLower = name.toLowerCase();
        const match = people.find((p: any) =>
          p.name?.toLowerCase().includes(nameLower) ||
          p.slug?.toLowerCase().includes(nameLower)
        );

        if (match) {
          const daysSince = Math.round(match.days_since || 0);
          const last = daysSince < 1 ? "today" : daysSince < 2 ? "yesterday" : `${daysSince}d ago`;
          const sections = [];

          sections.push(`Relationship score: ${(match.score || 0).toFixed(1)} | ${match.meeting_count} meetings | last: ${last}`);

          if (match.losing_touch) {
            sections.push("⚠ LOSING TOUCH — meeting frequency has declined");
          }

          if (match.top_topics?.length > 0) {
            sections.push("Top topics: " + match.top_topics.join(", "));
          }

          if (match.open_commitments > 0) {
            sections.push(`Open commitments: ${match.open_commitments}`);
          }

          return {
            content: [{ type: "text" as const, text: `Profile for ${match.name}:\n\n${sections.join("\n")}` }],
            structuredContent: { ...match, view: "person" },
            _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "person" },
          };
        }
      }

      // Fall back to legacy CLI person command for richer meeting-level data
      const { stdout: legacyOut, stderr } = await runMinutes(["person", name]);
      const profile = parseJsonOutput(legacyOut);

      if (profile && typeof profile === "object") {
        const topics = Array.isArray(profile.top_topics) ? profile.top_topics : [];
        const openIntents = Array.isArray(profile.open_intents) ? profile.open_intents : [];
        const recentMeetings = Array.isArray(profile.recent_meetings) ? profile.recent_meetings : [];

        if (topics.length === 0 && openIntents.length === 0 && recentMeetings.length === 0) {
          return {
            content: [{ type: "text" as const, text: `No profile data found for ${name}.` }],
            structuredContent: { name, top_topics: [], open_intents: [], recent_meetings: [], view: "person" },
            _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "person" },
          };
        }

        const sections = [];
        if (topics.length > 0) sections.push("Top topics:\n" + topics.map((t: any) => `- ${t.topic} (${t.count})`).join("\n"));
        if (openIntents.length > 0) sections.push("Open commitments:\n" + openIntents.map((i: any) => `- ${i.kind}: ${i.what}${i.by_date ? ` by ${i.by_date}` : ""}`).join("\n"));
        if (recentMeetings.length > 0) sections.push("Recent meetings:\n" + recentMeetings.map((m: any) => `- ${m.date} — ${m.title}`).join("\n"));

        return {
          content: [{ type: "text" as const, text: `Profile for ${profile.name}:\n\n${sections.join("\n\n")}` }],
          structuredContent: { ...profile, view: "person" },
          _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "person" },
        };
      }

      return { content: [{ type: "text" as const, text: stderr || legacyOut || `No data found for ${name}.` }] };
    }

    // Pure-TS fallback when CLI is not available
    const profile = await reader.getPersonProfile(MEETINGS_DIR, name);
    const sections = [];
    if (profile.topics.length > 0) sections.push("Topics: " + profile.topics.join(", "));
    if (profile.meetings.length > 0) sections.push("Meetings:\n" + profile.meetings.map((m) => `- ${m.date} — ${m.title}`).join("\n"));
    if (profile.openActions.length > 0) sections.push("Open actions:\n" + profile.openActions.map((a) => `- ${a.task} (${a.status})`).join("\n"));
    const text = sections.length > 0 ? sections.join("\n\n") : `No profile data found for ${name}.`;
    return {
      content: [{ type: "text" as const, text }],
      structuredContent: { name, top_topics: profile.topics.map((t) => ({ topic: t, count: 1 })), open_intents: profile.openActions, recent_meetings: profile.meetings, view: "person" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "person" },
    };
  }
);

// ── Tool: research_topic ────────────────────────────────────

server.tool(
  "research_topic",
  "Research a topic across meetings, decisions, and open follow-ups.",
  {
    query: z.string().describe("Topic or question to investigate across meetings"),
    type: z.enum(["meeting", "memo"]).optional().describe("Filter by type"),
    since: z.string().optional().describe("Only results after this date (ISO)"),
    attendee: z.string().optional().describe("Filter by attendee / person"),
  },
  { title: "Research Topic", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async ({ query, type: contentType, since, attendee }) => {
    if (!(await isCliAvailable())) {
      // Fallback: basic search when CLI is not available
      const results = await reader.searchMeetings(MEETINGS_DIR, query, 20);
      const filtered = contentType ? results.filter((m) => m.frontmatter.type === contentType) : results;
      const text = filtered.length > 0
        ? filtered.map((m) => `${m.frontmatter.date} — ${m.frontmatter.title}\n  ${m.path}`).join("\n\n")
        : `No results for "${query}". (Note: advanced research features require the CLI.)`;
      return { content: [{ type: "text" as const, text }] };
    }

    const args = ["research", query];
    if (contentType) args.push("-t", contentType);
    if (since) args.push("--since", since);
    if (attendee) args.push("--attendee", attendee);

    const { stdout, stderr } = await runMinutes(args);
    const report = parseJsonOutput(stdout);

    if (!report || typeof report !== "object") {
      return { content: [{ type: "text" as const, text: stderr || stdout }] };
    }

    const decisions = Array.isArray(report.related_decisions) ? report.related_decisions : [];
    const openIntents = Array.isArray(report.related_open_intents)
      ? report.related_open_intents
      : [];
    const recentMeetings = Array.isArray(report.recent_meetings)
      ? report.recent_meetings
      : [];
    const topics = Array.isArray(report.related_topics) ? report.related_topics : [];

    if (decisions.length === 0 && openIntents.length === 0 && recentMeetings.length === 0) {
      return {
        content: [
          {
            type: "text" as const,
            text: `No cross-meeting results found for ${query}.`,
          },
        ],
      };
    }

    const sections = [];
    if (topics.length > 0) {
      sections.push(
        "Related topics:\n" +
          topics.map((topic: any) => `- ${topic.topic} (${topic.count})`).join("\n")
      );
    }
    if (decisions.length > 0) {
      sections.push(
        "Recent decisions:\n" +
          decisions
            .map((decision: any) => `- ${decision.date} — ${decision.what} (${decision.title})`)
            .join("\n")
      );
    }
    if (openIntents.length > 0) {
      sections.push(
        "Open follow-ups:\n" +
          openIntents
            .map(
              (intent: any) =>
                `- ${intent.kind}: ${intent.what}${intent.who ? ` (@${intent.who})` : ""}${intent.by_date ? ` by ${intent.by_date}` : ""}`
            )
            .join("\n")
      );
    }
    if (recentMeetings.length > 0) {
      sections.push(
        "Matching meetings:\n" +
          recentMeetings
            .map((meeting: any) => `- ${meeting.date} — ${meeting.title}`)
            .join("\n")
      );
    }

    return {
      content: [
        {
          type: "text" as const,
          text: `Cross-meeting research for ${query}:\n\n${sections.join("\n\n")}`,
        },
      ],
    };
  }
);

// ── Tool: get_meeting ───────────────────────────────────────

registerAppTool(
  server,
  "get_meeting",
  {
    description: "Get the full transcript and details of a specific meeting or memo.",
    inputSchema: {
      path: z.string().describe("Path to the meeting markdown file"),
    },
    annotations: { title: "View Meeting", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ path: filePath }) => {
    try {
      const resolved = validatePathInDirectory(filePath, MEETINGS_DIR, [".md"]);
      const content = await readFile(resolved, "utf-8");
      return {
        content: [{ type: "text" as const, text: content }],
        structuredContent: { path: resolved, view: "detail" },
        _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "detail", path: resolved },
      };
    } catch (error: any) {
      return {
        content: [{ type: "text" as const, text: `Could not read: ${error.message}` }],
      };
    }
  }
);

// ── Tool: process_audio ─────────────────────────────────────

server.tool(
  "process_audio",
  "Process an audio file through the transcription pipeline.",
  {
    file_path: z.string().describe("Path to audio file (.wav, .m4a, .mp3)"),
    type: z.enum(["meeting", "memo"]).optional().default("memo").describe("Content type"),
    title: z.string().optional().describe("Optional title"),
  },
  { title: "Process Audio", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async ({ file_path, type: contentType, title }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }
    const allowedDirs = [
      join(MINUTES_HOME, "inbox"),
      MEETINGS_DIR,
      join(homedir(), "Downloads"),
    ];
    const audioExts = [".wav", ".m4a", ".mp3", ".ogg", ".webm"];

    try {
      const resolved = validatePathInDirectories(file_path, allowedDirs, audioExts);
      const args = ["process", resolved, "-t", contentType];
      if (title) args.push("--title", title);
      const { stdout } = await runMinutes(args, 300000);
      const result = parseJsonOutput(stdout);

      return {
        content: [
          {
            type: "text" as const,
            text: result.file
              ? `Processed: ${result.file}\nTitle: ${result.title}\nWords: ${result.words}`
              : stdout,
          },
        ],
      };
    } catch (error: any) {
      return {
        content: [{ type: "text" as const, text: `Failed: ${error.message}` }],
      };
    }
  }
);

// ── Tool: add_note ───────────────────────────────────────────

server.tool(
  "add_note",
  "Add a note to the current recording. Notes are timestamped and included in the meeting summary. If no recording is active, annotate an existing meeting file with --meeting.",
  {
    text: z.string().describe("The note text (plain text, no markdown needed)"),
    meeting_path: z
      .string()
      .optional()
      .describe("Path to an existing meeting file to annotate (for post-meeting notes)"),
  },
  { title: "Add Note", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async ({ text, meeting_path }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }
    try {
      const args = ["note", text];
      if (meeting_path) {
        const resolved = validatePathInDirectory(
          meeting_path,
          MEETINGS_DIR,
          [".md"]
        );
        args.push("--meeting", resolved);
      }

      const { stdout, stderr } = await runMinutes(args);
      return {
        content: [{ type: "text" as const, text: stderr || stdout || "Note added." }],
      };
    } catch (error: any) {
      return {
        content: [{ type: "text" as const, text: `Note failed: ${error.message}` }],
      };
    }
  }
);

// ── Tool: qmd_collection_status ─────────────────────────────

server.tool(
  "qmd_collection_status",
  "Check whether the Minutes output directory is already registered as a QMD collection.",
  {
    collection: z
      .string()
      .optional()
      .default("minutes")
      .describe("QMD collection name to check"),
  },
  { title: "QMD Status", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async ({ collection }) => {
    const { stdout, stderr } = await runMinutes([
      "qmd",
      "status",
      "--collection",
      collection,
    ]);
    const report = parseJsonOutput(stdout);

    if (!report || typeof report !== "object") {
      return { content: [{ type: "text" as const, text: stderr || stdout }] };
    }

    if (!report.qmd_available) {
      return {
        content: [
          {
            type: "text" as const,
            text: `QMD is not installed or not on PATH. Install qmd, then run register_qmd_collection for "${collection}".`,
          },
        ],
      };
    }

    if (report.registered) {
      return {
        content: [
          {
            type: "text" as const,
            text: `QMD collection "${collection}" already indexes ${report.output_dir}.`,
          },
        ],
      };
    }

    const aliases = Array.isArray(report.matching_collections)
      ? report.matching_collections.map((candidate: any) => candidate.name)
      : [];

    return {
      content: [
        {
          type: "text" as const,
          text:
            aliases.length > 0
              ? `${report.output_dir} is already indexed in QMD under: ${aliases.join(", ")}.`
              : `${report.output_dir} is not indexed in QMD yet.`,
        },
      ],
    };
  }
);

// ── Tool: register_qmd_collection ───────────────────────────

server.tool(
  "register_qmd_collection",
  "Register the Minutes output directory as a QMD collection.",
  {
    collection: z
      .string()
      .optional()
      .default("minutes")
      .describe("QMD collection name to register"),
  },
  { title: "Register QMD", readOnlyHint: false, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async ({ collection }) => {
    const { stdout, stderr } = await runMinutes([
      "qmd",
      "register",
      "--collection",
      collection,
    ]);
    const report = parseJsonOutput(stdout);

    if (!report || typeof report !== "object") {
      return { content: [{ type: "text" as const, text: stderr || stdout }] };
    }

    if (!report.registered) {
      return {
        content: [
          {
            type: "text" as const,
            text: stderr || stdout || `Failed to register QMD collection "${collection}".`,
          },
        ],
      };
    }

    return {
      content: [
        {
          type: "text" as const,
          text: `Registered ${report.output_dir} as QMD collection "${collection}".`,
        },
      ],
    };
  }
);

// ── Tool: track_commitments ─────────────────────────────────

registerAppTool(
  server,
  "track_commitments",
  {
    description: "List open and stale commitments (action items, intents, decisions) across all meetings. Optionally filter by person. Answers: 'What did I promise Sarah?' or 'What's overdue?'",
    inputSchema: {
      person: z.string().optional().describe("Filter by person name or slug (optional — omit for all commitments)"),
    },
    annotations: { title: "Track Commitments", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ person }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: "Minutes CLI not available. Install with: cargo install minutes-cli" }] };
    }

    // Use dedicated commitments command for full text detail
    const args = ["commitments", "--json"];
    if (person) args.push("--person", person);

    const { stdout } = await runMinutes(args);
    const commitments = parseJsonOutput(stdout);

    if (!Array.isArray(commitments) || commitments.length === 0) {
      const scope = person ? ` for ${person}` : "";
      return {
        content: [{ type: "text" as const, text: `No open commitments found${scope}.` }],
        structuredContent: { commitments: [], person: person || null, view: "commitments" },
        _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "commitments" },
      };
    }

    // Group by status
    const stale = commitments.filter((c: any) => c.status === "stale");
    const open = commitments.filter((c: any) => c.status === "open");

    const lines: string[] = [];
    if (stale.length > 0) {
      lines.push(`STALE (${stale.length} overdue):`);
      for (const c of stale) {
        const who = c.person_name || "unassigned";
        lines.push(`  ⚠ ${c.text} (${who}; due: ${c.due_date || "no date"}; from: ${c.meeting_title})`);
      }
    }
    if (open.length > 0) {
      if (stale.length > 0) lines.push("");
      lines.push(`OPEN (${open.length}):`);
      for (const c of open) {
        const who = c.person_name || "unassigned";
        lines.push(`  · ${c.text} (${who}; from: ${c.meeting_title})`);
      }
    }

    const text = `Commitments${person ? ` for ${person}` : ""}:\n\n${lines.join("\n")}`;

    return {
      content: [{ type: "text" as const, text }],
      structuredContent: { commitments, person: person || null, stale_count: stale.length, open_count: open.length, view: "commitments" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "commitments" },
    };
  }
);

// ── Tool: relationship_map ──────────────────────────────────

registerAppTool(
  server,
  "relationship_map",
  {
    description: "Show all contacts with relationship scores, meeting frequency, and 'losing touch' alerts. Overview of your entire conversation network.",
    inputSchema: {
      limit: z.number().optional().describe("Max people to return (default: 15)"),
    },
    annotations: { title: "Relationship Map", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ limit }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: "Minutes CLI not available. Install with: cargo install minutes-cli" }] };
    }

    const maxPeople = limit || 15;
    const { stdout } = await runMinutes(["people", "--json", "--limit", String(maxPeople)]);
    const people = parseJsonOutput(stdout);

    if (!Array.isArray(people) || people.length === 0) {
      return {
        content: [{ type: "text" as const, text: "No relationship data found. Run: minutes people --rebuild" }],
        structuredContent: { people: [], view: "relationship_map" },
        _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "relationship_map" },
      };
    }

    // Format human-readable output
    const lines: string[] = [];
    const losingTouch: string[] = [];

    for (const p of people) {
      const daysSince = Math.round(p.days_since || 0);
      const last = daysSince < 1 ? "today" : daysSince < 2 ? "yesterday" : `${daysSince}d ago`;
      const status = p.losing_touch
        ? "⚠ losing touch"
        : p.open_commitments > 0
          ? `${p.open_commitments} open commitment${p.open_commitments !== 1 ? "s" : ""}`
          : "✓ all clear";

      lines.push(`${p.name} — ${p.meeting_count} meetings, last: ${last}, ${status} (score: ${(p.score || 0).toFixed(1)})`);

      if (p.losing_touch) {
        losingTouch.push(`${p.name} — ${p.meeting_count} meetings total, last seen ${daysSince}d ago`);
      }
    }

    let text = `Relationship Map (${people.length} contacts):\n\n${lines.join("\n")}`;
    if (losingTouch.length > 0) {
      text += `\n\nLosing Touch:\n${losingTouch.join("\n")}`;
    }

    return {
      content: [{ type: "text" as const, text }],
      structuredContent: { people, view: "relationship_map" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "relationship_map" },
    };
  }
);

// ── Resources ───────────────────────────────────────────────

server.resource(
  "recent_meetings",
  "minutes://meetings/recent",
  { description: "List of recent meetings and memos" },
  async () => {
    if (!(await isCliAvailable())) {
      const meetings = await reader.listMeetings(MEETINGS_DIR, 20);
      const json = JSON.stringify(meetings.map((m) => ({
        date: m.frontmatter.date, title: m.frontmatter.title,
        content_type: m.frontmatter.type, path: m.path, duration: m.frontmatter.duration,
      })));
      return { contents: [{ uri: "minutes://meetings/recent", mimeType: "application/json", text: json }] };
    }
    const { stdout } = await runMinutes(["list", "--limit", "20"]);
    return { contents: [{ uri: "minutes://meetings/recent", mimeType: "application/json", text: stdout }] };
  }
);

server.resource(
  "recording_status",
  "minutes://status",
  { description: "Current recording status" },
  async () => {
    if (!(await isCliAvailable())) {
      return { contents: [{ uri: "minutes://status", mimeType: "application/json", text: JSON.stringify({ recording: false, processing: false, note: "Read-only mode (CLI not installed)" }) }] };
    }
    const { stdout } = await runMinutes(["status"]);
    return { contents: [{ uri: "minutes://status", mimeType: "application/json", text: stdout }] };
  }
);

server.resource(
  "open_actions",
  "minutes://actions/open",
  { description: "All open action items across meetings" },
  async () => {
    if (!(await isCliAvailable())) {
      const actions = await reader.findOpenActions(MEETINGS_DIR);
      return { contents: [{ uri: "minutes://actions/open", mimeType: "application/json", text: JSON.stringify(actions) }] };
    }
    const { stdout } = await runMinutes(["search", "", "--intents-only", "--intent-kind", "action-item"]);
    return { contents: [{ uri: "minutes://actions/open", mimeType: "application/json", text: stdout }] };
  }
);

server.resource(
  "recent_events",
  "minutes://events/recent",
  { description: "Recent pipeline events (recordings, processing, notes)" },
  async () => {
    if (!(await isCliAvailable())) {
      return { contents: [{ uri: "minutes://events/recent", mimeType: "application/json", text: "[]" }] };
    }
    const { stdout } = await runMinutes(["events", "--limit", "20"]);
    return { contents: [{ uri: "minutes://events/recent", mimeType: "application/json", text: stdout }] };
  }
);

server.resource(
  "meeting",
  new ResourceTemplate("minutes://meetings/{slug}", { list: undefined }),
  { description: "Get a specific meeting by its filename slug" },
  async (uri, variables) => {
    const slug = String(variables.slug);
    if (!(await isCliAvailable())) {
      // Without CLI resolve, find by filename match
      const meetings = await reader.listMeetings(MEETINGS_DIR, 1000);
      const match = meetings.find((m) => m.path.includes(slug));
      if (match) {
        const content = await readFile(match.path, "utf-8");
        return { contents: [{ uri: uri.href, mimeType: "text/markdown", text: content }] };
      }
      return { contents: [{ uri: uri.href, mimeType: "text/plain", text: `Meeting not found: ${slug}` }] };
    }
    const { stdout } = await runMinutes(["resolve", slug]);
    const parsed = parseJsonOutput(stdout);
    if (parsed.path) {
      const validated = validatePathInDirectory(parsed.path, MEETINGS_DIR, [".md"]);
      const content = await readFile(validated, "utf-8");
      return { contents: [{ uri: uri.href, mimeType: "text/markdown", text: content }] };
    }
    return { contents: [{ uri: uri.href, mimeType: "text/plain", text: `Meeting not found: ${slug}` }] };
  }
);

// ── Resource: recent_ideas (voice memos from last N days) ──

server.resource(
  "recent-ideas",
  "minutes://ideas/recent",
  { description: "Recent voice memos and ideas captured from any device (last 14 days)" },
  async (uri) => {
    const meetings = await reader.listMeetings(MEETINGS_DIR, 200);
    const cutoff = new Date();
    cutoff.setDate(cutoff.getDate() - 14);

    const memos = meetings.filter((m) => {
      if (m.frontmatter.type !== "memo") return false;
      const date = new Date(m.frontmatter.date);
      return date >= cutoff;
    });

    if (memos.length === 0) {
      return {
        contents: [{
          uri: uri.href,
          mimeType: "text/plain",
          text: "No voice memos in the last 14 days.",
        }],
      };
    }

    const lines = memos
      .sort((a, b) => new Date(b.frontmatter.date).getTime() - new Date(a.frontmatter.date).getTime())
      .slice(0, 20)
      .map((m) => {
        const date = new Date(m.frontmatter.date).toLocaleDateString("en-US", {
          month: "short",
          day: "numeric",
        });
        const device = m.frontmatter.device ? ` (${m.frontmatter.device})` : "";
        return `- [${date}] ${m.frontmatter.title}${device} — ${m.frontmatter.duration}`;
      })
      .join("\n");

    return {
      contents: [{
        uri: uri.href,
        mimeType: "text/plain",
        text: `Recent voice memos (${memos.length} in last 14 days):\n\n${lines}`,
      }],
    };
  }
);

// ── Tool: start_dictation ──────────────────────────────────

server.tool(
  "start_dictation",
  "Start dictation mode. Speak naturally — text goes to clipboard and daily note after each pause. Runs until stop_dictation is called or silence timeout.",
  {},
  { title: "Start Dictation", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async () => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }
    const { stdout: statusOut } = await runMinutes(["status"]);
    const status = parseJsonOutput(statusOut);
    if (status.recording) {
      return {
        content: [
          {
            type: "text" as const,
            text: "Recording in progress — stop recording before dictating.",
          },
        ],
      };
    }

    // Spawn detached dictation process
    const child = spawn(MINUTES_BIN, ["dictate"], {
      detached: true,
      stdio: "ignore",
      env: { ...process.env, RUST_LOG: "info" },
    });
    child.unref();

    // Wait briefly for startup
    await new Promise((r) => setTimeout(r, 500));

    return {
      content: [
        {
          type: "text" as const,
          text: "Dictation started. Speak naturally — text will be copied to clipboard after each pause. Say \"stop dictation\" when done.",
        },
      ],
    };
  }
);

// ── Tool: stop_dictation ───────────────────────────────────

server.tool(
  "stop_dictation",
  "Stop the current dictation session.",
  {},
  { title: "Stop Dictation", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async () => {
    // Send stop signal by killing the dictation process via PID file
    const minutesDir = join(homedir(), ".minutes");
    const pidPath = join(minutesDir, "dictation.pid");
    if (existsSync(pidPath)) {
      try {
        const pidContent = await readFile(pidPath, "utf-8");
        const pid = parseInt(pidContent.trim(), 10);
        if (Number.isFinite(pid) && pid > 0) {
          process.kill(pid, "SIGTERM");
        }
      } catch {
        // Process already dead or PID file invalid
      }
    }

    return {
      content: [
        {
          type: "text" as const,
          text: "Dictation stop requested.",
        },
      ],
    };
  }
);

// ── Tool: list_voices ────────────────────────────────────────

server.tool(
  "list_voices",
  "List enrolled voice profiles for speaker identification. Shows who has been enrolled, sample count, and model version.",
  {},
  { title: "Voice Profiles", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async () => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: "Minutes CLI not available." }] };
    }

    const { stdout, stderr } = await runMinutes(["voices", "--json"]);
    const profiles = parseJsonOutput(stdout);

    if (!Array.isArray(profiles) || profiles.length === 0) {
      return {
        content: [{ type: "text" as const, text: "No voice profiles enrolled. The user can enroll with: minutes enroll" }],
      };
    }

    const lines = profiles.map((p: any) =>
      `${p.name} — ${p.sample_count} samples, ${p.source} (${p.model_version})`
    );

    return {
      content: [{ type: "text" as const, text: `Voice profiles (${profiles.length}):\n\n${lines.join("\n")}` }],
      structuredContent: { profiles, view: "voices" },
    };
  }
);

// ── Tool: confirm_speaker ────────────────────────────────────

server.tool(
  "confirm_speaker",
  "Confirm or correct a speaker attribution in a meeting. Promotes the attribution to High confidence and rewrites the transcript label. Optionally saves the speaker's voice profile for future meetings.",
  {
    meeting: z.string().describe("Path to the meeting markdown file"),
    speaker_label: z.string().describe("Speaker label to confirm (e.g., SPEAKER_1)"),
    name: z.string().describe("Real name to assign to this speaker"),
    save_voice: z.boolean().optional().default(false).describe("Save this speaker's voice profile for future automatic identification"),
  },
  { title: "Confirm Speaker", readOnlyHint: false, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async ({ meeting, speaker_label, name, save_voice }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: "Minutes CLI not available." }] };
    }

    const args = ["confirm", "--meeting", meeting, "--speaker", speaker_label, "--name", name];
    if (save_voice) args.push("--save-voice");

    try {
      const { stdout, stderr } = await runMinutes(args);
      const output = (stderr || stdout || "").trim();

      return {
        content: [{ type: "text" as const, text: output || `Confirmed: ${speaker_label} = ${name}` }],
        structuredContent: { meeting, speaker_label, name, save_voice, confirmed: true },
      };
    } catch (error: any) {
      const msg = error?.stderr || error?.message || String(error);
      return {
        content: [{ type: "text" as const, text: `Failed to confirm speaker: ${msg}` }],
        isError: true,
      };
    }
  }
);

// ── Start server ────────────────────────────────────────────

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
  console.error("Minutes MCP server running on stdio");
}

main().catch((error) => {
  console.error("Fatal error:", error);
  process.exit(1);
});
