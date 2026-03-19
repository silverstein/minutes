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
 *
 * All tools use execFile (not exec) to shell out to the `minutes` CLI binary.
 * No shell interpolation — safe from injection.
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { execFile, spawn } from "child_process";
import { promisify } from "util";
import { existsSync, realpathSync } from "fs";
import { readFile } from "fs/promises";
import { extname, isAbsolute, join, relative, resolve } from "path";
import { homedir } from "os";

const execFileAsync = promisify(execFile);

// ── Find the minutes binary ─────────────────────────────────

function findMinutesBinary(): string {
  const candidates = [
    join(__dirname, "..", "..", "..", "target", "release", "minutes"),
    join(__dirname, "..", "..", "..", "target", "debug", "minutes"),
    join(homedir(), ".cargo", "bin", "minutes"),
    "/opt/homebrew/bin/minutes",
    "/usr/local/bin/minutes",
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
  version: "0.1.0",
});

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
  async ({ title, mode }) => {
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
  async () => {
    try {
      const { stdout, stderr } = await runMinutes(["stop"], 180000);
      const result = parseJsonOutput(stdout);

      const message = result.file
        ? `Recording saved: ${result.file}\nTitle: ${result.title}\nWords: ${result.words}`
        : stderr || "Recording stopped.";

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
  async () => {
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

server.tool(
  "list_meetings",
  "List recent meetings and voice memos.",
  {
    limit: z.number().optional().default(10).describe("Maximum results"),
    type: z.enum(["meeting", "memo"]).optional().describe("Filter by type"),
  },
  async ({ limit, type: contentType }) => {
    const args = ["list", "--limit", String(limit)];
    if (contentType) args.push("-t", contentType);

    const { stdout, stderr } = await runMinutes(args);
    const meetings = parseJsonOutput(stdout);

    if (Array.isArray(meetings) && meetings.length === 0) {
      return { content: [{ type: "text" as const, text: "No meetings or memos found." }] };
    }

    const text = Array.isArray(meetings)
      ? meetings
          .map((m: any) => `${m.date} — ${m.title} [${m.content_type}]\n  ${m.path}`)
          .join("\n\n")
      : (stderr || stdout);

    return { content: [{ type: "text" as const, text }] };
  }
);

// ── Tool: search_meetings ───────────────────────────────────

server.tool(
  "search_meetings",
  "Search meeting transcripts and voice memos.",
  {
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
  async ({ query, type: contentType, since, limit, intent_kind, owner, intents_only }) => {
    const args = ["search", query, "--limit", String(limit)];
    if (contentType) args.push("-t", contentType);
    if (since) args.push("--since", since);
    if (intent_kind) args.push("--intent-kind", intent_kind);
    if (owner) args.push("--owner", owner);
    if (intents_only) args.push("--intents-only");

    const { stdout, stderr } = await runMinutes(args);
    const results = parseJsonOutput(stdout);

    if (Array.isArray(results) && results.length === 0) {
      return {
        content: [{ type: "text" as const, text: `No results found for "${query}".` }],
      };
    }

    const text = Array.isArray(results)
      ? intents_only
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
            .join("\n\n")
      : (stderr || stdout);

    return { content: [{ type: "text" as const, text }] };
  }
);

// ── Tool: consistency_report ───────────────────────────────

server.tool(
  "consistency_report",
  "Flag conflicting decisions and stale commitments across meetings using structured intent data.",
  {
    owner: z.string().optional().describe("Filter stale commitments by owner / person"),
    stale_after_days: z
      .number()
      .optional()
      .default(7)
      .describe("Flag commitments this many days old or older"),
  },
  async ({ owner, stale_after_days }) => {
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
      return { content: [{ type: "text" as const, text: "No consistency issues found." }] };
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

    return { content: [{ type: "text" as const, text: sections.join("\n\n") }] };
  }
);

// ── Tool: get_person_profile ───────────────────────────────

server.tool(
  "get_person_profile",
  "Build a first-pass profile for a person across meetings using structured intent data.",
  {
    name: z.string().describe("Person / attendee name to profile"),
  },
  async ({ name }) => {
    const { stdout, stderr } = await runMinutes(["person", name]);
    const profile = parseJsonOutput(stdout);

    if (!profile || typeof profile !== "object") {
      return { content: [{ type: "text" as const, text: stderr || stdout }] };
    }

    const topics = Array.isArray(profile.top_topics) ? profile.top_topics : [];
    const openIntents = Array.isArray(profile.open_intents) ? profile.open_intents : [];
    const recentMeetings = Array.isArray(profile.recent_meetings)
      ? profile.recent_meetings
      : [];

    if (topics.length === 0 && openIntents.length === 0 && recentMeetings.length === 0) {
      return { content: [{ type: "text" as const, text: `No profile data found for ${name}.` }] };
    }

    const sections = [];
    if (topics.length > 0) {
      sections.push(
        "Top topics:\n" +
          topics.map((topic: any) => `- ${topic.topic} (${topic.count})`).join("\n")
      );
    }
    if (openIntents.length > 0) {
      sections.push(
        "Open commitments/actions:\n" +
          openIntents
            .map(
              (intent: any) =>
                `- ${intent.kind}: ${intent.what}${intent.by_date ? ` by ${intent.by_date}` : ""}`
            )
            .join("\n")
      );
    }
    if (recentMeetings.length > 0) {
      sections.push(
        "Recent meetings:\n" +
          recentMeetings
            .map((meeting: any) => `- ${meeting.date} — ${meeting.title}`)
            .join("\n")
      );
    }

    return {
      content: [
        {
          type: "text" as const,
          text: `Profile for ${profile.name}:\n\n${sections.join("\n\n")}`,
        },
      ],
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
  async ({ query, type: contentType, since, attendee }) => {
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

server.tool(
  "get_meeting",
  "Get the full transcript and details of a specific meeting or memo.",
  {
    path: z.string().describe("Path to the meeting markdown file"),
  },
  async ({ path: filePath }) => {
    try {
      const resolved = validatePathInDirectory(filePath, join(homedir(), "meetings"), [".md"]);
      const content = await readFile(resolved, "utf-8");
      return { content: [{ type: "text" as const, text: content }] };
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
  async ({ file_path, type: contentType, title }) => {
    const allowedDirs = [
      join(homedir(), ".minutes", "inbox"),
      join(homedir(), "meetings"),
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
  async ({ text, meeting_path }) => {
    try {
      const args = ["note", text];
      if (meeting_path) {
        const resolved = validatePathInDirectory(
          meeting_path,
          join(homedir(), "meetings"),
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
