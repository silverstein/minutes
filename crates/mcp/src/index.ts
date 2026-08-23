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
 *   - add_note: Add a timestamped note to the active recording
 *   - activity_summary: Summarize meeting-adjacent desktop context for a session/path/window
 *   - search_context: Search app and captured window-title desktop context
 *   - get_moment: Show the local rewind around a linked artifact, session, or timestamp
 *   - get_screen_context: Retrieve bounded, session-linked screen images
 *   - consistency_report: Flag conflicting decisions and stale commitments
 *   - get_person_profile: Policy-authorized live profile for a person
 *   - track_commitments: Live open/stale action and intent commitments
 *   - relationship_map: bounded process-private policy-fresh core projection
 *   - research_topic: Cross-meeting topic research
 *   - list_voices: List enrolled voice profiles for speaker identification
 *   - confirm_speaker: Compatibility registration; directs humans to the app/CLI
 *   - get_meeting_insights: Query structured insights (decisions, commitments, etc.) with confidence filtering
 *   - start_copilot / stop_copilot: Control the independent real-time copilot engine
 *   - copilot_status / read_copilot_nudges: Observe copilot health and cursor-based nudges
 *
 * All tools use execFile (not exec) to shell out to the `minutes` CLI binary.
 * No shell interpolation — safe from injection.
 */

// ── Crash tracer must load before any other import (see ./crashTracer.ts) ──
// Issue #149 — Claude Desktop 1.3109.0 with MCP protocol 2025-11-25 kills
// the extension server with no stderr visible in the host log. The tracer
// writes synchronously to ~/.minutes/logs/mcp-crash.log so a reinstall
// produces a real trace instead of a silent exit.
import { crashTrace, CRASH_LOG_PATH } from "./crashTracer.js";

import { McpServer, ResourceTemplate } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  ErrorCode,
  McpError,
  SubscribeRequestSchema,
  UnsubscribeRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import {
  registerAppTool,
  registerAppResource,
  RESOURCE_MIME_TYPE,
  EXTENSION_ID,
} from "@modelcontextprotocol/ext-apps/server";
import { z } from "zod";
import { execFile, spawn, spawnSync } from "child_process";
import { createHash } from "crypto";
import { promisify } from "util";
import {
  closeSync,
  constants,
  copyFileSync,
  existsSync,
  fchmodSync,
  fstatSync,
  lstatSync,
  mkdirSync,
  openSync,
  readdirSync,
  realpathSync,
  statSync,
} from "fs";
import { mkdir, readFile, rm, stat, writeFile } from "fs/promises";
import { basename, delimiter, dirname, extname, isAbsolute, join, relative, resolve } from "path";
import { fileURLToPath } from "url";
import { homedir } from "os";
import { parse as parseYaml } from "yaml";

import * as reader from "minutes-sdk";
import {
  canonicalizeRoot,
  expandHomeLikePath,
  readTextFileInDirectory,
  validatePathInDirectories,
  validatePathInDirectory,
} from "./paths.js";
import { isCliCompatible } from "./version.js";
import { nodeChildEnvironment } from "./node-child.js";
import {
  hasFeature,
  probeCapabilitiesSync,
  type CapabilityProbeResult,
} from "./capabilities.js";
import {
  downloadReleaseBinaryWithChecksum,
  extractZipWithPowerShell,
} from "./autoInstall.js";
import {
  attachCaptureRelay,
  type CaptureRelayCursor,
  type CaptureRelaySnapshot,
} from "./captureRelay.js";
import {
  withStableCorpusLease,
  type CorpusLeaseHooks,
  type StableCorpusSnapshot,
} from "./corpus-lease.js";
import {
  HTTP_BIND_HOST,
  DEFAULT_HTTP_PORT,
  DEFAULT_MAX_SESSIONS,
  startMinutesHttpServer,
} from "./httpTransport.js";
import {
  boundReadFingerprint,
  captureBoundReadExpectation,
  readTextFileFromBoundParent,
  type BoundReadExpectation,
  type BoundReadHooks,
} from "./secure-read.js";

crashTrace("imports-complete");

// ── Demo mode (--demo flag) ────────────────────────────────
// `npx minutes-mcp --demo` is a one-shot setup: copies bundled fixture
// meetings to ~/.minutes/demo/, prints the MCP config snippet with an explicit
// MEETINGS_DIR env override, prints suggested questions, and exits 0.
//
// The printed config uses env:{ MEETINGS_DIR } pointing at the demo dir. No
// separate --demo flag at runtime. The MCP host just launches standard
// `minutes-mcp`; the env override is what routes it at the demo corpus. This
// avoids the TTY-detection ambiguity that an earlier dual-mode design had.
//
// Guarded on `--demo` AND on being the actual entry point so importers don't
// trigger disk side effects by mistake. Use the same realpath-aware guard as
// `main()` so npm/.bin shims and symlinked entrypoints still execute demo mode.
if (process.argv.includes("--demo") && shouldRunMainEntry(process.argv[1], fileURLToPath(import.meta.url))) {
  handleDemoSetup();
}

function handleDemoSetup(): void {
  const demoDir = join(homedir(), ".minutes", "demo");
  const here = dirname(fileURLToPath(import.meta.url));
  // Package layout after build: dist/index.js; fixtures live at
  // <pkg>/fixtures/demo/ next to dist/.
  const fixturesSrc = resolve(here, "..", "fixtures", "demo");

  if (!existsSync(fixturesSrc)) {
    console.error(
      `[minutes-mcp --demo] bundled fixtures not found at ${fixturesSrc}. ` +
        `This build of minutes-mcp is missing the demo corpus. ` +
        `Try upgrading with: npm install -g minutes-mcp@latest`
    );
    process.exit(1);
  }

  mkdirSync(demoDir, { recursive: true });
  for (const entry of readdirSync(fixturesSrc)) {
    if (!entry.endsWith(".md")) continue;
    copyFileSync(join(fixturesSrc, entry), join(demoDir, entry));
  }

  // The config snippet embeds the fully-resolved demoDir so users don't have
  // to fill it in manually. MCP hosts inject this env when launching the
  // server; the server's existing MEETINGS_DIR logic (line ~800) picks it up.
  const configSnippet = JSON.stringify(
    {
      mcpServers: {
        "minutes-demo": {
          command: "npx",
          args: ["minutes-mcp"],
          env: {
            MEETINGS_DIR: demoDir,
          },
        },
      },
    },
    null,
    2
  );

  console.log("");
  console.log("Demo corpus ready at: " + demoDir);
  console.log("5 fixture meetings with a pricing reversal, a customer commitment that slips, and a feature cut.");
  console.log("");
  console.log("═══ MCP config (paste into Claude Desktop, Cursor, Claude Code, or any MCP client) ═══");
  console.log(configSnippet);
  console.log("");
  console.log("═══ Try asking your agent ═══");
  console.log("  • List the meetings in this corpus.");
  console.log("  • What did we decide about pricing? Which decision is current?");
  console.log("  • What got killed in the last product prioritization meeting?");
  console.log("  • What action items are still open, and who owns each?");
  console.log("  • Summarize the Northwind customer thread.");
  console.log("");
  console.log("Note: some structured tools (consistency report, person profile) auto-install the Minutes CLI on first use.");
  console.log("Full setup (real audio capture, transcription, real meetings): https://useminutes.app");
  console.log("");
  process.exit(0);
}

const UI_RESOURCE_URI = "ui://minutes/dashboard";
const MCP_TOOLS_DOCS_BASE_URL = "https://useminutes.app/docs/mcp/tools";
export const MEETING_INSIGHT_KINDS = ["decision", "commitment", "question"] as const;
export type KnowledgeConfigStatus = {
  enabled: boolean;
  path?: string;
  adapter: string;
  engine: string;
};

export const MCP_MEETING_RESULT_MAX = 50;
export const MCP_ACTION_RESULT_MAX = 50;
/** Maximum live meetings materialized for one exported policy projection. */
export const MCP_POLICY_MEETING_RESULT_MAX = 5_000;
/** Independent structured/text collection caps for derived MCP surfaces. */
export const MCP_INTENT_RESULT_MAX = 50;
/**
 * Largest number of insight records one read may return. Bounded like every
 * peer surface so a caller cannot request an unbounded projection. This caps
 * the answer; the number of records examined is MCP_INSIGHT_SCAN_WINDOW below,
 * which is deliberately not caller-controlled.
 */
export const MCP_INSIGHT_RESULT_MAX = 500;
/**
 * How many of the newest insight records one read examines, whatever the
 * caller asked for.
 *
 * This is a constant on purpose. A withheld tally computed over a
 * caller-shaped window can be differenced across two calls to read a single
 * record's policy verdict: ask for the newest k, ask for the newest k+1, and
 * whichever counter moved names the verdict on the (k+1)-th record. Sweeping k
 * maps every restricted meeting in the log. Holding the scanned window fixed
 * makes the tally a function of corpus state alone.
 *
 * Scope that claim precisely, because it is easy to overstate and was. This
 * closes differencing across CALLER ARGUMENTS only. It does not close
 * differencing across CORPUS STATE: observe the tally, cause one record to be
 * appended or one meeting's designation to change, observe again, and the delta
 * names that record's verdict. The tally is also an aggregate restricted count
 * whenever every record in the window has a resolvable source. Both are
 * accepted leaks, not absent ones. Removing the exact count would close them
 * and would not cost the partial-view contract, which needs only the boolean;
 * it is kept because an agent that cannot tell "two withheld" from "two hundred
 * withheld" cannot judge how incomplete its answer is.
 *
 * It is set equal to the largest permitted `limit` so that no request can
 * widen it, and so that applying `since` in this process returns exactly the
 * records the CLI would have returned for the same `limit`: `since` is a lower
 * bound on time, so the newest N records that satisfy it are the same set
 * whether it is applied before or after taking the newest N.
 *
 * That last equivalence has a premise worth naming, because it is a property of
 * this log rather than of `since`. The CLI filters by timestamp but orders and
 * tail-limits by `seq` (crates/core/src/events.rs), so "newest N" and "newest N
 * by time" coincide only while the two orders agree. They do on the real log
 * today: zero inversions among insight records. A restored or merged log with
 * mixed `seq` would break it, and the symptom would be a `since` query missing
 * a record that the CLI would have returned.
 */
export const MCP_INSIGHT_SCAN_WINDOW = MCP_INSIGHT_RESULT_MAX;
export const MCP_PERSON_PROFILE_MEETING_MAX = 50;
export const MCP_PERSON_PROFILE_OPEN_ACTION_MAX = 50;
export const MCP_PERSON_PROFILE_TOPIC_MAX = 50;
export const MCP_PERSON_PROFILE_DECISION_MAX = 50;
export const MCP_RELATIONSHIP_RESULT_MAX = 50;
export const MCP_PROCESSING_JOB_RESULT_MAX = 50;
export const MCP_RESEARCH_MEETING_RESULT_MAX = 20;
export const MCP_RESEARCH_DECISION_RESULT_MAX = 50;
export const MCP_RESEARCH_TOPIC_RESULT_MAX = 50;
const MCP_RESULT_FIELD_MAX_CHARS = 2_048;
const MCP_TEXT_OUTPUT_MAX_CHARS = 256 * 1024;
const MCP_QUERY_MAX_CHARS = 2_048;

type MeetingLike = {
  path: string;
  frontmatter: {
    date?: string;
    title?: string;
    type?: string;
    duration?: string;
    recording_health?: unknown;
  };
};

export function meetingListItem(meeting: MeetingLike) {
  return {
    date: boundedMcpField(meeting.frontmatter.date),
    title: boundedMcpField(meeting.frontmatter.title),
    content_type: boundedMcpField(meeting.frontmatter.type),
    path: boundedMcpField(meeting.path),
    duration: boundedMcpField(meeting.frontmatter.duration),
  };
}

export function meetingSearchItem(meeting: MeetingLike) {
  return {
    date: boundedMcpField(meeting.frontmatter.date),
    title: boundedMcpField(meeting.frontmatter.title),
    content_type: boundedMcpField(meeting.frontmatter.type),
    path: boundedMcpField(meeting.path),
  };
}

function boundedMcpField(value: string | undefined): string | undefined {
  return value?.slice(0, MCP_RESULT_FIELD_MAX_CHARS);
}

function boundedMcpText(value: string): string {
  return value.slice(0, MCP_TEXT_OUTPUT_MAX_CHARS);
}

function boundedMcpJsonArray(values: unknown[]): string {
  const serialized: string[] = [];
  let charLength = 2;
  for (const value of values) {
    const item = JSON.stringify(value);
    const nextLength = charLength + item.length + (serialized.length > 0 ? 1 : 0);
    if (nextLength > MCP_TEXT_OUTPUT_MAX_CHARS) break;
    serialized.push(item);
    charLength = nextLength;
  }
  return `[${serialized.join(",")}]`;
}

function normalizeMcpResultLimit(
  limit: number,
  max: number,
  surface: string
): number {
  if (
    !Number.isSafeInteger(limit) ||
    limit < 1 ||
    limit > max
  ) {
    throw new McpError(
      ErrorCode.InvalidParams,
      `${surface} limit must be an integer between 1 and ${max}`
    );
  }
  return limit;
}

export function normalizeMcpMeetingResultLimit(limit: number): number {
  return normalizeMcpResultLimit(limit, MCP_MEETING_RESULT_MAX, "meeting result");
}

/**
 * Pull the text of a top-level markdown section (e.g. `## Summary`) out of a
 * meeting body, stopping at the next `## ` heading. Returns undefined when the
 * section is absent or empty. Used to surface the synthesized summary in
 * get_meeting's structuredContent without re-parsing the whole transcript.
 */
export function extractMarkdownSection(
  body: string | undefined,
  heading: string
): string | undefined {
  if (!body) return undefined;
  const lines = body.split(/\r?\n/);
  const start = lines.findIndex((line) => line.trim() === `## ${heading}`);
  if (start === -1) return undefined;

  const collected: string[] = [];
  for (let i = start + 1; i < lines.length; i++) {
    if (/^##\s/.test(lines[i])) break;
    collected.push(lines[i]);
  }

  const text = collected.join("\n").trim();
  return text.length > 0 ? text : undefined;
}

export function meetingDetailPayload(input: {
  path: string;
  speaker_map?: unknown;
  recording_health?: unknown;
  overlay_applied?: boolean;
  title?: unknown;
  summary?: string;
  action_items?: unknown;
  decisions?: unknown;
  intents?: unknown;
  body?: string;
}) {
  const payload: {
    path: string;
    view: "detail";
    title?: unknown;
    summary?: string;
    action_items?: unknown;
    decisions?: unknown;
    intents?: unknown;
    speaker_map?: unknown;
    recording_health?: unknown;
    overlay_applied?: boolean;
    body?: string;
  } = {
    path: input.path,
    view: "detail",
  };

  if (input.title !== undefined) {
    payload.title = input.title;
  }
  if (input.summary !== undefined) {
    payload.summary = input.summary;
  }
  if (input.action_items !== undefined) {
    payload.action_items = input.action_items;
  }
  if (input.decisions !== undefined) {
    payload.decisions = input.decisions;
  }
  if (input.intents !== undefined) {
    payload.intents = input.intents;
  }
  if (input.speaker_map !== undefined) {
    payload.speaker_map = input.speaker_map;
  }
  if (input.recording_health !== undefined) {
    payload.recording_health = input.recording_health;
  }
  if (input.overlay_applied !== undefined) {
    payload.overlay_applied = input.overlay_applied;
  }
  if (input.body !== undefined) {
    payload.body = input.body;
  }

  return payload;
}

/** Accept CLI speaker overlays only when the CLI proves they were selected
 * for the exact Markdown bytes already authorized by this MCP process.
 * Older CLIs omit the proof field and therefore degrade to the raw map. */
export function verifiedCliSpeakerOverlay(
  payload: any,
  authorizedContent: string
): { speaker_map: unknown[]; overlay_applied: true } | null {
  if (
    payload?.overlay_applied !== true ||
    !Array.isArray(payload?.frontmatter?.speaker_map) ||
    payload?.raw_markdown !== authorizedContent ||
    payload?.overlay_source_sha256 !==
      createHash("sha256").update(authorizedContent).digest("hex")
  ) {
    return null;
  }
  return {
    speaker_map: payload.frontmatter.speaker_map,
    overlay_applied: true,
  };
}

function toolDocsUrl(name: string): string {
  return `${MCP_TOOLS_DOCS_BASE_URL}#tool-${name}`;
}

function withToolDocs(name: string, description: string): string {
  return `${description} Docs: ${toolDocsUrl(name)}`;
}

export type RestrictedContentPolicy = "logged-override" | "deny";
const MAX_RESTRICTED_OVERRIDE_AUDIT_BYTES = 16 * 1024;

export type RestrictedAuditWriter = (auditPath: string, line: string) => void;

/**
 * Native Recall is unattended: a model-selected tool argument is not human
 * consent. Standalone MCP keeps the documented explicit/logged override, but
 * an embedding surface can set this process policy to make every current and
 * future tool reject `include_restricted: true` at the registration boundary.
 */
export function restrictedContentPolicyFromEnv(
  value: string | undefined = process.env.MINUTES_MCP_RESTRICTED_POLICY,
  _platform: NodeJS.Platform = process.platform
): RestrictedContentPolicy {
  const normalized = value?.trim().toLowerCase();
  // The Rust CLI bridge retains an exact no-follow capability chain and an
  // owner-only audit leaf on every supported platform, including a protected
  // Windows DACL and non-delete-sharing handle.
  if (normalized === "logged-override") {
    return "logged-override";
  }
  // Absence, `deny`, and misspelled/future values all fail closed. Enabling
  // the standalone override requires an operator-controlled launch setting.
  return "deny";
}

export function enforceRestrictedContentPolicy(
  input: unknown,
  surface: string,
  policy: RestrictedContentPolicy = restrictedContentPolicyFromEnv(),
  auditPath: string = sensitivityOverrideAuditPath(),
  auditWriter: RestrictedAuditWriter = appendDurableRestrictedOverrideAudit
): void {
  if (
    input !== null &&
    typeof input === "object" &&
    (input as Record<string, unknown>).include_restricted === true
  ) {
    if (policy === "deny") {
      throw new McpError(
        ErrorCode.InvalidParams,
        `Restricted meeting content is unavailable in this ${surface} session. ` +
          "A human operator must launch the server with " +
          "MINUTES_MCP_RESTRICTED_POLICY=logged-override to enable audited overrides."
      );
    }
    try {
      const auditScope = restrictedOverrideAuditScope(
        input as Record<string, unknown>
      );
      auditWriter(
        auditPath,
        `${JSON.stringify({
          v: 1,
          event: "sensitivity.override",
          recorded_at: new Date().toISOString(),
          surface,
          authorization: "operator-launch-policy+tool-argument",
          scope_fields: auditScope.fields,
          scope_sha256: auditScope.sha256,
          pid: process.pid,
        })}\n`
      );
      console.error(
        `[Minutes] operator-authorized include_restricted request via ${surface}; audit recorded`
      );
    } catch {
      throw new McpError(
        ErrorCode.InternalError,
        "Restricted override denied because its audit record could not be written safely."
      );
    }
  }
}

function appendDurableRestrictedOverrideAudit(
  _auditPath: string,
  line: string
): void {
  const record = Buffer.from(line, "utf8");
  if (
    record.length === 0 ||
    record.length > MAX_RESTRICTED_OVERRIDE_AUDIT_BYTES
  ) {
    throw new Error("override audit record exceeded its bounded size");
  }
  const child = spawnSync(MINUTES_BIN, ["policy-audit"], {
    env: mcpCliChildEnv(),
    input: record,
    encoding: "buffer",
    timeout: 5_000,
    maxBuffer: MAX_RESTRICTED_OVERRIDE_AUDIT_BYTES,
    windowsHide: true,
  });
  if (child.error || child.status !== 0 || child.signal !== null) {
    throw new Error("override audit append failed");
  }
}

function restrictedOverrideAuditScope(input: Record<string, unknown>): {
  fields: string[];
  sha256: string;
} {
  const scoped = Object.entries(input)
    .filter(([key, value]) => key !== "include_restricted" && value !== undefined)
    .sort(([left], [right]) => left.localeCompare(right));
  return {
    fields: scoped.map(([key]) => key),
    // The audit correlates repeated/scoped authorization without copying a
    // meeting path, person, or query into a durable diagnostics file.
    sha256: createHash("sha256").update(JSON.stringify(scoped)).digest("hex"),
  };
}

export function sensitivityOverrideAuditPath(): string {
  const minutesHome = expandHomeLikePath(
    process.env.MINUTES_HOME || join(homedir(), ".minutes")
  );
  return join(minutesHome, "audit", "sensitivity-overrides.jsonl");
}

// These operations return user-authored or user-derived content to the agent.
// Readiness is deliberately checked for every invocation: the local registry
// can change after MCP startup, so a successful startup probe is not durable
// authorization for a later read.
const CONTENT_BEARING_AGENT_TOOLS = new Set([
  "list_processing_jobs",
  "list_meetings",
  "search_meetings",
  "activity_summary",
  "search_context",
  "get_moment",
  "get_screen_context",
  "consistency_report",
  "get_person_profile",
  "research_topic",
  "get_meeting",
  "track_commitments",
  "relationship_map",
  "process_audio",
  "list_voices",
  "confirm_speaker",
  "start_copilot",
  "read_live_transcript",
  "ingest_meeting",
  // Returns decision and commitment text, owners, deadlines and participant
  // names drawn from meetings, so it belongs behind the same per-call
  // readiness gate as every peer above. Omitting it meant a machine whose
  // registry had degraded withheld track_commitments while serving the same
  // content through this tool.
  "get_meeting_insights",
]);

export function isContentBearingAgentTool(name: string): boolean {
  return CONTENT_BEARING_AGENT_TOOLS.has(name);
}

export function contentBearingAgentToolNames(): string[] {
  return [...CONTENT_BEARING_AGENT_TOOLS].sort();
}

export async function afterContentBearingToolReadiness<T>(
  name: string,
  operation: () => T | Promise<T>,
  readiness: () => Promise<unknown> = () => requireAgentTrustReadiness()
): Promise<T> {
  if (isContentBearingAgentTool(name)) {
    await readiness();
  }
  return operation();
}

/// Resource handlers do not pass through the tool registry above. Every
/// resource that can expose user-authored or derived content must therefore
/// use this per-read boundary explicitly; a successful MCP connection is not
/// durable authorization after the QMD registry changes.
const CONTENT_BEARING_AGENT_RESOURCES = new Set([
  "recent_meetings",
  "open_actions",
  "live_events",
  "live_events_since_seq",
  "live_copilot",
  "meeting",
  "recent-ideas",
]);

export function contentBearingAgentResourceNames(): string[] {
  return [...CONTENT_BEARING_AGENT_RESOURCES].sort();
}

export async function afterContentResourceReadiness<T>(
  name: string,
  operation: () => T | Promise<T>,
  readiness: () => Promise<unknown> = () => requireAgentTrustReadiness()
): Promise<T> {
  if (CONTENT_BEARING_AGENT_RESOURCES.has(name)) {
    await readiness();
  }
  return operation();
}

export async function terminalControlBeforeContentReadiness<T>(
  control: () => T | Promise<T>,
  readiness: () => Promise<unknown> = () => requireAgentTrustReadiness()
): Promise<{ result: T; mayRevealContent: boolean }> {
  const result = await control();
  try {
    await readiness();
    return { result, mayRevealContent: true };
  } catch {
    // The local terminal control has already completed. Readiness failure may
    // withhold derived content, but must never leave capture running.
    return { result, mayRevealContent: false };
  }
}

export function runAgentToolPolicies<T>(
  name: string,
  input: unknown,
  operation: () => T | Promise<T>,
  readiness: () => Promise<unknown> = () => requireAgentTrustReadiness(),
  policy: RestrictedContentPolicy = restrictedContentPolicyFromEnv(),
  auditWriter: RestrictedAuditWriter = appendDurableRestrictedOverrideAudit
): T | Promise<T> {
  // Authorization and its durable audit record must precede readiness. The
  // readiness bridge can retire QMD state, so it is not a read-only preflight
  // and must never run for a denied or not-yet-audited request.
  enforceRestrictedContentPolicy(
    input,
    name,
    policy,
    sensitivityOverrideAuditPath(),
    auditWriter
  );
  if (isContentBearingAgentTool(name)) {
    return afterContentBearingToolReadiness(name, operation, readiness);
  }
  return operation();
}

function withAgentToolPolicies(
  name: string,
  handler: (...args: any[]) => any,
  readiness?: () => Promise<unknown>
): (...args: any[]) => any {
  return (...args: any[]) =>
    readiness
      ? runAgentToolPolicies(name, args[0], () => handler(...args), readiness)
      : runAgentToolPolicies(name, args[0], () => handler(...args));
}

/**
 * The optional `readiness` override exists so a test can drive a
 * content-bearing tool without the trust bridge shelling out to the CLI.
 * Omitting it keeps the live gate, which is what every production registration
 * does.
 */
export function registerToolWithRestrictedPolicy(
  serverArg: McpServer,
  name: string,
  description: string,
  inputSchema: Record<string, unknown>,
  annotations: Record<string, unknown>,
  handler: (...args: any[]) => any,
  readiness?: () => Promise<unknown>
) {
  return serverArg.tool(
    name,
    withToolDocs(name, description),
    inputSchema as any,
    annotations as any,
    withAgentToolPolicies(name, handler, readiness) as any
  );
}

function registerTool(
  name: string,
  description: string,
  inputSchema: Record<string, unknown>,
  annotations: Record<string, unknown>,
  handler: (...args: any[]) => any
) {
  forEachServer((target) => {
    registerToolWithRestrictedPolicy(
      target,
      name,
      description,
      inputSchema,
      annotations,
      handler
    );
  });
}

export function registerDocsAppToolWithRestrictedPolicy(
  serverArg: McpServer,
  name: string,
  config: Record<string, unknown>,
  handler: (...args: any[]) => any
) {
  const description =
    typeof config.description === "string" ? config.description : "";

  return registerAppTool(
    serverArg,
    name,
    {
      ...config,
      description: withToolDocs(name, description),
    } as any,
    withAgentToolPolicies(name, handler) as any
  );
}

function registerDocsAppTool(
  name: string,
  config: Record<string, unknown>,
  handler: (...args: any[]) => any
) {
  forEachServer((target) => {
    registerDocsAppToolWithRestrictedPolicy(target, name, config, handler);
  });
}

const execFileAsync = promisify(execFile);

// ── Sensitivity enforcement (consent layer Wave 2) ──────────

/** `frontmatter.sensitivity` from the sensitivity-capable SDK. */
function meetingSensitivity(meeting: unknown): string | undefined {
  return (meeting as any)?.frontmatter?.sensitivity;
}

/**
 * Parse a live meeting under the stricter agent policy. The SDK version
 * bundled by an already-published MCP package may not yet know a future or
 * mistyped sensitivity value, so the MCP boundary independently rejects any
 * explicit value other than `normal` or `restricted`.
 */
export function parsePolicyVerifiedMeeting(
  content: string,
  filePath: string
): NonNullable<ReturnType<typeof reader.parseFrontmatter>> | null {
  const { yaml } = reader.splitFrontmatter(content);
  if (!yaml) return null;

  try {
    const parsed = parseYaml(yaml);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
    if (typeof parsed.title !== "string" || parsed.title.trim() === "") return null;
    if (
      typeof parsed.type !== "string" ||
      !["meeting", "memo", "dictation"].includes(parsed.type)
    ) {
      return null;
    }
    const parsedDate =
      parsed.date instanceof Date ? parsed.date : new Date(String(parsed.date ?? ""));
    if (Number.isNaN(parsedDate.getTime())) return null;
    if (
      Object.prototype.hasOwnProperty.call(parsed, "sensitivity") &&
      parsed.sensitivity !== "normal" &&
      parsed.sensitivity !== "restricted"
    ) {
      return null;
    }
  } catch {
    return null;
  }

  return reader.parseFrontmatter(content, filePath);
}

type PolicyVerifiedMeeting = NonNullable<
  ReturnType<typeof reader.parseFrontmatter>
>;

type PolicyVerifiedMeetingSnapshot = {
  path: string;
  content: string;
  meeting: PolicyVerifiedMeeting;
};

function comparePolicyMeetingsNewestFirst(
  left: PolicyVerifiedMeetingSnapshot,
  right: PolicyVerifiedMeetingSnapshot
): number {
  const leftDate = Date.parse(left.meeting.frontmatter.date);
  const rightDate = Date.parse(right.meeting.frontmatter.date);
  // parsePolicyVerifiedMeeting already rejects invalid dates. Keep the fallback
  // deterministic if a future SDK parser ever widens that contract.
  const byDate = (Number.isFinite(rightDate) ? rightDate : Number.NEGATIVE_INFINITY) -
    (Number.isFinite(leftDate) ? leftDate : Number.NEGATIVE_INFINITY);
  if (byDate) return byDate;
  if (left.path === right.path) return 0;
  return left.path < right.path ? -1 : 1;
}

function newestPolicySnapshots(
  snapshots: PolicyVerifiedMeetingSnapshot[]
): PolicyVerifiedMeetingSnapshot[] {
  return [...snapshots].sort(comparePolicyMeetingsNewestFirst);
}

const MAX_POLICY_SCAN_FILES = 100_000;
const INACTIVE_CORPUS_DIRS = new Set([
  "archive",
  "processed",
  "failed",
  "failed-captures",
]);

export function isActiveCorpusMeetingPath(filePath: string, root: string): boolean {
  const canonicalRoot = canonicalizeRoot(root);
  const canonicalFile = canonicalizeRoot(filePath);
  const relativePath = relative(canonicalRoot, canonicalFile);
  if (
    relativePath === "" ||
    isAbsolute(relativePath) ||
    relativePath.split(/[\\/]+/).some((component) => component === "..")
  ) {
    return false;
  }
  return !relativePath
    .split(/[\\/]+/)
    .some(
      (component) =>
        component.startsWith(".") || INACTIVE_CORPUS_DIRS.has(component.toLowerCase())
    );
}

export function isPathWithinCanonicalRoot(filePath: string, root: string): boolean {
  const canonicalRoot = canonicalizeRoot(root);
  const canonicalFile = canonicalizeRoot(filePath);
  const relativePath = relative(canonicalRoot, canonicalFile);
  return (
    relativePath === "" ||
    (!isAbsolute(relativePath) &&
      !relativePath.split(/[\\/]+/).some((component) => component === ".."))
  );
}

/**
 * Re-read SDK candidates from their canonical live paths and apply the MCP's
 * strict classifier. This deliberately does not trust the installed SDK's
 * parsed object: an older published artifact can map an unknown sensitivity
 * value to `undefined` while retaining the body.
 */
function policySnapshotIsWorse(
  left: PolicyVerifiedMeetingSnapshot,
  right: PolicyVerifiedMeetingSnapshot
): boolean {
  return comparePolicyMeetingsNewestFirst(left, right) > 0;
}

function pushNewestPolicySnapshot(
  heap: PolicyVerifiedMeetingSnapshot[],
  snapshot: PolicyVerifiedMeetingSnapshot,
  limit: number
): void {
  if (heap.length < limit) {
    heap.push(snapshot);
    let child = heap.length - 1;
    while (child > 0) {
      const parent = Math.floor((child - 1) / 2);
      if (!policySnapshotIsWorse(heap[child], heap[parent])) break;
      [heap[parent], heap[child]] = [heap[child], heap[parent]];
      child = parent;
    }
    return;
  }

  // The root is the oldest/path-last retained meeting. A candidate that is
  // not newer cannot improve the bounded newest-first corpus window.
  if (comparePolicyMeetingsNewestFirst(snapshot, heap[0]) >= 0) return;
  heap[0] = snapshot;
  let parent = 0;
  for (;;) {
    const left = parent * 2 + 1;
    if (left >= heap.length) break;
    const right = left + 1;
    let worse = left;
    if (right < heap.length && policySnapshotIsWorse(heap[right], heap[left])) {
      worse = right;
    }
    if (!policySnapshotIsWorse(heap[worse], heap[parent])) break;
    [heap[parent], heap[worse]] = [heap[worse], heap[parent]];
    parent = worse;
  }
}

export function collectPolicyVerifiedMeetingSnapshots(
  snapshot: StableCorpusSnapshot,
  includeRestricted: boolean,
  matches: (meeting: PolicyVerifiedMeeting) => boolean = () => true
): PolicyVerifiedMeetingSnapshot[] {
  const snapshots: PolicyVerifiedMeetingSnapshot[] = [];

  let scanned = 0;
  for (const file of snapshot.files) {
    if (scanned >= MAX_POLICY_SCAN_FILES) break;
    scanned += 1;
    if (!isActiveCorpusMeetingPath(file.path, snapshot.canonicalRoot)) continue;
    const meeting = parsePolicyVerifiedMeeting(file.content, file.path);
    if (!meeting) continue;
    if (!includeRestricted && meetingSensitivity(meeting) === "restricted") {
      continue;
    }
    if (!matches(meeting)) continue;
    pushNewestPolicySnapshot(
      snapshots,
      { path: file.path, content: file.content, meeting },
      MCP_POLICY_MEETING_RESULT_MAX
    );
  }

  return snapshots;
}

async function policyMatchingSnapshotOperation<T>(
  dir: string,
  includeRestricted: boolean,
  matches: (meeting: PolicyVerifiedMeeting) => boolean,
  operation: (snapshots: PolicyVerifiedMeetingSnapshot[]) => T | Promise<T>,
  hooks: CorpusLeaseHooks = {}
): Promise<T> {
  return withStableCorpusLease(
    dir,
    (snapshot) =>
      operation(
        collectPolicyVerifiedMeetingSnapshots(snapshot, includeRestricted, matches)
      ),
    hooks
  );
}

async function policySnapshotsStillAuthorized(
  dir: string,
  includeRestricted: boolean,
  snapshots: PolicyVerifiedMeetingSnapshot[]
): Promise<boolean> {
  try {
    return await withStableCorpusLease(dir, (corpus) => {
      const live = new Map(corpus.files.map((file) => [file.path, file.content]));
      return snapshots.every((snapshot) => {
        const content = live.get(snapshot.path);
        if (content !== snapshot.content) return false;
        const meeting = parsePolicyVerifiedMeeting(content, snapshot.path);
        return !!meeting &&
          (includeRestricted || meetingSensitivity(meeting) !== "restricted");
      });
    });
  } catch {
    return false;
  }
}

async function policySnapshotOperation<T>(
  dir: string,
  includeRestricted: boolean,
  operation: (snapshots: PolicyVerifiedMeetingSnapshot[]) => T | Promise<T>,
  hooks: CorpusLeaseHooks = {}
): Promise<T> {
  return withStableCorpusLease(
    dir,
    (snapshot) =>
      operation(collectPolicyVerifiedMeetingSnapshots(snapshot, includeRestricted)),
    hooks
  );
}

/**
 * Rust's `std::fs::canonicalize` preserves Windows' extended-length namespace
 * (`\\?\C:\...` / `\\?\UNC\server\share\...`), while Node's
 * `realpathSync` returns the equivalent DOS/UNC spelling. Strip only those two
 * well-formed namespace prefixes. Case, separators, dot components, trailing
 * separators, and every other namespace remain exact.
 */
export function normalizeCanonicalPathWire(path: string): string {
  const extendedUnc = /^\\\\\?\\UNC\\([^\\]+)\\([^\\]+)(.*)$/i.exec(path);
  if (extendedUnc) {
    return `\\\\${extendedUnc[1]}\\${extendedUnc[2]}${extendedUnc[3]}`;
  }
  if (/^\\\\\?\\[A-Za-z]:\\/.test(path)) {
    return path.slice(4);
  }
  return path;
}

export function canonicalPathWireEquals(left: string, right: string): boolean {
  return normalizeCanonicalPathWire(left) === normalizeCanonicalPathWire(right);
}

function contextSourceAuthorizationSessionId(
  value: unknown,
  source: PolicyVerifiedMeetingSnapshot
): string {
  if (!value || typeof value !== "object") {
    throw new Error("Desktop context did not return a source authorization receipt.");
  }
  const receipt = (value as any).source_authorization;
  if (
    !receipt ||
    typeof receipt !== "object" ||
    Object.keys(receipt).sort().join("\0") !==
      ["path", "session_id", "sha256"].sort().join("\0") ||
    typeof receipt.session_id !== "string" ||
    receipt.session_id.trim() === "" ||
    typeof receipt.path !== "string" ||
    typeof receipt.sha256 !== "string"
  ) {
    throw new Error("Desktop context returned an invalid source authorization receipt.");
  }
  const expectedSha256 = createHash("sha256")
    .update(source.content)
    .digest("hex");
  if (
    !canonicalPathWireEquals(receipt.path, source.path) ||
    receipt.sha256 !== expectedSha256
  ) {
    throw new Error("Desktop context source authorization no longer matches the live meeting.");
  }
  return receipt.session_id;
}

export async function withPolicyBoundContextPath<TValue, TResult>(
  requestedPath: string,
  meetingsDir: string,
  operation: (
    canonicalPath: string,
    timeoutMs: number,
    signal: AbortSignal
  ) => Promise<TValue>,
  consume: (
    value: TValue,
    sessionId: string,
    source: PolicyVerifiedMeetingSnapshot,
    signal: AbortSignal
  ) => TResult | Promise<TResult>,
  leaseHooks: CorpusLeaseHooks = {}
): Promise<TResult> {
  const canonicalPath = canonicalizeRoot(requestedPath);
  return withStableCorpusLease(
    meetingsDir,
    async (snapshot, _attempt, signal) => {
      const file = snapshot.files.find((candidate) => candidate.path === canonicalPath);
      if (!file || !isActiveCorpusMeetingPath(file.path, snapshot.canonicalRoot)) {
        throw new Error("Desktop context source is outside the active meeting corpus.");
      }
      const meeting = parsePolicyVerifiedMeeting(file.content, file.path);
      if (!meeting || meetingSensitivity(meeting) === "restricted") {
        throw new Error("Desktop context source is unavailable under the active policy.");
      }
      const source = { path: file.path, content: file.content, meeting };
      const value = await operation(file.path, 10_000, signal);
      const sessionId = contextSourceAuthorizationSessionId(value, source);
      return consume(value, sessionId, source, signal);
    },
    leaseHooks
  );
}

function assertContextSession(value: unknown, sessionId: string): void {
  if (
    !value ||
    typeof value !== "object" ||
    !(value as any).session ||
    (value as any).session.id !== sessionId
  ) {
    throw new Error("Desktop context session does not match its authorized source.");
  }
}

function assertContextItemsSession(
  items: unknown,
  sessionId: string,
  label: string
): void {
  if (
    !Array.isArray(items) ||
    items.some(
      (item) =>
        !item ||
        typeof item !== "object" ||
        (item as any).session_id !== sessionId
    )
  ) {
    throw new Error(`${label} escaped its authorized context session.`);
  }
}

export function assistantSafeContextLinks(items: unknown, sourcePath: string): unknown[] {
  if (!Array.isArray(items)) {
    throw new Error("Desktop context links were unavailable.");
  }
  return items.filter((item) => {
    const kind = (item as any)?.kind;
    const target = (item as any)?.target;
    if (
      kind === "markdown-artifact" &&
      typeof target === "string" &&
      canonicalPathWireEquals(target, sourcePath)
    ) {
      return true;
    }
    return false;
  });
}

function withoutContextSourceAuthorization<T extends Record<string, unknown>>(
  value: T
): Omit<T, "source_authorization"> {
  const { source_authorization: _receipt, ...publicValue } = value;
  return publicValue;
}

async function policyVerifiedMeetingSnapshots(
  dir: string,
  includeRestricted: boolean
): Promise<PolicyVerifiedMeetingSnapshot[]> {
  return policySnapshotOperation(dir, includeRestricted, (snapshots) => snapshots);
}

export async function policyVerifiedExactMeetingSnapshot(
  filePath: string,
  dir: string,
  includeRestricted: boolean
): Promise<PolicyVerifiedMeetingSnapshot | null> {
  try {
    if (!isActiveCorpusMeetingPath(filePath, dir)) return null;
    const { path, content } = await readTextFileInDirectory(filePath, dir, [".md"]);
    if (!isActiveCorpusMeetingPath(path, dir)) return null;
    const meeting = parsePolicyVerifiedMeeting(content, path);
    if (!meeting) return null;
    if (!includeRestricted && meetingSensitivity(meeting) === "restricted") return null;
    return { path, content, meeting };
  } catch {
    return null;
  }
}

export async function policyListMeetings(
  dir: string,
  limit: number,
  includeRestricted: boolean,
  beforeFinalAuthorization: () => void | Promise<void> = () => {},
  leaseHooks: CorpusLeaseHooks = {}
): Promise<PolicyVerifiedMeeting[]> {
  const boundedLimit = normalizeMcpResultLimit(
    limit,
    MCP_POLICY_MEETING_RESULT_MAX,
    "policy meeting"
  );
  return policySnapshotOperation(
    dir,
    includeRestricted,
    (snapshots) =>
      newestPolicySnapshots(snapshots)
        .slice(0, boundedLimit)
        .map((snapshot) => snapshot.meeting),
    {
      ...leaseHooks,
      beforeFinalManifest: async (context) => {
        await beforeFinalAuthorization();
        await leaseHooks.beforeFinalManifest?.(context);
      },
    }
  );
}

export async function policySearchMeetings(
  dir: string,
  query: string,
  limit: number,
  includeRestricted: boolean,
  beforeFinalAuthorization: () => void | Promise<void> = () => {},
  leaseHooks: CorpusLeaseHooks = {}
): Promise<PolicyVerifiedMeeting[]> {
  const boundedLimit = normalizeMcpResultLimit(
    limit,
    MCP_POLICY_MEETING_RESULT_MAX,
    "policy search"
  );
  if (!query) return [];
  const needle = query.toLowerCase();
  return policyMatchingSnapshotOperation(
    dir,
    includeRestricted,
    (meeting) =>
      meeting.frontmatter.title.toLowerCase().includes(needle) ||
      meeting.body.toLowerCase().includes(needle),
    (snapshots) => {
      const results: PolicyVerifiedMeeting[] = [];
      for (const snapshot of newestPolicySnapshots(snapshots)) {
        results.push(snapshot.meeting);
        if (results.length >= boundedLimit) break;
      }
      return results;
    },
    {
      ...leaseHooks,
      beforeFinalManifest: async (context) => {
        await beforeFinalAuthorization();
        await leaseHooks.beforeFinalManifest?.(context);
      },
    }
  );
}

export type PolicyIntentResult = {
  date: string;
  title: string;
  content_type: string;
  path: string;
  kind: string;
  what: string;
  who?: string;
  by_date?: string;
  status: string;
};

function meetingMatchesSince(meeting: PolicyVerifiedMeeting, since?: string): boolean {
  if (!since) return true;
  const boundary = Date.parse(since);
  const meetingDate = Date.parse(meeting.frontmatter.date);
  return Number.isFinite(boundary) && Number.isFinite(meetingDate) && meetingDate >= boundary;
}

function meetingMatchesPerson(meeting: PolicyVerifiedMeeting, person?: string): boolean {
  if (!person) return true;
  const needle = person.trim().toLowerCase();
  if (!needle) return true;
  const people = [
    ...meeting.frontmatter.attendees,
    ...meeting.frontmatter.people,
    ...(meeting.frontmatter.attendees_raw || "").split(/[,;\n]/),
  ];
  return people.some((entry) => entry.trim().toLowerCase().includes(needle));
}

function normalizedPersonSelector(value?: string): string {
  return (value ?? "").trim().replace(/\s+/g, " ").toLowerCase();
}

function personSelectorSlug(value?: string): string {
  return normalizedPersonSelector(value)
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function ownerMatchesExactSelector(owner: string | undefined, selector: string | undefined): boolean {
  const normalizedSelector = normalizedPersonSelector(selector);
  if (!normalizedSelector) return true;
  const normalizedOwner = normalizedPersonSelector(owner);
  return normalizedOwner === normalizedSelector
    || personSelectorSlug(owner) === personSelectorSlug(selector);
}

export function policyIntentResults(
  meetings: PolicyVerifiedMeeting[],
  query: string,
  intentKind?: string,
  owner?: string,
  limit: number = MCP_INTENT_RESULT_MAX,
  statuses?: ReadonlySet<string>,
  includeMeetingTitleInQuery = true
): PolicyIntentResult[] {
  const boundedLimit = normalizeMcpResultLimit(
    limit,
    MCP_INTENT_RESULT_MAX,
    "intent result"
  );
  const needle = query.trim().toLowerCase();
  const ownerNeedle = owner?.trim().toLowerCase();
  const results: PolicyIntentResult[] = [];

  const push = (
    meeting: PolicyVerifiedMeeting,
    kind: string,
    what: string,
    status: string,
    who?: string,
    byDate?: string
  ): boolean => {
    if (intentKind && kind !== intentKind) return false;
    if (statuses && !statuses.has(status)) return false;
    const queryFields = includeMeetingTitleInQuery
      ? [what, who || "", meeting.frontmatter.title]
      : [what, who || ""];
    if (needle && !queryFields.some((value) => value.toLowerCase().includes(needle))) return false;
    if (ownerNeedle && !ownerMatchesExactSelector(who, ownerNeedle)) return false;
    results.push(boundedPolicyIntentResult({
      date: meeting.frontmatter.date,
      title: meeting.frontmatter.title,
      content_type: meeting.frontmatter.type,
      path: meeting.path,
      kind,
      what,
      who,
      by_date: byDate,
      status,
    }));
    return results.length >= boundedLimit;
  };

  for (const meeting of meetings) {
    for (const item of meeting.frontmatter.action_items) {
      if (push(meeting, "action-item", item.task, item.status, item.assignee, item.due)) {
        return results;
      }
    }
    for (const decision of meeting.frontmatter.decisions) {
      if (push(meeting, "decision", decision.text, "decided")) return results;
    }
    for (const intent of meeting.frontmatter.intents) {
      if (push(meeting, intent.kind, intent.what, intent.status, intent.who, intent.by_date)) {
        return results;
      }
    }
  }
  return results;
}

function normalizedCommitmentKeyField(value?: string): string {
  return (value ?? "").trim().replace(/\s+/g, " ").toLowerCase();
}

function commitmentStatus(status: string, byDate: string | undefined, nowMs: number): "open" | "stale" | null {
  if (status !== "open" && status !== "stale") return null;
  if (status === "stale") return "stale";
  if (!byDate) return "open";
  const dateOnly = /^(\d{4})-(\d{2})-(\d{2})$/.exec(byDate);
  if (dateOnly) {
    const dueEndMs = new Date(
      Number(dateOnly[1]),
      Number(dateOnly[2]) - 1,
      Number(dateOnly[3]) + 1,
    ).getTime();
    return Number.isFinite(dueEndMs) && nowMs >= dueEndMs ? "stale" : "open";
  }
  const dueMs = Date.parse(byDate);
  return Number.isFinite(dueMs) && dueMs < nowMs ? "stale" : "open";
}

/** Build one deduplicated, bounded live commitment projection. */
export function policyCommitmentResults(
  meetings: PolicyVerifiedMeeting[],
  owner?: string,
  limit: number = MCP_INTENT_RESULT_MAX,
  nowMs: number = Date.now()
): PolicyIntentResult[] {
  const boundedLimit = normalizeMcpResultLimit(
    limit,
    MCP_INTENT_RESULT_MAX,
    "commitment result"
  );
  if (!Number.isFinite(nowMs)) {
    throw new Error("commitment clock must be finite");
  }
  const ownerNeedle = owner?.trim().toLowerCase();
  const results: PolicyIntentResult[] = [];
  const resultIndexByKey = new Map<string, number>();

  const push = (
    meeting: PolicyVerifiedMeeting,
    kind: "action-item" | "commitment",
    what: string,
    status: string,
    who?: string,
    byDate?: string
  ): void => {
    const projectedStatus = commitmentStatus(status, byDate, nowMs);
    if (!projectedStatus) return;
    if (ownerNeedle && !ownerMatchesExactSelector(who, ownerNeedle)) return;
    const key = [meeting.path, what, who, byDate]
      .map(normalizedCommitmentKeyField)
      .join("\u0000");
    const existingIndex = resultIndexByKey.get(key);
    if (existingIndex !== undefined) {
      if (projectedStatus === "stale") {
        results[existingIndex].status = "stale";
      }
      return;
    }
    if (results.length >= boundedLimit) return;
    resultIndexByKey.set(key, results.length);
    results.push(boundedPolicyIntentResult({
      date: meeting.frontmatter.date,
      title: meeting.frontmatter.title,
      content_type: meeting.frontmatter.type,
      path: meeting.path,
      kind,
      what,
      who,
      by_date: byDate,
      status: projectedStatus,
    }));
  };

  for (const meeting of meetings) {
    for (const item of meeting.frontmatter.action_items) {
      push(meeting, "action-item", item.task, item.status, item.assignee, item.due);
    }
    for (const intent of meeting.frontmatter.intents) {
      if (intent.kind !== "commitment" && intent.kind !== "action-item") continue;
      push(meeting, intent.kind, intent.what, intent.status, intent.who, intent.by_date);
    }
  }
  return results;
}

function boundedPolicyIntentResult(result: PolicyIntentResult): PolicyIntentResult {
  return {
    date: boundedMcpField(result.date) ?? "",
    title: boundedMcpField(result.title) ?? "",
    content_type: boundedMcpField(result.content_type) ?? "",
    path: boundedMcpField(result.path) ?? "",
    kind: boundedMcpField(result.kind) ?? "",
    what: boundedMcpField(result.what) ?? "",
    who: boundedMcpField(result.who),
    by_date: boundedMcpField(result.by_date),
    status: boundedMcpField(result.status) ?? "",
  };
}

export type PolicyToolSearchFilter = {
  query: string;
  contentType?: "meeting" | "memo";
  since?: string;
  intentKind?: string;
  owner?: string;
  intentsOnly?: boolean;
};

function meetingMatchesToolSearch(
  meeting: PolicyVerifiedMeeting,
  filter: PolicyToolSearchFilter
): boolean {
  if (
    (filter.contentType && meeting.frontmatter.type !== filter.contentType) ||
    !meetingMatchesSince(meeting, filter.since)
  ) {
    return false;
  }
  const intentMode = filter.intentsOnly || !!filter.intentKind || !!filter.owner;
  if (intentMode) {
    return policyIntentResults(
      [meeting],
      filter.query,
      filter.intentKind,
      filter.owner,
      1
    ).length > 0;
  }
  const needle = filter.query.trim().toLowerCase();
  return needle.length > 0 &&
    (meeting.frontmatter.title.toLowerCase().includes(needle) ||
      meeting.body.toLowerCase().includes(needle));
}

export function collectPolicyToolSearchSnapshots(
  snapshot: StableCorpusSnapshot,
  includeRestricted: boolean,
  filter: PolicyToolSearchFilter
): PolicyVerifiedMeetingSnapshot[] {
  return newestPolicySnapshots(
    collectPolicyVerifiedMeetingSnapshots(
      snapshot,
      includeRestricted,
      (meeting) => meetingMatchesToolSearch(meeting, filter)
    )
  );
}

async function policyToolSearchMeetings(
  dir: string,
  includeRestricted: boolean,
  filter: PolicyToolSearchFilter
): Promise<PolicyVerifiedMeeting[]> {
  return withStableCorpusLease(dir, (snapshot) =>
    collectPolicyToolSearchSnapshots(snapshot, includeRestricted, filter)
      .map((entry) => entry.meeting)
  );
}

export function openActionsFromMeetings(
  meetings: PolicyVerifiedMeeting[],
  limit: number = MCP_ACTION_RESULT_MAX
) {
  const boundedLimit = normalizeMcpResultLimit(
    limit,
    MCP_ACTION_RESULT_MAX,
    "open action"
  );
  const actions: Array<{
    path: string;
    item: PolicyVerifiedMeeting["frontmatter"]["action_items"][number];
  }> = [];
  for (const meeting of meetings) {
    for (const item of meeting.frontmatter.action_items) {
      if (item.status !== "open") continue;
      actions.push({ path: meeting.path, item });
      if (actions.length >= boundedLimit) return actions;
    }
  }
  return actions;
}

function boundedActionItem(
  item: PolicyVerifiedMeeting["frontmatter"]["action_items"][number]
) {
  return {
    task: boundedMcpField(item.task) ?? "",
    assignee: boundedMcpField(item.assignee) ?? "",
    due: boundedMcpField(item.due),
    status: boundedMcpField(item.status) ?? "open",
  };
}

export type McpPersonProfileLimits = {
  meetingLimit?: number;
  openActionLimit?: number;
  topicLimit?: number;
};

type PolicyPersonIdentity = {
  canonical: string;
  selectors: Set<string>;
};

function parsePolicyRawAttendees(raw?: string): string[] {
  if (!raw) return [];
  return raw.split(",").flatMap((token) => {
    const trimmed = token.trim();
    if (!trimmed || trimmed.toLowerCase() === "none") return [];
    const parenthesized = trimmed.match(/^(.*?)\s*\([^)]*\)$/)?.[1];
    const angled = trimmed.match(/^(.*?)\s*<[^>]*>$/)?.[1];
    const display = (parenthesized || angled || trimmed).trim();
    return display ? [display] : [];
  });
}

function policyPersonIdentities(
  meeting: PolicyVerifiedMeeting,
  participationOnly: boolean = false
): PolicyPersonIdentity[] {
  const frontmatter = meeting.frontmatter as any;
  const entities = Array.isArray(frontmatter?.entities?.people)
    ? frontmatter.entities.people
    : [];
  const entityIdentities: PolicyPersonIdentity[] = entities.flatMap((entity: any) => {
    const label = typeof entity?.label === "string" ? entity.label : "";
    const slug = typeof entity?.slug === "string" ? entity.slug : personSelectorSlug(label);
    if (!slug || !label) return [];
    const aliases = Array.isArray(entity.aliases)
      ? entity.aliases.filter((alias: unknown): alias is string => typeof alias === "string")
      : [];
    return [{
      canonical: slug,
      selectors: new Set([label, slug, ...aliases].flatMap((value) => [
        normalizedPersonSelector(value),
        personSelectorSlug(value),
      ]).filter(Boolean)),
    }];
  });

  const participantNames = [
    ...meeting.frontmatter.attendees,
    ...parsePolicyRawAttendees(meeting.frontmatter.attendees_raw),
  ];
  const mentionedNames = [
    ...meeting.frontmatter.people,
    ...meeting.frontmatter.action_items
      .filter((item) => item.status === "open" || item.status === "stale")
      .map((item) => item.assignee),
    ...meeting.frontmatter.intents
      .filter((intent) =>
        (intent.kind === "action-item" || intent.kind === "commitment") &&
        (intent.status === "open" || intent.status === "stale")
      )
      .flatMap((intent) => intent.who ? [intent.who] : []),
  ];
  const rawNames = participationOnly
    ? participantNames
    : [...participantNames, ...mentionedNames];
  const speakerMap = Array.isArray(frontmatter?.speaker_map) ? frontmatter.speaker_map : [];
  for (const speaker of speakerMap) {
    if (
      speaker?.confidence === "high" &&
      typeof speaker?.name === "string"
    ) rawNames.push(speaker.name);
  }

  const identities = participationOnly ? [] : [...entityIdentities];
  for (const raw of rawNames) {
    const name = raw.trim();
    if (!name) continue;
    const normalized = normalizedPersonSelector(name);
    const slug = personSelectorSlug(name);
    const entity = entityIdentities.find(
      (candidate) => candidate.selectors.has(normalized) || candidate.selectors.has(slug)
    );
    if (entity) {
      if (
        participationOnly &&
        !identities.some((candidate) => candidate.canonical === entity.canonical)
      ) identities.push(entity);
      continue;
    }
    identities.push({
      canonical: slug,
      selectors: new Set([normalized, slug].filter(Boolean)),
    });
  }
  const merged = new Map<string, PolicyPersonIdentity>();
  for (const identity of identities) {
    const existing = merged.get(identity.canonical);
    if (existing) {
      for (const selector of identity.selectors) existing.selectors.add(selector);
    } else {
      merged.set(identity.canonical, identity);
    }
  }
  return [...merged.values()];
}

export function personProfileFromMeetings(
  meetings: PolicyVerifiedMeeting[],
  name: string,
  limits: McpPersonProfileLimits = {}
): {
  name: string;
  meetings: Array<{ title: string; date: string; path: string }>;
  openActions: PolicyIntentResult[];
  topics: string[];
  topicCounts: Array<{ topic: string; count: number }>;
  recentDecisions: Array<{
    path: string;
    title: string;
    date: string;
    what: string;
    authority?: string;
  }>;
} {
  const meetingLimit = normalizeMcpResultLimit(
    limits.meetingLimit ?? MCP_PERSON_PROFILE_MEETING_MAX,
    MCP_PERSON_PROFILE_MEETING_MAX,
    "person profile meeting"
  );
  const openActionLimit = normalizeMcpResultLimit(
    limits.openActionLimit ?? MCP_PERSON_PROFILE_OPEN_ACTION_MAX,
    MCP_PERSON_PROFILE_OPEN_ACTION_MAX,
    "person profile open-action"
  );
  const topicLimit = normalizeMcpResultLimit(
    limits.topicLimit ?? MCP_PERSON_PROFILE_TOPIC_MAX,
    MCP_PERSON_PROFILE_TOPIC_MAX,
    "person profile topic"
  );
  const normalizedSelector = normalizedPersonSelector(name);
  const slugSelector = personSelectorSlug(name);
  if (!normalizedSelector) throw new Error("person selector is empty");
  const profileMeetings: Array<{ title: string; date: string; path: string }> = [];
  const topicCounts = new Map<string, number>();
  const openActions: PolicyIntentResult[] = [];
  const recentDecisions: Array<{
    path: string;
    title: string;
    date: string;
    what: string;
    authority?: string;
  }> = [];
  const seenCommitments = new Set<string>();
  const matchedCanonicalPeople = new Set<string>();
  for (const meeting of meetings) {
    const matchingIdentities = policyPersonIdentities(meeting).filter(
      (identity) => identity.selectors.has(normalizedSelector) || identity.selectors.has(slugSelector)
    );
    const participatingIdentities = policyPersonIdentities(meeting, true).filter(
      (identity) => identity.selectors.has(normalizedSelector) || identity.selectors.has(slugSelector)
    );
    for (const identity of matchingIdentities) matchedCanonicalPeople.add(identity.canonical);
    if (matchedCanonicalPeople.size > 1) {
      throw new Error("person selector is ambiguous across policy-authorized meetings");
    }
    if (matchingIdentities.length === 0) continue;

    if (participatingIdentities.length > 0 && profileMeetings.length < meetingLimit) {
      profileMeetings.push({
        title: boundedMcpField(meeting.frontmatter.title) ?? "",
        date: boundedMcpField(meeting.frontmatter.date) ?? "",
        path: boundedMcpField(meeting.path) ?? "",
      });
    }
    if (participatingIdentities.length > 0) {
      for (const rawTag of meeting.frontmatter.tags) {
        const tag = boundedMcpField(rawTag) ?? "";
        if (!tag) continue;
        const existing = topicCounts.get(tag);
        if (existing !== undefined) topicCounts.set(tag, existing + 1);
        else if (topicCounts.size < topicLimit) topicCounts.set(tag, 1);
      }
      for (const decision of meeting.frontmatter.decisions) {
        if (recentDecisions.length >= MCP_PERSON_PROFILE_DECISION_MAX) break;
        const what = boundedMcpField(decision.text) ?? "";
        if (!what) continue;
        recentDecisions.push({
          path: boundedMcpField(meeting.path) ?? "",
          title: boundedMcpField(meeting.frontmatter.title) ?? "",
          date: boundedMcpField(meeting.frontmatter.date) ?? "",
          what,
          authority: boundedMcpField((decision as any).authority),
        });
      }
    }
    if (openActions.length < openActionLimit) {
      for (const commitment of policyCommitmentResults(
        [meeting],
        undefined,
        MCP_INTENT_RESULT_MAX
      )) {
        const ownerMatches = matchingIdentities.some((identity) =>
          identity.selectors.has(normalizedPersonSelector(commitment.who)) ||
          identity.selectors.has(personSelectorSlug(commitment.who))
        );
        if (!ownerMatches) continue;
        const key = [commitment.path, commitment.what, commitment.who, commitment.by_date]
          .map((value) => normalizedCommitmentKeyField(value))
          .join("\0");
        if (seenCommitments.has(key)) continue;
        seenCommitments.add(key);
        openActions.push(commitment);
        if (openActions.length >= openActionLimit) break;
      }
    }
  }
  const rankedTopics = [...topicCounts.entries()]
    .sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]))
    .map(([topic, count]) => ({ topic, count }));
  return {
    name: boundedMcpField(name) ?? "",
    meetings: profileMeetings,
    openActions,
    topics: rankedTopics.map((entry) => entry.topic),
    topicCounts: rankedTopics,
    recentDecisions,
  };
}

export function boundedCorePersonProfile(raw: unknown): {
  name: string;
  meetings: Array<{ title: string; date: string; path: string }>;
  openIntents: PolicyIntentResult[];
  recentDecisions: Array<{
    title: string;
    date: string;
    path: string;
    what: string;
    authority: string | null;
  }>;
  topicCounts: Array<{ topic: string; count: number }>;
} {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error("Minutes returned an invalid person profile");
  }
  const profile = raw as Record<string, unknown>;
  const meetingsRaw = Array.isArray(profile.recent_meetings) ? profile.recent_meetings : null;
  const intentsRaw = Array.isArray(profile.open_intents) ? profile.open_intents : null;
  const decisionsRaw = Array.isArray(profile.recent_decisions) ? profile.recent_decisions : null;
  const topicsRaw = Array.isArray(profile.top_topics) ? profile.top_topics : null;
  if (
    !meetingsRaw || meetingsRaw.length > MCP_PERSON_PROFILE_MEETING_MAX ||
    !intentsRaw || intentsRaw.length > MCP_PERSON_PROFILE_OPEN_ACTION_MAX ||
    !decisionsRaw || decisionsRaw.length > MCP_PERSON_PROFILE_DECISION_MAX ||
    !topicsRaw || topicsRaw.length > MCP_PERSON_PROFILE_TOPIC_MAX
  ) {
    throw new Error("Minutes returned an invalid bounded person profile");
  }
  const field = (value: unknown, label: string): string => {
    if (typeof value !== "string") throw new Error(`Minutes returned an invalid profile ${label}`);
    return boundedMcpField(value) ?? "";
  };
  const meetings = meetingsRaw.map((value) => {
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      throw new Error("Minutes returned an invalid profile meeting");
    }
    const row = value as Record<string, unknown>;
    return {
      title: field(row.title, "meeting title"),
      date: field(row.date, "meeting date"),
      path: field(row.path, "meeting path"),
    };
  });
  const openIntents = intentsRaw.map((value) => {
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      throw new Error("Minutes returned an invalid profile commitment");
    }
    const row = value as Record<string, unknown>;
    return boundedPolicyIntentResult({
      date: field(row.date, "commitment date"),
      title: field(row.title, "commitment title"),
      content_type: field(row.content_type, "commitment content type"),
      path: field(row.path, "commitment path"),
      kind: field(row.kind, "commitment kind"),
      what: field(row.what, "commitment text"),
      who: typeof row.who === "string" ? row.who : undefined,
      by_date: typeof row.by_date === "string" ? row.by_date : undefined,
      status: field(row.status, "commitment status"),
    });
  });
  const topicCounts = topicsRaw.map((value) => {
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      throw new Error("Minutes returned an invalid profile topic");
    }
    const row = value as Record<string, unknown>;
    if (!Number.isSafeInteger(row.count) || (row.count as number) < 0) {
      throw new Error("Minutes returned an invalid profile topic count");
    }
    return { topic: field(row.topic, "topic"), count: row.count as number };
  });
  const recentDecisions = decisionsRaw.map((value) => {
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      throw new Error("Minutes returned an invalid profile decision");
    }
    const row = value as Record<string, unknown>;
    return {
      title: field(row.title, "decision title"),
      date: field(row.date, "decision date"),
      path: field(row.path, "decision path"),
      what: field(row.what, "decision text"),
      authority: row.authority === null || row.authority === undefined
        ? null
        : field(row.authority, "decision authority"),
    };
  });
  return {
    name: field(profile.name, "name"),
    meetings,
    openIntents,
    recentDecisions,
    topicCounts,
  };
}

export type McpResearchTopicProjection = {
  decisions: string[];
  openIntents: PolicyIntentResult[];
  topics: Array<{ topic: string; count: number }>;
  meetings: Array<{ date: string; title: string }>;
  text: string;
};

/** Build a field- and collection-bounded research response from matched meetings. */
export function researchTopicProjection(
  sourceMeetings: PolicyVerifiedMeeting[],
  query: string
): McpResearchTopicProjection {
  const meetings = sourceMeetings.slice(0, MCP_RESEARCH_MEETING_RESULT_MAX);
  const needle = query.trim().toLowerCase();
  const decisions: string[] = [];
  for (const meeting of meetings) {
    for (const decision of meeting.frontmatter.decisions) {
      if (
        !needle ||
        ![decision.text, decision.topic || ""].some((value) =>
          value.toLowerCase().includes(needle)
        )
      ) {
        continue;
      }
      const line = `- ${boundedMcpField(meeting.frontmatter.date) ?? ""} — ${boundedMcpField(decision.text) ?? ""} (${boundedMcpField(meeting.frontmatter.title) ?? ""})`;
      decisions.push(boundedMcpField(line) ?? "");
      if (decisions.length >= MCP_RESEARCH_DECISION_RESULT_MAX) break;
    }
    if (decisions.length >= MCP_RESEARCH_DECISION_RESULT_MAX) break;
  }

  const openIntents = policyIntentResults(
    meetings,
    query,
    undefined,
    undefined,
    MCP_INTENT_RESULT_MAX,
    new Set(["open"]),
    false
  );
  const topicCounts = new Map<string, number>();
  for (const meeting of meetings) {
    for (const rawTag of meeting.frontmatter.tags) {
      if (!needle || !rawTag.toLowerCase().includes(needle)) continue;
      const topic = boundedMcpField(rawTag) ?? "";
      if (!topic) continue;
      const current = topicCounts.get(topic);
      if (current !== undefined) {
        topicCounts.set(topic, current + 1);
      } else if (topicCounts.size < MCP_RESEARCH_TOPIC_RESULT_MAX) {
        topicCounts.set(topic, 1);
      }
    }
  }
  const topics = Array.from(topicCounts, ([topic, count]) => ({ topic, count }));
  const meetingResults = meetings.map((meeting) => ({
    date: boundedMcpField(meeting.frontmatter.date) ?? "",
    title: boundedMcpField(meeting.frontmatter.title) ?? "",
  }));
  const sections: string[] = [];
  if (topics.length > 0) {
    sections.push(
      "Related topics:\n" +
        topics.map(({ topic, count }) => `- ${topic} (${count})`).join("\n")
    );
  }
  if (decisions.length > 0) {
    sections.push(`Recent decisions:\n${decisions.join("\n")}`);
  }
  if (openIntents.length > 0) {
    sections.push(
      "Open follow-ups:\n" +
        openIntents
          .map(
            (intent) =>
              `- ${intent.kind}: ${intent.what}${intent.who ? ` (@${intent.who})` : ""}${intent.by_date ? ` by ${intent.by_date}` : ""}`
          )
          .join("\n")
    );
  }
  if (meetingResults.length > 0) {
    sections.push(
      "Matching meetings:\n" +
        meetingResults
          .map((meeting) => `- ${meeting.date} — ${meeting.title}`)
          .join("\n")
    );
  }
  const boundedQuery = boundedMcpField(query) ?? "";
  const text = sections.length > 0
    ? `Cross-meeting research for ${boundedQuery}:\n\n${sections.join("\n\n")}`
    : `No cross-meeting results found for ${boundedQuery}.`;
  return {
    decisions,
    openIntents,
    topics,
    meetings: meetingResults,
    text: boundedMcpText(text),
  };
}

// ── Live-snapshot enrichment for policy-filtered search indexes ─────

export async function enrichWithFrontmatter(
  qmdResults: any[],
  includeRestricted: boolean,
  meetingsDir?: string,
  query?: string,
  limit: number = MCP_MEETING_RESULT_MAX
): Promise<any[]> {
  const boundedLimit = normalizeMcpMeetingResultLimit(limit);
  const verifiedMeetingsDir = meetingsDir ?? (await getEffectiveMeetingsDir());
  return withStableCorpusLease(verifiedMeetingsDir, (snapshot) => {
    // Absolute canonical roots can cross a process boundary with an equivalent
    // Windows namespace/short-path spelling. Authorization remains anchored to
    // the canonical root and candidate below; use the snapshot's exact
    // corpus-relative key only after those containment checks have succeeded.
    const canonicalMeetingsDir = canonicalizeRoot(verifiedMeetingsDir);
    const liveFiles = new Map(
      snapshot.files.map((file) => [file.relativePath, file.content])
    );
    const enriched: any[] = [];
    for (const r of qmdResults) {
      try {
        const candidatePath = r.source_path || r.path;
        if (!isActiveCorpusMeetingPath(candidatePath, verifiedMeetingsDir)) continue;
        const filePath = canonicalizeRoot(candidatePath);
        if (!isActiveCorpusMeetingPath(filePath, verifiedMeetingsDir)) continue;
        const snapshotKey = relative(canonicalMeetingsDir, filePath).replaceAll(
          "\\",
          "/"
        );
        const content = liveFiles.get(snapshotKey);
        if (content === undefined) continue;
        const meeting = parsePolicyVerifiedMeeting(content, filePath);
        // Verification failure is never overridable: an operator can grant
        // access to a known restricted file, not to an unreadable or
        // policy-uncertain index record.
        if (!meeting) continue;
        if (!includeRestricted && meetingSensitivity(meeting) === "restricted") {
          continue;
        }
        enriched.push({
          date: boundedMcpField(meeting.frontmatter.date) ?? "",
          title: boundedMcpField(meeting.frontmatter.title) ?? "",
          content_type: boundedMcpField(meeting.frontmatter.type) ?? "meeting",
          path: boundedMcpField(filePath) ?? "",
          // QMD is only a ranking/path hint. Its cached snippet may predate a
          // sensitivity change, so derive display text from the verified live
          // snapshot instead of returning index content.
          snippet: liveMeetingSnippet(meeting.body, query),
        });
        if (enriched.length >= boundedLimit) break;
      } catch {
        // QMD is an index, not an authorization source. A hit whose live file
        // cannot be canonicalized, read, or parsed must disappear completely;
        // returning its stale path/snippet would leak the very content the
        // frontmatter verification is meant to protect.
        continue;
      }
    }
    return enriched;
  });
}

export function liveMeetingSnippet(body: string, query?: string): string {
  const normalized = body.replace(/\s+/g, " ").trim();
  if (!normalized) return "";
  const maxChars = 320;
  const needle = query?.trim().toLowerCase();
  if (!needle) return normalized.slice(0, maxChars);
  const match = normalized.toLowerCase().indexOf(needle);
  if (match < 0) return normalized.slice(0, maxChars);
  const start = Math.max(0, match - 120);
  return normalized.slice(start, start + maxChars);
}

// ESM-compatible __dirname
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

function canonicalEntrypointPath(filePath: string | null | undefined): string | null {
  if (!filePath) return null;

  const resolved = resolve(filePath);

  try {
    return realpathSync(resolved);
  } catch {
    return resolved;
  }
}

export function shouldRunMainEntry(argv1: string | null | undefined, moduleFilename: string): boolean {
  const entryPath = canonicalEntrypointPath(argv1);
  const modulePath = canonicalEntrypointPath(moduleFilename);

  return !!entryPath && !!modulePath && entryPath === modulePath;
}

// ── Extension runtime detection ───────────────────────────────
// When running as a Claude Desktop extension (.mcpb), Claude uses its built-in
// Node.js runtime.  Child processes spawned from that runtime land in a
// different macOS audit session and do NOT inherit the host app's TCC
// microphone grant — CoreAudio delivers all-zero samples (silence).
//
// Manual MCP configs (`claude_desktop_config.json`) spawn the user's own
// `node` binary, which typically has an independent TCC mic entry, so child
// processes work fine.
//
// Detection: the .mcpb unpacks into "Claude Extensions" inside Application
// Support, and Claude Desktop sets MCP_EXTENSION_ID for extension servers.
// This is macOS-specific — Windows/Linux don't have TCC, so their extension
// runtimes can spawn child processes with mic access normally.
const isExtensionRuntime: boolean =
  process.platform === "darwin" &&
  (!!process.env.MCP_EXTENSION_ID ||
   __dirname.includes("Claude Extensions") ||
   __dirname.includes("claude-extensions"));

if (isExtensionRuntime) {
  console.error(
    "[Minutes] Running as Claude Desktop extension — audio capture will " +
    "delegate to the Minutes desktop app (TCC mic grants don't propagate " +
    "through the extension runtime). Launch Minutes.app for recording."
  );
} else {
  console.error(
    `[Minutes] Extension runtime detection: false ` +
    `(MCP_EXTENSION_ID=${!!process.env.MCP_EXTENSION_ID}, dirname=${__dirname})`
  );
}

// ── Find the minutes binary ─────────────────────────────────

function findMinutesBinary(): string {
  const platform = process.platform;
  const isWindows = platform === "win32";
  const ext = isWindows ? ".exe" : "";
  const candidates = [
    join(__dirname, "..", "..", "..", "target", "release", `minutes${ext}`),
    join(__dirname, "..", "..", "..", "target", "debug", `minutes${ext}`),
    // Where the Windows auto-installer puts it: a Minutes-owned directory, so
    // the MSVC runtime shipped beside the binary (#657) does not leak into a
    // shared bin directory. Listed before ~/.cargo/bin so a fresh install wins
    // over an older cargo-installed copy.
    ...(isWindows ? [join(homedir(), ".minutes", "bin", `minutes${ext}`)] : []),
    join(homedir(), ".cargo", "bin", `minutes${ext}`),
    ...(isWindows
      ? []
      : [
          join(homedir(), ".local", "bin", "minutes"),
          "/opt/homebrew/bin/minutes",
          "/usr/local/bin/minutes",
        ]),
    // Inside the desktop app, which ships the CLI as a sidecar and updates it
    // with the app. Without this, someone who installed the app and never
    // opened it has the engine on disk while the extension reports it missing,
    // and the remedy we print is something they have already done. The app's
    // own "Set up CLI" symlinks ~/.local/bin above, so this is the path for
    // people who never clicked it.
    ...(platform === "darwin"
      ? [
          "/Applications/Minutes.app/Contents/MacOS/minutes",
          join(homedir(), "Applications", "Minutes.app", "Contents", "MacOS", "minutes"),
          join(homedir(), "Applications", "Minutes Dev.app", "Contents", "MacOS", "minutes"),
        ]
      : []),
  ];

  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      return candidate;
    }
  }

  // Fall back to PATH lookup
  return "minutes";
}

let MINUTES_BIN = findMinutesBinary();

// ── Capability probe (Phase 2 of #183) ────────────────────────
// Ask the CLI what it supports instead of inferring from version strings.
// Synchronous so it can run before tool registrations at module load.
// Distinguish a truly missing CLI (first-run auto-install can still recover
// later in the session) from an already-installed CLI that does not support
// the capabilities contract and should stay fail-closed.
const CLI_CAPABILITIES: CapabilityProbeResult =
  probeCapabilitiesSync(MINUTES_BIN);
if (CLI_CAPABILITIES.kind === "report") {
  crashTrace("cli-capabilities-probed", {
    cliVersion: CLI_CAPABILITIES.report.version,
    apiVersion: CLI_CAPABILITIES.report.api_version,
    featureCount: Object.keys(CLI_CAPABILITIES.report.features).length,
  });
} else if (CLI_CAPABILITIES.kind === "missing-cli") {
  crashTrace("cli-capabilities-cli-missing");
} else {
  crashTrace("cli-capabilities-unsupported");
}
const LIVE_EVENTS_SUPPORTED = hasFeature(CLI_CAPABILITIES, "events_since_seq");
const COPILOT_SUPPORTED = hasFeature(CLI_CAPABILITIES, "copilot_realtime");

// ── MCP server version ────────────────────────────────────────
// Kept for capabilities handshake and user-facing log messages.
// The compatibility decision against the installed CLI lives in
// `./version.ts` (see issue #183). Hosted `.mcpb` bundles will run
// against CLIs with different minor/patch numbers within the same
// major; that is explicitly supported.
const MCP_SERVER_VERSION = "0.25.2";

export function parseKnowledgeConfig(configContent: string): KnowledgeConfigStatus | null {
  const knowledgeMatch = configContent.match(/\[knowledge\][\s\S]*?(?=\n\[|$)/);
  if (!knowledgeMatch) {
    return null;
  }

  const section = knowledgeMatch[0];
  const enabled = /^\s*enabled\s*=\s*true(?:\s*#.*)?$/m.test(section);
  const pathMatch = section.match(/^\s*path\s*=\s*"([^"]+)"/m);
  const adapterMatch = section.match(/^\s*adapter\s*=\s*"([^"]+)"/m);
  const engineMatch = section.match(/^\s*engine\s*=\s*"([^"]+)"/m);

  return {
    enabled,
    path: pathMatch?.[1],
    adapter: adapterMatch?.[1] || "wiki",
    engine: engineMatch?.[1] || "none",
  };
}
// ── CLI auto-install ────────────────────────────────────────
// Auto-install fetches from the GitHub `releases/latest/download/` redirect,
// not a pinned tag, so hosted `.mcpb` bundles self-heal across our release
// cadence. See issue #183 for context.
// When installed via MCPB or `npx minutes-mcp`, the Rust CLI binary
// may not be present. We attempt to install it automatically so
// non-technical users don't hit a "binary not found" dead end.

let installAttempted = false;
const MAX_CAPABILITY_REPAIR_ATTEMPTS = 2;

export function createCapabilityRepairCoordinator(
  repair: () => Promise<boolean>,
  maximumAttempts: number = MAX_CAPABILITY_REPAIR_ATTEMPTS
): () => Promise<boolean> {
  let attempts = 0;
  let inFlight: Promise<boolean> | null = null;
  return async () => {
    if (inFlight) return inFlight;
    if (attempts >= maximumAttempts) return false;
    attempts += 1;
    const attempt = repair();
    inFlight = attempt;
    try {
      return await attempt;
    } finally {
      inFlight = null;
    }
  };
}

const runCapabilityRepair = createCapabilityRepairCoordinator(
  () => tryAutoInstallAttempt(true)
);

function getReleaseBinaryName(): string | null {
  const platform = process.platform;
  const arch = process.arch;
  if (platform === "darwin" && arch === "arm64") return "minutes-macos-arm64";
  if (platform === "darwin" && arch === "x64") return "minutes-macos-arm64"; // Rosetta handles it
  if (platform === "linux" && arch === "x64") return "minutes-linux-x64";
  if (platform === "win32" && arch === "x64") return "minutes-windows-x64.exe";
  return null;
}

function getInstallDir(): string {
  const localBin = join(homedir(), ".local", "bin");
  if (process.platform === "win32") {
    // A Minutes-owned directory, not ~/.cargo/bin. The Windows install ships
    // the MSVC runtime beside the executable (#657), and Windows resolves an
    // application's own directory ahead of the system path. Dropping those
    // DLLs into the shared cargo bin directory would make every other
    // cargo-installed tool launched from there load Minutes' pinned copies,
    // and uninstalling would leave them behind.
    return join(homedir(), ".minutes", "bin");
  }
  return localBin;
}

async function tryAutoInstallAttempt(capabilityRepair: boolean = false): Promise<boolean> {
  if (!capabilityRepair) {
    if (installAttempted) return false;
    installAttempted = true;
  }

  console.error(
    capabilityRepair
      ? "[Minutes] CLI is missing required trust capabilities — attempting a verified update..."
      : "[Minutes] CLI not found — attempting automatic install..."
  );

  // Strategy 1: Download pre-built binary from GitHub release (fastest, no deps)
  const binaryName = getReleaseBinaryName();
  if (binaryName) {
    try {
      const installDir = getInstallDir();
      const isWindows = process.platform === "win32";
      const targetName = isWindows ? "minutes.exe" : "minutes";
      const targetPath = join(installDir, targetName);

      // Ensure install directory exists
      await mkdir(installDir, { recursive: true });

      if (isWindows) {
        // The Windows binary imports the MSVC runtime (VCRUNTIME140.dll,
        // MSVCP140.dll and friends), which is not part of Windows. On a PC
        // that has never had the Visual C++ Redistributable, the bare .exe
        // exits 0xC0000135 (STATUS_DLL_NOT_FOUND) printing nothing at all, so
        // the user sees the command silently do nothing (#657).
        //
        // The zip carries those four files beside minutes.exe. Windows
        // resolves the application's own directory ahead of the system path,
        // so extracting them next to the executable is sufficient and needs no
        // installer and no admin rights. Verified on a clean Windows 11 VM.
        const archiveName = "minutes-windows-x64.zip";
        const archivePath = join(installDir, archiveName);
        console.error(`[Minutes] Downloading ${archiveName} from latest release...`);
        await downloadReleaseBinaryWithChecksum({
          binaryName: archiveName,
          targetPath: archivePath,
          execFileAsync,
        });
        await extractZipWithPowerShell({
          archivePath,
          destDir: installDir,
          execFileAsync,
        });
        await rm(archivePath, { force: true });
        // Confirm the extraction actually produced the binary. The POSIX path
        // gets this for free from rename(); without it a changed archive
        // layout would report success while MINUTES_BIN points at nothing.
        await stat(targetPath);
      } else {
        console.error(`[Minutes] Downloading ${binaryName} from latest release...`);
        // Download with curl, verify SHA256SUMS.txt, then move the verified
        // binary into place.
        await downloadReleaseBinaryWithChecksum({
          binaryName,
          targetPath,
          execFileAsync,
        });
        await execFileAsync("chmod", ["+x", targetPath], { timeout: 5000 });
      }

      console.error(`[Minutes] ✓ Installed to ${targetPath}`);
      // Everything this server runs uses the absolute path, and child
      // processes get the directory prepended via mcpCliChildEnv. A terminal
      // is a different matter: nothing adds this directory to the user's own
      // PATH, so say so rather than leaving `minutes: command not found` as
      // the first thing they see.
      if (process.platform === "win32") {
        console.error(
          `[Minutes] To run 'minutes' in your own terminal, add ${installDir} to PATH:\n` +
            `[Minutes]   setx PATH "%PATH%;${installDir}"   (new terminals only)`,
        );
      }
      MINUTES_BIN = targetPath;
      return true;
    } catch (e: any) {
      console.error(`[Minutes] Binary download failed: ${e.message || e}`);
    }
  }

  // Strategy 2: Homebrew (macOS only)
  if (process.platform === "darwin") {
    try {
      console.error("[Minutes] Trying: brew tap silverstein/tap && brew install minutes");
      await execFileAsync("brew", ["tap", "silverstein/tap"], { timeout: 120000 });
      await execFileAsync("brew", ["install", "minutes"], { timeout: 300000 });
      console.error("[Minutes] ✓ Installed via Homebrew");
      MINUTES_BIN = findMinutesBinary();
      return true;
    } catch (e: any) {
      console.error(`[Minutes] Homebrew install failed: ${e.message || e}`);
    }
  }

  // Strategy 3: Cargo (if Rust is installed)
  try {
    console.error("[Minutes] Trying: cargo install minutes-cli");
    await execFileAsync("cargo", ["install", "minutes-cli"], { timeout: 600000 });
    console.error("[Minutes] ✓ Installed via cargo");
    MINUTES_BIN = findMinutesBinary();
    return true;
  } catch (e: any) {
    console.error(`[Minutes] cargo install failed: ${e.message || e}`);
  }

  console.error(
    "[Minutes] Auto-install failed. Install manually:\n" +
    "  macOS:   brew tap silverstein/tap && brew install minutes\n" +
    "  Any:     cargo install minutes-cli\n" +
    "  Source:  https://github.com/silverstein/minutes"
  );
  return false;
}

// ── CLI version check ───────────────────────────────────────

async function checkCliVersion(): Promise<void> {
  try {
    const { stdout } = await execFileAsync(MINUTES_BIN, ["--version"], { timeout: 5000, env: mcpCliChildEnv() });
    // Output is like "minutes 0.8.0" or just "0.8.0".
    const match = stdout.trim().match(/(\d+\.\d+\.\d+)/);
    if (!match) return;

    const installedVersion = match[1];
    const result = isCliCompatible(installedVersion, MCP_SERVER_VERSION);

    // Only surface logs the user should see. Same-major skew is silent-
    // compatible, which is the whole point of issue #183 fix: hosted `.mcpb`
    // bundles frequently run against a CLI with a different minor/patch
    // and that is fine.
    if (result.severity === "error") {
      console.error(`[Minutes] ${result.message}`);
    } else if (result.severity === "ok") {
      console.error(`[Minutes] ${result.message}`);
    }
    // "info" severity (compatible skew, unparseable version) stays silent.
  } catch {
    // Version check is best-effort. Don't block on failure.
  }
}

// ── Auto-setup: download whisper model if missing ───────────
// Recording needs a whisper model (~75MB for tiny). If the CLI is
// available but the model isn't downloaded, trigger setup automatically
// in the background so the first "start recording" just works.

let modelCheckDone = false;

async function ensureWhisperModel(): Promise<void> {
  if (modelCheckDone) return;
  modelCheckDone = true;

  try {
    // health --json returns an array of { label, state, detail, optional } items.
    // The "Speech model" item has state "ready" when downloaded.
    const { stdout } = await execFileAsync(MINUTES_BIN, ["health", "--json"], { timeout: 10000, env: mcpCliChildEnv() });
    const items = JSON.parse(stdout);
    const modelItem = Array.isArray(items) && items.find((i: any) => i.label === "Speech model");
    if (modelItem && modelItem.state === "ready") {
      console.error("[Minutes] Whisper model ready");
      return;
    }
  } catch {
    // health command may not exist in older CLI versions — fall through to setup
  }

  // Model not found — download tiny model in background
  console.error("[Minutes] Whisper model not found — downloading tiny model (~75MB)...");
  try {
    await execFileAsync(MINUTES_BIN, ["setup", "--model", "tiny"], { timeout: 300000, env: mcpCliChildEnv() });
    console.error("[Minutes] ✓ Whisper tiny model downloaded — recording is ready");
  } catch (e: any) {
    console.error(
      `[Minutes] Model download failed: ${e.message || e}. ` +
      `Run manually: minutes setup --model tiny`
    );
  }
}

// ── CLI availability detection ──────────────────────────────
// When installed via `npx minutes-mcp`, the Rust CLI may not be present yet.
// The CLI is the trust-boundary bridge, so startup must install/probe it before
// accepting any MCP request rather than advertising a CLI-less reader mode.

let cliAvailable: boolean | null = null;
let cliCheckedAt = 0;
const CLI_CACHE_TTL_MS = 5 * 60 * 1000; // re-check every 5 minutes

async function isCliAvailable(): Promise<boolean> {
  // Cache hit: return true permanently (CLI won't disappear mid-session)
  // Cache miss (false): re-probe after TTL so installing CLI mid-session works
  if (cliAvailable === true) return true;
  if (cliAvailable === false && Date.now() - cliCheckedAt < CLI_CACHE_TTL_MS) return false;

  try {
    await execFileAsync(MINUTES_BIN, ["--version"], { timeout: 5000, env: mcpCliChildEnv() });
    cliAvailable = true;
    cliCheckedAt = Date.now();
    console.error("[Minutes] CLI found — full mode (all tools enabled)");
    // Check version and ensure whisper model in background (non-blocking)
    checkCliVersion();
    ensureWhisperModel();
  } catch {
    // CLI not found — try to install it automatically
    if (!installAttempted) {
      const installed = await tryAutoInstall();
      if (installed) {
        try {
          await execFileAsync(MINUTES_BIN, ["--version"], { timeout: 5000, env: mcpCliChildEnv() });
          cliAvailable = true;
          cliCheckedAt = Date.now();
          console.error("[Minutes] CLI now available after auto-install — full mode");
          checkCliVersion();
          ensureWhisperModel();
          return true;
        } catch {
          // Install succeeded but binary still not found — path issue
        }
      }
    }
    cliAvailable = false;
    cliCheckedAt = Date.now();
    console.error(
      "[Minutes] CLI not available — the agent trust boundary cannot be established"
    );
  }
  return cliAvailable;
}

async function tryAutoInstall(capabilityRepair: boolean = false): Promise<boolean> {
  if (!capabilityRepair) return tryAutoInstallAttempt(false);
  return runCapabilityRepair();
}

export async function repairCliCapabilities(
  required: readonly string[],
  capabilities: CapabilityProbeResult,
  repair: () => Promise<boolean>,
  reprobe: () => CapabilityProbeResult
): Promise<CapabilityProbeResult> {
  if (
    capabilities.kind === "report" &&
    required.every((feature) => hasFeature(capabilities, feature))
  ) {
    return capabilities;
  }
  if (await repair()) {
    return reprobe();
  }
  return capabilities;
}

async function ensureCliCapabilities(
  required: readonly string[]
): Promise<CapabilityProbeResult> {
  if (!(await isCliAvailable())) {
    return { kind: "missing-cli" };
  }
  return repairCliCapabilities(
    required,
    probeCapabilitiesSync(MINUTES_BIN),
    () => tryAutoInstall(true),
    () => probeCapabilitiesSync(MINUTES_BIN)
  );
}

type DesktopAppStatus = {
  pid: number;
  updated_at: string;
  platform: string;
};

type DesktopControlResponse = {
  id: string;
  handled_at: string;
  accepted: boolean;
  detail: string;
};

function desktopControlDir(): string {
  return join(homedir(), ".minutes", "desktop-control");
}

function desktopAppStatusPath(): string {
  return join(desktopControlDir(), "desktop-app.json");
}

function desktopRequestPath(id: string): string {
  return join(desktopControlDir(), "requests", `${id}.json`);
}

function desktopResponsePath(id: string): string {
  return join(desktopControlDir(), "responses", `${id}.json`);
}

function isProcessAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (e: any) {
    // EPERM means the process exists but is owned by a different user — still alive.
    if (e.code === "EPERM") return true;
    return false;
  }
}

async function readRunningDesktopAppStatus(): Promise<DesktopAppStatus | null> {
  let raw: string;
  try {
    raw = await readFile(desktopAppStatusPath(), "utf8");
  } catch (e: any) {
    if (e.code === "ENOENT") return null; // File doesn't exist — app not running
    console.error(`[Minutes] Failed to read desktop status file: ${e.message}`);
    return null;
  }

  try {
    const status = JSON.parse(raw) as DesktopAppStatus;
    const updatedAt = Date.parse(status.updated_at);
    if (!Number.isFinite(updatedAt)) {
      console.error(`[Minutes] Desktop status file has invalid updated_at: ${status.updated_at}`);
      return null;
    }
    const ageMs = Date.now() - updatedAt;
    if (ageMs > 10000) {
      console.error(`[Minutes] Desktop app status stale (${Math.round(ageMs / 1000)}s old, pid=${status.pid})`);
      return null;
    }
    if (!status.pid || !isProcessAlive(status.pid)) return null;
    return status;
  } catch (e: any) {
    console.error(`[Minutes] Failed to parse desktop status file: ${e.message}`);
    return null;
  }
}

async function delegateRecordingToDesktop(args: {
  title?: string;
  mode: "meeting" | "quick-thought";
  intent?: string;
  allow_degraded: boolean;
  language?: string;
}): Promise<DesktopControlResponse | null> {
  const status = await readRunningDesktopAppStatus();
  if (!status) return null;

  const id = `mcp-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  try {
    await mkdir(join(desktopControlDir(), "requests"), { recursive: true });
    await mkdir(join(desktopControlDir(), "responses"), { recursive: true });
  } catch (e: any) {
    console.error(`[Minutes] Failed to create desktop control dirs: ${e.message}`);
    return null;
  }

  const request = {
    id,
    created_at: new Date().toISOString(),
    action: {
      type: "start-recording",
      mode: args.mode,
      intent: args.intent,
      allow_degraded: args.allow_degraded,
      title: args.title,
      language: args.language,
    },
  };

  const requestPath = desktopRequestPath(id);
  const responsePath = desktopResponsePath(id);
  await writeFile(requestPath, JSON.stringify(request, null, 2), "utf8");

  const timeoutAt = Date.now() + 10000;
  try {
    while (Date.now() < timeoutAt) {
      if (existsSync(responsePath)) {
        // The Tauri side writes via tmp → rename, so the file may briefly exist
        // as an empty or partial write. Catch parse errors and keep polling.
        try {
          const response = JSON.parse(
            await readFile(responsePath, "utf8")
          ) as DesktopControlResponse;
          await rm(responsePath, { force: true });
          return response;
        } catch {
          // Partial write or rename in progress — retry on next poll cycle.
        }
      }
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    throw new Error("Minutes desktop app did not respond to the recording request in time.");
  } finally {
    await rm(requestPath, { force: true }).catch(() => {});
  }
}

const CLI_INSTALL_MSG =
  `Recording requires the minutes CLI binary.\n` +
  `Searched: ${MINUTES_BIN}\n\n` +
  `Install it:\n` +
  `  macOS:   brew tap silverstein/tap && brew install minutes\n` +
  `  Any:     cargo install minutes-cli\n` +
  `  Source:  https://github.com/silverstein/minutes\n\n` +
  `If already installed via Homebrew, try:\n` +
  `  sudo ln -s /opt/homebrew/bin/minutes /usr/local/bin/minutes`;

// Common binary locations that may not be in Claude Desktop's restricted PATH.
//
// ~/.minutes/bin is where the Windows auto-installer puts the CLI (#657). The
// MCP server resolves it absolutely via MINUTES_BIN, but plugin skills shell
// out to a bare `minutes`, so without this entry those fail right after an
// otherwise successful install.
const EXTRA_PATH_DIRS = [
  join(homedir(), ".minutes", "bin"),
  join(homedir(), ".local", "bin"),
  join(homedir(), ".cargo", "bin"),
  "/opt/homebrew/bin",
  "/usr/local/bin",
];

export function mcpCliChildEnv(
  extra?: Record<string, string>
): Record<string, string | undefined> {
  const currentPath = process.env.PATH || "";
  const augmentedPath = [...EXTRA_PATH_DIRS, currentPath].join(delimiter);
  return {
    ...process.env,
    PATH: augmentedPath,
    ...extra,
    // A child CLI is still an assistant surface. This assignment is
    // deliberately last so neither the ambient environment nor a call-site
    // override can restore the human CLI's restricted-content access.
    MINUTES_CLI_RESTRICTED_POLICY: "deny",
  };
}

export const LIVE_EVENTS_RESOURCE_URI = "minutes://events/live";
export const LIVE_EVENTS_URI_TEMPLATE = "minutes://events/live{?since_seq,limit}";
export const LIVE_EVENTS_SUBSCRIPTIONS_ENABLED = false;
const LIVE_EVENTS_DEFAULT_RECENT_LIMIT = 20;
const LIVE_EVENTS_DEFAULT_CURSOR_LIMIT = 100;
const LIVE_EVENTS_POLL_INTERVAL_MS = Math.max(
  250,
  Number.parseInt(process.env.MINUTES_MCP_EVENT_POLL_MS || "1000", 10) || 1000
);

export const LIVE_COPILOT_RESOURCE_URI = "minutes://live/copilot";
const COPILOT_NUDGE_LOG_FILENAME = "mcp-copilot-nudges.jsonl";
const COPILOT_STDERR_LOG_FILENAME = "mcp-copilot-stderr.log";
const COPILOT_OBSERVER_SESSION_FILENAME = "mcp-copilot-session.json";
const COPILOT_READ_DEFAULT_LIMIT = 50;
const COPILOT_READ_MAX_LIMIT = 200;

type JsonObject = Record<string, unknown>;

export type CopilotStatusState =
  | "Off"
  | "Arming"
  | "Listening"
  | "Thinking"
  | "Nudge"
  | "Paused"
  | "Degraded";

export type CopilotStatusPayload = {
  schema_version: 1;
  available: boolean;
  active: boolean;
  state: CopilotStatusState;
  pid: number | null;
  surface: "stdout" | "tui" | null;
  evidence_cursor: number;
  input_mode: "realtime" | "final_only";
  setup_needed: boolean;
  error?: string;
};

export type CopilotObserverSession = {
  v: 1;
  pid: number;
  goal: string;
  surface: "stdout" | "tui";
  started_ts: string;
};

export type ObservedCopilotNudge = {
  cursor: number;
  format: "json" | "text" | "raw";
  nudge: JsonObject;
  raw: string;
  observed_ts: string | null;
  expires_at: string | null;
  expired: boolean | null;
};

export type CopilotNudgeObservation = {
  attached: boolean;
  cursor: number;
  session: CopilotObserverSession | null;
  nudges: ObservedCopilotNudge[];
  note: string;
};

export type LiveCopilotResourcePayload = {
  v: 1;
  resource: typeof LIVE_COPILOT_RESOURCE_URI;
  available: boolean;
  active: boolean;
  state: string;
  status: CopilotStatusPayload;
  nudge_stream: {
    attached: boolean;
    cursor: number;
    note: string;
  };
  latest_nudge: ObservedCopilotNudge | null;
  current_nudge: ObservedCopilotNudge | null;
};

export type LiveEventsResourceOptions = {
  uri: string;
  sinceSeq: number | null;
  limit: number;
};

export type LiveEventsResourcePayload = {
  v: 1;
  resource: typeof LIVE_EVENTS_RESOURCE_URI;
  mode: "recent" | "since_seq";
  since_seq: number | null;
  limit: number;
  latest_seq: number;
  events: unknown[];
  reconnect: {
    cursor: number;
    read_uri: string;
  };
};

export function parseLiveEventsResourceUri(rawUri: string): LiveEventsResourceOptions | null {
  let url: URL;
  try {
    url = new URL(rawUri);
  } catch {
    return null;
  }

  if (url.protocol !== "minutes:" || url.hostname !== "events" || url.pathname !== "/live") {
    return null;
  }

  const sinceSeqRaw = url.searchParams.get("since_seq");
  const limitRaw = url.searchParams.get("limit");
  const sinceSeq = parseOptionalNonNegativeInteger(sinceSeqRaw);
  const limit = parseOptionalPositiveInteger(
    limitRaw,
    sinceSeq === null ? LIVE_EVENTS_DEFAULT_RECENT_LIMIT : LIVE_EVENTS_DEFAULT_CURSOR_LIMIT
  );

  if (sinceSeqRaw !== null && sinceSeq === null) {
    throw new McpError(ErrorCode.InvalidParams, "since_seq must be a non-negative integer");
  }
  if (limitRaw !== null && limit === null) {
    throw new McpError(ErrorCode.InvalidParams, "limit must be a positive integer");
  }

  return {
    uri: url.href,
    sinceSeq,
    limit: limit ?? LIVE_EVENTS_DEFAULT_CURSOR_LIMIT,
  };
}

function parseOptionalNonNegativeInteger(raw: string | null): number | null {
  if (raw === null) return null;
  if (!/^\d+$/.test(raw)) return null;
  const parsed = Number.parseInt(raw, 10);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

function parseOptionalPositiveInteger(raw: string | null, fallback: number): number | null {
  if (raw === null) return fallback;
  if (!/^\d+$/.test(raw)) return null;
  const parsed = Number.parseInt(raw, 10);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) return null;
  return Math.min(parsed, 1000);
}

function eventSeq(event: unknown): number {
  if (!event || typeof event !== "object") return 0;
  const seq = (event as JsonObject).seq;
  return typeof seq === "number" && Number.isSafeInteger(seq) && seq >= 0 ? seq : 0;
}

function maxEventSeq(events: unknown[], floor: number = 0): number {
  return events.reduce<number>((max, event) => Math.max(max, eventSeq(event)), floor);
}

export function buildLiveEventsResourcePayload(
  options: LiveEventsResourceOptions,
  events: unknown[],
  latestSeq: number
): LiveEventsResourcePayload {
  const deliveredCursorFloor = options.sinceSeq ?? latestSeq;
  const cursor = maxEventSeq(events, deliveredCursorFloor);
  const latestKnownSeq = maxEventSeq(events, latestSeq);
  return {
    v: 1,
    resource: LIVE_EVENTS_RESOURCE_URI,
    mode: options.sinceSeq === null ? "recent" : "since_seq",
    since_seq: options.sinceSeq,
    limit: options.limit,
    latest_seq: latestKnownSeq,
    events,
    reconnect: {
      cursor,
      read_uri: `${LIVE_EVENTS_RESOURCE_URI}?since_seq=${cursor}`,
    },
  };
}

function inactiveCopilotStatus(error?: string): CopilotStatusPayload {
  return {
    schema_version: 1,
    available: error === undefined,
    active: false,
    state: "Off",
    pid: null,
    surface: null,
    evidence_cursor: 0,
    input_mode: "final_only",
    setup_needed: false,
    ...(error ? { error } : {}),
  };
}

const COPILOT_STATUS_STATES = new Set<CopilotStatusState>([
  "Off",
  "Arming",
  "Listening",
  "Thinking",
  "Nudge",
  "Paused",
  "Degraded",
]);
const COPILOT_STATUS_JSON_KEYS = [
  "active",
  "evidence_cursor",
  "input_mode",
  "pid",
  "schema_version",
  "setup_needed",
  "state",
  "surface",
].sort();

/** Parse the exact, content-free `minutes copilot status --json` contract. */
export function parseCopilotStatusOutput(raw: string): CopilotStatusPayload {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("Copilot status response was invalid.");
  }
  if (!isJsonObject(parsed)) {
    throw new Error("Copilot status response was invalid.");
  }
  const keys = Object.keys(parsed).sort();
  const state = parsed.state;
  const pid = parsed.pid;
  const surface = parsed.surface;
  const inputMode = parsed.input_mode;
  if (
    keys.join("\0") !== COPILOT_STATUS_JSON_KEYS.join("\0") ||
    parsed.schema_version !== 1 ||
    typeof parsed.active !== "boolean" ||
    typeof state !== "string" ||
    !COPILOT_STATUS_STATES.has(state as CopilotStatusState) ||
    !(pid === null || (typeof pid === "number" && Number.isSafeInteger(pid) && pid > 0)) ||
    !(surface === null || surface === "stdout" || surface === "tui") ||
    typeof parsed.evidence_cursor !== "number" ||
    !Number.isSafeInteger(parsed.evidence_cursor) ||
    parsed.evidence_cursor < 0 ||
    (inputMode !== "realtime" && inputMode !== "final_only") ||
    typeof parsed.setup_needed !== "boolean" ||
    (!parsed.active && (state !== "Off" || pid !== null || surface !== null)) ||
    (parsed.active && (state === "Off" || pid === null || surface === null)) ||
    (parsed.setup_needed && parsed.active)
  ) {
    throw new Error("Copilot status response was invalid.");
  }

  return {
    schema_version: 1,
    available: true,
    active: parsed.active,
    state: state as CopilotStatusState,
    pid: pid as number | null,
    surface: surface as "stdout" | "tui" | null,
    evidence_cursor: parsed.evidence_cursor,
    input_mode: inputMode,
    setup_needed: parsed.setup_needed,
  };
}

function isJsonObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isCopilotNudge(value: unknown): value is JsonObject {
  if (!isJsonObject(value)) return false;
  return (
    value.v === 1 &&
    typeof value.id === "string" &&
    ["Say", "Ask", "Clarify", "Hold", "Watch"].includes(String(value.kind)) &&
    typeof value.text === "string" &&
    typeof value.source_chip === "string" &&
    typeof value.evidence_revision === "number" &&
    typeof value.created_ts === "string" &&
    typeof value.ttl_ms === "number"
  );
}

function nudgeExpiry(
  createdTs: unknown,
  ttlMs: unknown,
  nowMs: number
): { observedTs: string | null; expiresAt: string | null; expired: boolean | null } {
  if (typeof createdTs !== "string" || typeof ttlMs !== "number" || ttlMs < 0) {
    return { observedTs: null, expiresAt: null, expired: null };
  }
  const createdMs = Date.parse(createdTs);
  if (!Number.isFinite(createdMs)) {
    return { observedTs: null, expiresAt: null, expired: null };
  }
  const expiresMs = createdMs + ttlMs;
  return {
    observedTs: new Date(createdMs).toISOString(),
    expiresAt: new Date(expiresMs).toISOString(),
    expired: nowMs >= expiresMs,
  };
}

/** Parse the CLI's stdout presentation stream without invoking inference. */
export function parseCopilotNudgeLog(
  raw: string,
  nowMs: number = Date.now(),
  latestObservedMs?: number
): ObservedCopilotNudge[] {
  const entries: ObservedCopilotNudge[] = [];
  const lines = raw.split(/\r?\n/);
  let lastNonEmptyIndex = -1;
  for (let index = lines.length - 1; index >= 0; index--) {
    if (lines[index].trim().length > 0) {
      lastNonEmptyIndex = index;
      break;
    }
  }

  for (let index = 0; index < lines.length; index++) {
    const line = lines[index].trim();
    if (!line) continue;
    const cursor = index + 1;

    try {
      const parsed: unknown = JSON.parse(line);
      if (isCopilotNudge(parsed)) {
        const expiry = nudgeExpiry(parsed.created_ts, parsed.ttl_ms, nowMs);
        entries.push({
          cursor,
          format: "json",
          nudge: parsed,
          raw: line,
          observed_ts: expiry.observedTs,
          expires_at: expiry.expiresAt,
          expired: expiry.expired,
        });
        continue;
      }
    } catch {
      // The `tui` surface emits a compact human-readable line when detached.
    }

    const textMatch = line.match(
      /^\[(Say|Ask|Clarify|Hold|Watch)\]\s+(.+)\s+—\s+(.+)\s+\(evidence r(\d+), ttl (\d+)ms\)$/
    );
    if (textMatch) {
      const isLatest = index === lastNonEmptyIndex;
      const observedMs = isLatest && latestObservedMs !== undefined ? latestObservedMs : null;
      const ttlMs = Number.parseInt(textMatch[5], 10);
      entries.push({
        cursor,
        format: "text",
        nudge: {
          v: 1,
          kind: textMatch[1],
          text: textMatch[2],
          source_chip: textMatch[3],
          evidence_revision: Number.parseInt(textMatch[4], 10),
          ttl_ms: ttlMs,
        },
        raw: line,
        observed_ts: observedMs === null ? null : new Date(observedMs).toISOString(),
        expires_at: observedMs === null ? null : new Date(observedMs + ttlMs).toISOString(),
        expired: observedMs === null ? null : nowMs >= observedMs + ttlMs,
      });
      continue;
    }

    entries.push({
      cursor,
      format: "raw",
      nudge: { raw: line },
      raw: line,
      observed_ts: null,
      expires_at: null,
      expired: null,
    });
  }

  return entries;
}

export function buildLiveCopilotResourcePayload(
  status: CopilotStatusPayload,
  observation: CopilotNudgeObservation
): LiveCopilotResourcePayload {
  const latestNudge = status.active && observation.attached
    ? observation.nudges.at(-1) ?? null
    : null;
  return {
    v: 1,
    resource: LIVE_COPILOT_RESOURCE_URI,
    available: status.available,
    active: status.active,
    state: status.state,
    status,
    nudge_stream: {
      attached: observation.attached,
      cursor: observation.cursor,
      note: observation.note,
    },
    latest_nudge: latestNudge,
    current_nudge: latestNudge?.expired === false ? latestNudge : null,
  };
}

export type CopilotNudgeReadPage = {
  cursor: number;
  next_cursor: number;
  cursor_reset: boolean;
  has_more: boolean;
  nudges: ObservedCopilotNudge[];
};

export function selectCopilotNudges(
  observation: CopilotNudgeObservation,
  options: { cursor?: number; since?: string; limit?: number },
  nowMs: number = Date.now()
): CopilotNudgeReadPage {
  if (options.cursor !== undefined && options.since !== undefined) {
    throw new Error("Use either cursor or since, not both.");
  }

  const limit = Math.min(
    Math.max(options.limit ?? COPILOT_READ_DEFAULT_LIMIT, 1),
    COPILOT_READ_MAX_LIMIT
  );
  let requestedCursor = options.cursor ?? 0;
  let sinceMs: number | null = null;

  if (options.since !== undefined) {
    const since = options.since.trim();
    if (/^\d+$/.test(since)) {
      requestedCursor = Number.parseInt(since, 10);
      if (!Number.isSafeInteger(requestedCursor)) {
        throw new Error("since cursor must be a safe non-negative integer.");
      }
    } else {
      const duration = since.match(/^(\d+)(ms|s|m|h)$/);
      if (duration) {
        const amount = Number.parseInt(duration[1], 10);
        if (!Number.isSafeInteger(amount)) {
          throw new Error("since duration is too large.");
        }
        const multiplier = duration[2] === "ms"
          ? 1
          : duration[2] === "s"
            ? 1000
            : duration[2] === "m"
              ? 60_000
              : 3_600_000;
        sinceMs = nowMs - amount * multiplier;
      } else {
        const parsed = Date.parse(since);
        if (!Number.isFinite(parsed)) {
          throw new Error(
            "since must be a cursor, duration such as '5m' or '30s', or an ISO timestamp."
          );
        }
        sinceMs = parsed;
      }
    }
  }

  if (!Number.isSafeInteger(requestedCursor) || requestedCursor < 0) {
    throw new Error("cursor must be a safe non-negative integer.");
  }

  const cursorReset = requestedCursor > observation.cursor;
  const effectiveCursor = cursorReset ? 0 : requestedCursor;
  const eligible = observation.nudges.filter((entry) => {
    if (entry.cursor <= effectiveCursor) return false;
    if (sinceMs === null) return true;
    if (entry.observed_ts === null) return false;
    return Date.parse(entry.observed_ts) >= sinceMs;
  });
  const nudges = eligible.slice(0, limit);
  const hasMore = eligible.length > nudges.length;
  const nextCursor = hasMore
    ? nudges.at(-1)?.cursor ?? effectiveCursor
    : observation.cursor;

  return {
    cursor: observation.cursor,
    next_cursor: nextCursor,
    cursor_reset: cursorReset,
    has_more: hasMore,
    nudges,
  };
}

// ── Helper: run minutes CLI command (uses execFile, not exec) ──

/**
 * Ceiling on one CLI read, applied per stream.
 *
 * `execFile` defaults to 1 MiB and turns an overrun into a hard error, not a
 * truncation, so the ceiling has to clear the largest projection any surface
 * asks for. The insight read is the binding case: it now always fetches
 * MCP_INSIGHT_SCAN_WINDOW records rather than the caller's default, measured at
 * roughly 620 bytes per record on a real log, so about 310 KB today. The
 * headroom is for corpora with long decision text, and is forward-looking
 * rather than measured.
 *
 * Two things this is NOT, stated because the number is large and applies to
 * every `runMinutes` caller, not only insights. Node applies `maxBuffer` per
 * stream, so a call may buffer this much stdout AND this much stderr, and every
 * child runs with RUST_LOG=info. And the old 1 MiB default doubled as a
 * circuit breaker on a runaway child; that breaker is now much further out. It
 * is set here rather than per-call because `MinutesRunner` takes no options
 * bag, and one ceiling that every surface shares is easier to reason about
 * than several.
 */
const MCP_CLI_MAX_STDOUT_BYTES = 64 * 1024 * 1024;

/**
 * What a person should actually do when the CLI is absent, phrased for the
 * surface they installed.
 *
 * The server downloads a prebuilt CLI on first run, so this text is only ever
 * reached when that could not complete: no network, a proxy, or a platform
 * with no published binary. Naming the likely cause matters more than naming
 * the component, because the reader is usually a Claude Desktop user who never
 * chose to have a CLI at all (#774).
 */
export function cliMissingGuidance(
  platform: NodeJS.Platform = process.platform
): string {
  // Only macOS and Windows have a desktop app. Linux ships the CLI alone, so
  // pointing a Linux user at "the app" sends them to a download that does not
  // exist.
  const manual =
    platform === "darwin" || platform === "win32"
      ? "Install the free Minutes desktop app from https://useminutes.app, which includes the engine and keeps it updated."
      : "Install the Minutes CLI from https://useminutes.app, or run `cargo install minutes-cli`.";
  return (
    "The Minutes engine is not installed yet. Minutes normally installs it " +
    "automatically on first run, so this usually means it could not be " +
    "downloaded: no internet access, a proxy, or an unsupported platform. " +
    `${manual} Then restart this app.`
  );
}

async function runMinutes(
  args: string[],
  timeoutMs: number = 30000,
  signal?: AbortSignal,
  extraEnv: Record<string, string> = {}
): Promise<{ stdout: string; stderr: string }> {
  try {
    const { stdout, stderr } = await execFileAsync(MINUTES_BIN, args, {
      timeout: timeoutMs,
      signal,
      maxBuffer: MCP_CLI_MAX_STDOUT_BYTES,
      env: mcpCliChildEnv({ RUST_LOG: "info", ...extraEnv }),
    });
    return { stdout: stdout.trim(), stderr: stderr.trim() };
  } catch (error: any) {
    if (error.killed) {
      throw new Error(`Command timed out after ${timeoutMs}ms`);
    }
    // Every tool reaches the CLI through here, including the readiness probe
    // behind the content-bearing tools, so this one branch is what turns a
    // missing engine into advice instead of ENOENT.
    if (error?.code === "ENOENT") {
      const missing = new Error(cliMissingGuidance());
      // Tagged rather than string-matched, so callers that deliberately keep
      // their own failures opaque can still let this one through.
      (missing as any).cliMissing = true;
      throw missing;
    }
    const stderr = error.stderr?.trim() || "";
    const stdout = error.stdout?.trim() || "";
    // Preserve both streams on the thrown error: some commands (e.g.
    // `resummarize --json`) print a structured failure envelope to stdout
    // while anyhow's text lands on stderr — message alone would lose it.
    const wrapped = new Error(stderr || stdout || error.message);
    (wrapped as any).stdout = stdout;
    (wrapped as any).stderr = stderr;
    throw wrapped;
  }
}

async function runPolicyGraphMinutes(
  args: string[]
): Promise<{ stdout: string; stderr: string }> {
  const corpusRoot = await getEffectiveMeetingsDir();
  return runMinutes(args, 30000, undefined, {
    MINUTES_POLICY_GRAPH_CORPUS_ROOT: corpusRoot,
  });
}

type MinutesRunner = (
  args: string[],
  timeoutMs?: number
) => Promise<{ stdout: string; stderr: string }>;

export type KnowledgeStatusSnapshot = {
  enabled: boolean;
  configured: boolean;
  adapter: string | null;
  engine: string | null;
  people_count: number;
  log_entries: number;
};

/**
 * Single fail-closed bridge for MCP knowledge status. Rust strict-loads the
 * authoritative config, reconciles and counts under one policy lock, and
 * refreshes or disables configured QMD before returning this path-free result.
 * JavaScript must not reopen config or the knowledge tree afterward.
 */
export async function readKnowledgeStatusSnapshot(
  runner: MinutesRunner = runMinutes
): Promise<KnowledgeStatusSnapshot> {
  const { stdout } = await runner(["knowledge-status", "--json"]);
  let result: any;
  try {
    result = JSON.parse(stdout);
  } catch {
    throw new Error("Persistent meeting derivatives could not be safely read.");
  }
  if (
    !result ||
    typeof result.enabled !== "boolean" ||
    typeof result.configured !== "boolean" ||
    !(typeof result.adapter === "string" || result.adapter === null) ||
    !(typeof result.engine === "string" || result.engine === null) ||
    !Number.isSafeInteger(result.people_count) ||
    result.people_count < 0 ||
    !Number.isSafeInteger(result.log_entries) ||
    result.log_entries < 0
  ) {
    throw new Error("Persistent meeting derivatives could not be safely read.");
  }
  return result as KnowledgeStatusSnapshot;
}

export type AgentTrustReadiness = {
  schema: 1;
  ready: boolean;
  qmd_retirement: "ready-clean" | "blocked";
  remediation?: string;
};

export async function readAgentTrustReadiness(
  runner: MinutesRunner = runMinutes
): Promise<AgentTrustReadiness> {
  let stdout: string;
  try {
    ({ stdout } = await runner(["agent-readiness", "--json"]));
  } catch (error: any) {
    // A malformed or unverifiable readiness answer stays opaque on purpose:
    // the caller must not learn why the boundary refused. A missing engine is
    // not that. It is the reader's own machine state, it is already printed by
    // the capture tools, and withholding it leaves a Claude Desktop user with
    // "could not be verified safely" and nothing to do about it (#774).
    if (error?.cliMissing) throw error;
    throw new Error("Minutes agent readiness could not be verified safely.");
  }
  let result: any;
  try {
    result = JSON.parse(stdout);
  } catch {
    throw new Error("Minutes agent readiness could not be verified safely.");
  }
  if (
    !result ||
    result.schema !== 1 ||
    typeof result.ready !== "boolean" ||
    !["ready-clean", "blocked"].includes(
      result.qmd_retirement
    ) ||
    (result.ready && result.qmd_retirement === "blocked") ||
    (!result.ready && result.qmd_retirement !== "blocked") ||
    (result.ready && result.remediation !== undefined) ||
    (!result.ready &&
      (typeof result.remediation !== "string" ||
        result.remediation.trim().length === 0))
  ) {
    throw new Error("Minutes agent readiness could not be verified safely.");
  }
  return result as AgentTrustReadiness;
}

export async function requireAgentTrustReadiness(
  runner: MinutesRunner = runMinutes
): Promise<AgentTrustReadiness> {
  const readiness = await readAgentTrustReadiness(runner);
  if (!readiness.ready) {
    throw new Error(
      readiness.remediation ||
        "This machine shows a Minutes-owned QMD registration that qmd could not confirm was removed. Make sure `qmd` runs (`qmd collection list`), then run `minutes qmd cleanup` and restart Minutes."
    );
  }
  return readiness;
}

export async function afterAgentTrustReadiness<T>(
  operation: () => Promise<T>,
  runner: MinutesRunner = runMinutes
): Promise<T> {
  await requireAgentTrustReadiness(runner);
  return operation();
}

export async function afterRequiredCli<T>(
  operation: () => Promise<T>,
  cliProbe: () => Promise<boolean> = isCliAvailable
): Promise<T> {
  let available = false;
  try {
    available = await cliProbe();
  } catch {
    // Installation/probe diagnostics stay local; the MCP failure is path-free.
  }
  if (!available) {
    throw new Error(
      "Minutes CLI is required to establish the agent trust boundary."
    );
  }
  return operation();
}

const OPERATIONAL_JOB_STATES = {
  queued: { state: "queued", stage: "Queued for processing" },
  transcribing: { state: "transcribing", stage: "Transcribing" },
  transcriptonly: { state: "transcript-ready", stage: "Transcript ready" },
  diarizing: { state: "diarizing", stage: "Separating speakers" },
  summarizing: { state: "summarizing", stage: "Generating summary" },
  saving: { state: "saving", stage: "Saving" },
  needsreview: { state: "needs-review", stage: "Needs review" },
  complete: { state: "complete", stage: "Complete" },
  failed: { state: "failed", stage: "Failed" },
} as const;

type PrivacySafeJobState =
  | (typeof OPERATIONAL_JOB_STATES)[keyof typeof OPERATIONAL_JOB_STATES]["state"]
  | "unknown";

export type PrivacySafeProcessingJob = {
  id: string;
  state: PrivacySafeJobState;
  stage: string;
};

export type PrivacySafeRecordingStatus = {
  schema_version: 1;
  status_available: boolean;
  recording: boolean;
  processing: boolean;
  recording_mode: "meeting" | "quick-thought" | "dictation" | "live-transcript" | null;
  processing_stage: string | null;
  processing_job_count: number;
};

function normalizedOperationalToken(value: unknown): string {
  return typeof value === "string"
    ? value.toLowerCase().replace(/[^a-z0-9]+/g, "")
    : "";
}

function operationalJobState(value: unknown): {
  state: PrivacySafeJobState;
  stage: string;
} {
  const normalized = normalizedOperationalToken(value);
  const aliases: Record<string, keyof typeof OPERATIONAL_JOB_STATES> = {
    queued: "queued",
    transcribing: "transcribing",
    transcriptonly: "transcriptonly",
    transcriptready: "transcriptonly",
    diarizing: "diarizing",
    summarizing: "summarizing",
    saving: "saving",
    needsreview: "needsreview",
    complete: "complete",
    completed: "complete",
    failed: "failed",
  };
  const key = aliases[normalized];
  return key
    ? OPERATIONAL_JOB_STATES[key]
    : { state: "unknown", stage: "Status unavailable" };
}

function privacySafeJobId(value: unknown, index: number): string {
  return typeof value === "string" && /^job-\d{17}-\d+-\d+$/.test(value)
    ? value
    : `job-${index + 1}`;
}

/**
 * Project the CLI's intentionally rich local job record into the complete MCP
 * job schema. The projection is closed: source titles, paths, notes, context,
 * consent, calendar fields, templates, raw stages, and errors can never enter
 * either MCP text or structuredContent.
 */
export function privacySafeProcessingJobs(value: unknown): PrivacySafeProcessingJob[] {
  if (!Array.isArray(value)) {
    throw new Error("Processing jobs could not be safely read.");
  }
  const jobs: PrivacySafeProcessingJob[] = [];
  for (const [index, rawJob] of value.entries()) {
    const job = rawJob !== null && typeof rawJob === "object" ? rawJob as any : {};
    const operational = operationalJobState(job.state);
    jobs.push({
      id: privacySafeJobId(job.id, index),
      state: operational.state,
      stage: operational.stage,
    });
    if (jobs.length >= MCP_PROCESSING_JOB_RESULT_MAX) break;
  }
  return jobs;
}

export function buildPrivacySafeProcessingJobsResult(value: unknown) {
  const jobs = privacySafeProcessingJobs(value);
  if (jobs.length === 0) {
    return {
      content: [{ type: "text" as const, text: "No processing jobs right now." }],
      structuredContent: { jobs },
    };
  }
  const lines = jobs.map((job) => `- ${job.id}: ${job.state} — ${job.stage}`);
  return {
    content: [
      {
        type: "text" as const,
        text: `Processing jobs:\n\n${lines.join("\n")}`,
      },
    ],
    structuredContent: { jobs },
  };
}

function privacySafeRecordingMode(
  value: unknown
): PrivacySafeRecordingStatus["recording_mode"] {
  switch (normalizedOperationalToken(value)) {
    case "meeting":
      return "meeting";
    case "quickthought":
      return "quick-thought";
    case "dictation":
      return "dictation";
    case "livetranscript":
      return "live-transcript";
    default:
      return null;
  }
}

function privacySafeProcessingStage(value: unknown): string | null {
  const stages: Record<string, string> = {
    queuedforprocessing: "Queued for processing",
    transcribing: "Transcribing",
    transcribingaudio: "Transcribing",
    transcribingmeeting: "Transcribing",
    transcriptready: "Transcript ready",
    transcriptreadyenrichingartifact: "Transcript ready",
    separatingspeakers: "Separating speakers",
    generatingsummary: "Generating summary",
    saving: "Saving",
    savingartifact: "Saving",
    needsreview: "Needs review",
    needsreviewrawcapturepreserved: "Needs review",
    processingfailed: "Failed",
  };
  return stages[normalizedOperationalToken(value)] ?? null;
}

/** Build the entire path-free MCP status schema from untrusted CLI JSON. */
export function privacySafeRecordingStatus(value: unknown): PrivacySafeRecordingStatus {
  if (
    value === null ||
    typeof value !== "object" ||
    typeof (value as any).recording !== "boolean" ||
    typeof (value as any).processing !== "boolean"
  ) {
    return {
      schema_version: 1,
      status_available: false,
      recording: false,
      processing: false,
      recording_mode: null,
      processing_stage: null,
      processing_job_count: 0,
    };
  }

  const raw = value as any;
  return {
    schema_version: 1,
    status_available: true,
    recording: raw.recording,
    processing: raw.processing,
    recording_mode: privacySafeRecordingMode(raw.recording_mode),
    processing_stage: raw.processing
      ? privacySafeProcessingStage(raw.processing_stage)
      : null,
    processing_job_count:
      raw.processing &&
      Number.isSafeInteger(raw.processing_job_count) &&
      raw.processing_job_count >= 0
        ? raw.processing_job_count
        : 0,
  };
}

export function buildPrivacySafeStatusText(value: unknown): string {
  const status = privacySafeRecordingStatus(value);
  if (!status.status_available) {
    return "Recording status is unavailable.";
  }
  const modeLabel = status.recording_mode === "quick-thought" ? "Quick thought" : "Recording";
  const processingLabel =
    status.recording_mode === "quick-thought" ? "Quick thought processing" : "Processing";
  if (status.recording) {
    return `${modeLabel} in progress.`;
  }
  if (!status.processing) {
    return "No recording in progress.";
  }
  const stage = status.processing_stage ? `: ${status.processing_stage}` : ".";
  const queue =
    status.processing_job_count > 1
      ? ` (${status.processing_job_count} jobs queued)`
      : "";
  return `${processingLabel}${stage}${queue}`;
}

export function buildPrivacySafeStatusResource(value: unknown) {
  return {
    contents: [
      {
        uri: "minutes://status",
        mimeType: "application/json",
        text: JSON.stringify(privacySafeRecordingStatus(value)),
      },
    ],
  };
}

function parseJsonOutput(stdout: string): any {
  try {
    return JSON.parse(stdout);
  } catch {
    return { raw: stdout };
  }
}

function copilotObserverPaths(minutesHome: string = join(homedir(), ".minutes")) {
  return {
    root: minutesHome,
    nudges: join(minutesHome, COPILOT_NUDGE_LOG_FILENAME),
    stderr: join(minutesHome, COPILOT_STDERR_LOG_FILENAME),
    session: join(minutesHome, COPILOT_OBSERVER_SESSION_FILENAME),
  };
}

export async function readCopilotStatusFromCli(
  runner: MinutesRunner = runMinutes,
  cliAvailable: () => Promise<boolean> = isCliAvailable
): Promise<CopilotStatusPayload> {
  if (!(await cliAvailable())) {
    return inactiveCopilotStatus("Minutes CLI is not installed; copilot control requires the local CLI.");
  }

  try {
    const { stdout } = await runner(["copilot", "status", "--json"], 5000);
    return parseCopilotStatusOutput(stdout);
  } catch {
    return inactiveCopilotStatus("Unable to read copilot status safely.");
  }
}

function parseCopilotObserverSession(raw: string): CopilotObserverSession | null {
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!isJsonObject(parsed)) return null;
    if (
      parsed.v !== 1 ||
      typeof parsed.pid !== "number" ||
      !Number.isSafeInteger(parsed.pid) ||
      parsed.pid <= 0 ||
      typeof parsed.goal !== "string" ||
      (parsed.surface !== "stdout" && parsed.surface !== "tui") ||
      typeof parsed.started_ts !== "string"
    ) {
      return null;
    }
    return parsed as CopilotObserverSession;
  } catch {
    return null;
  }
}

function observerMatchesStatus(
  session: CopilotObserverSession,
  status: CopilotStatusPayload
): boolean {
  if (!status.active) return false;
  if (status.pid !== null && session.pid !== status.pid) return false;
  return session.surface === status.surface;
}

async function readCopilotNudgeObservation(
  status: CopilotStatusPayload,
  minutesHome: string = join(homedir(), ".minutes")
): Promise<CopilotNudgeObservation> {
  if (!status.active) {
    return {
      attached: false,
      cursor: 0,
      session: null,
      nudges: [],
      note: "Copilot is not active.",
    };
  }

  const paths = copilotObserverPaths(minutesHome);
  let session: CopilotObserverSession | null = null;
  try {
    session = parseCopilotObserverSession(await readFile(paths.session, "utf8"));
  } catch {
    // A copilot started directly from a terminal has no MCP observer sidecar.
  }

  if (!session || !observerMatchesStatus(session, status)) {
    return {
      attached: false,
      cursor: 0,
      session,
      nudges: [],
      note:
        "Copilot is active, but this nudge stream is not attached. " +
        "The session was started outside start_copilot (or belongs to a different process); state remains observable via copilot_status.",
    };
  }

  try {
    const [raw, fileStat] = await Promise.all([
      readFile(paths.nudges, "utf8"),
      stat(paths.nudges),
    ]);
    const nudges = parseCopilotNudgeLog(raw, Date.now(), fileStat.mtimeMs);
    return {
      attached: true,
      cursor: nudges.at(-1)?.cursor ?? 0,
      session,
      nudges,
      note: session.surface === "stdout"
        ? "Attached to the real CLI stdout nudge stream."
        : "Attached to the real CLI TUI text stream; use surface=stdout for complete structured nudge fields.",
    };
  } catch (error: unknown) {
    const code = isJsonObject(error) && typeof error.code === "string" ? error.code : null;
    if (code === "ENOENT") {
      return {
        attached: true,
        cursor: 0,
        session,
        nudges: [],
        note: "Attached to the real CLI nudge stream; no nudges have been emitted yet.",
      };
    }
    return {
      attached: false,
      cursor: 0,
      session,
      nudges: [],
      note: `Copilot is active, but its observation log could not be read: ${error instanceof Error ? error.message : String(error)}`,
    };
  }
}

async function getLiveCopilotSnapshot(): Promise<LiveCopilotResourcePayload> {
  const status = await readCopilotStatusFromCli();
  return afterActiveCopilotReadiness(status, async () => {
    const observation = await readCopilotNudgeObservation(status);
    return buildLiveCopilotResourcePayload(status, observation);
  });
}

/// An inactive copilot status is operational metadata and contains no meeting
/// content. Once active, its observation stream can include derived nudges, so
/// the QMD-retirement boundary must pass before the stream is read at all.
export async function afterActiveCopilotReadiness<T>(
  status: Pick<CopilotStatusPayload, "active">,
  operation: () => T | Promise<T>,
  readiness: () => Promise<unknown> = () => requireAgentTrustReadiness()
): Promise<T> {
  if (status.active) {
    await readiness();
  }
  return operation();
}

async function readLiveCopilotResource(uri: URL): Promise<{
  contents: Array<{ uri: string; mimeType: string; text: string }>;
}> {
  if (uri.href !== LIVE_COPILOT_RESOURCE_URI) {
    throw new McpError(
      ErrorCode.InvalidParams,
      `Unsupported live copilot resource: ${uri.href}`
    );
  }
  const payload = await getLiveCopilotSnapshot();
  return {
    contents: [{
      uri: uri.href,
      mimeType: "application/json",
      text: JSON.stringify(payload, null, 2),
    }],
  };
}

async function liveCopilotFingerprint(): Promise<string> {
  const payload = await getLiveCopilotSnapshot();
  return JSON.stringify({
    available: payload.available,
    active: payload.active,
    state: payload.state,
    status: payload.status,
    attached: payload.nudge_stream.attached,
    cursor: payload.nudge_stream.cursor,
    latest: payload.latest_nudge?.raw ?? null,
  });
}

async function spawnCopilotCli(
  goal: string,
  surface: "stdout" | "tui"
): Promise<CopilotObserverSession> {
  const paths = copilotObserverPaths();
  await mkdir(paths.root, { recursive: true });
  await Promise.all([
    writeFile(paths.nudges, "", "utf8"),
    writeFile(paths.stderr, "", "utf8"),
  ]);

  const stdoutFd = openSync(paths.nudges, "a");
  const stderrFd = openSync(paths.stderr, "a");
  let child: ReturnType<typeof spawn>;
  try {
    child = spawn(
      MINUTES_BIN,
      ["copilot", "start", "--goal", goal, "--surface", surface],
      {
        detached: true,
        stdio: ["ignore", stdoutFd, stderrFd],
        env: mcpCliChildEnv({ RUST_LOG: "info" }),
      }
    );
  } finally {
    closeSync(stdoutFd);
    closeSync(stderrFd);
  }

  await new Promise<void>((resolveSpawn, rejectSpawn) => {
    child.once("spawn", resolveSpawn);
    child.once("error", rejectSpawn);
  });
  if (!child.pid) {
    throw new Error("Copilot process started without a process identifier.");
  }

  const session: CopilotObserverSession = {
    v: 1,
    pid: child.pid,
    goal,
    surface,
    started_ts: new Date().toISOString(),
  };
  child.unref();
  try {
    await writeFile(paths.session, JSON.stringify(session, null, 2), "utf8");
  } catch (error) {
    try {
      process.kill(session.pid, "SIGTERM");
    } catch {
      // The child may already have exited; preserve the original sidecar error.
    }
    throw error;
  }
  return session;
}

function processIsAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function readCopilotStderrTail(): Promise<string> {
  try {
    const raw = (await readFile(copilotObserverPaths().stderr, "utf8")).trim();
    return raw.slice(-4000);
  } catch {
    return "";
  }
}

async function waitForCopilotStatus(
  predicate: (status: CopilotStatusPayload) => boolean,
  timeoutMs: number
): Promise<CopilotStatusPayload> {
  const deadline = Date.now() + timeoutMs;
  let status = await readCopilotStatusFromCli();
  while (!predicate(status) && Date.now() < deadline) {
    await new Promise((resolveWait) => setTimeout(resolveWait, 200));
    status = await readCopilotStatusFromCli();
  }
  return status;
}

export type CopilotStopAfterControl =
  | { mayRevealContent: false }
  | { mayRevealContent: true; status: CopilotStatusPayload };

/**
 * Issue the idempotent terminal control before status observation. The strict
 * status schema is path-free operational metadata: an inactive result is safe
 * to return without QMD readiness. If the engine remains active, readiness is
 * required before that active session can be surfaced.
 */
export async function stopCopilotBeforeStatusRead(
  control: () => Promise<unknown> = () => runMinutes(["copilot", "stop"], 5000),
  readStatus: () => Promise<CopilotStatusPayload> = () =>
    waitForCopilotStatus((candidate) => !candidate.active, 3000),
  readiness: () => Promise<unknown> = () => requireAgentTrustReadiness()
): Promise<CopilotStopAfterControl> {
  await control();
  const status = await readStatus();
  if (!status.active) {
    return { mayRevealContent: true, status };
  }
  try {
    await readiness();
  } catch {
    return { mayRevealContent: false };
  }
  return { mayRevealContent: true, status };
}

async function readEventsFromCli(args: string[]): Promise<unknown[]> {
  if (!(await isCliAvailable())) {
    return [];
  }
  const { stdout } = await runMinutes(args, 10000);
  const parsed = parseJsonOutput(stdout);
  return Array.isArray(parsed) ? parsed : [];
}

async function readRecentEventsFromCli(limit: number): Promise<unknown[]> {
  return readEventsFromCli(["events", "--limit", String(limit)]);
}

async function readAgentAnnotationsFromCli(limit: number): Promise<any[]> {
  const events = await readEventsFromCli([
    "events",
    "--event-type",
    "agent.annotation",
    "--limit",
    String(limit),
  ]);
  return events.filter((event: any) => event?.event_type === "agent.annotation");
}

async function readEventsSinceSeqFromCli(sinceSeq: number, limit: number): Promise<unknown[]> {
  return readEventsFromCli(["events", "--since-seq", String(sinceSeq), "--limit", String(limit)]);
}

function parseStructuredCliError(message: string): any | null {
  const trimmed = message.trim();
  const start = trimmed.indexOf("{");
  const end = trimmed.lastIndexOf("}");
  if (start === -1 || end === -1 || end < start) {
    return null;
  }
  try {
    return JSON.parse(trimmed.slice(start, end + 1));
  } catch {
    return null;
  }
}

function formatResummarizeFailure(data: any, fallback: string): string {
  if (!data?.error) return fallback;
  return `Resummarize failed${data.stage ? ` during ${data.stage}` : ""}: ${data.error}`;
}

async function latestEventSeqFromCli(): Promise<number> {
  const events = await readRecentEventsFromCli(1);
  return maxEventSeq(events);
}

export async function readLiveEventsResource(uri: URL): Promise<{
  contents: Array<{ uri: string; mimeType: string; text: string }>;
}> {
  const options = parseLiveEventsResourceUri(uri.href);
  if (!options) {
    throw new McpError(ErrorCode.InvalidParams, `Unsupported live events resource: ${uri.href}`);
  }

  // Raw event cursors advance for restricted markers and overrides even when
  // their bodies are removed. A constant unavailable surface is therefore the
  // only honest default until event records carry independently verifiable
  // source policy provenance and a non-sensitive cursor namespace.
  const stableCursor = options.sinceSeq ?? 0;
  const payload = {
    ...buildLiveEventsResourcePayload(options, [], stableCursor),
    unavailable:
      "Live event reads and subscriptions are withheld from MCP until records carry live-verifiable source policy provenance and a non-sensitive cursor.",
  };

  return {
    contents: [{
      uri: uri.href,
      mimeType: "application/json",
      text: JSON.stringify(payload, null, 2),
    }],
  };
}

export type LiveEventsSubscriptionOptions = {
  pollIntervalMs?: number;
  enableLiveEvents?: boolean;
  enableCopilot?: boolean;
  latestEventSeq?: () => Promise<number>;
  readEventsSinceSeq?: (sinceSeq: number, limit: number) => Promise<unknown[]>;
  copilotFingerprint?: () => Promise<string>;
  resourceReadiness?: () => Promise<unknown>;
  sendResourceUpdated?: (uri: string) => Promise<void>;
  onError?: (error: unknown) => void;
};

export type LiveEventsSubscriptionController = {
  stop: () => void;
  subscriptionCount: () => number;
};

export function registerLiveEventsSubscriptionHandlers(
  mcpServer: McpServer,
  options: LiveEventsSubscriptionOptions = {}
): LiveEventsSubscriptionController {
  const eventSubscriptions = new Set<string>();
  const copilotSubscriptions = new Set<string>();
  const enableLiveEvents =
    options.enableLiveEvents ?? LIVE_EVENTS_SUBSCRIPTIONS_ENABLED;
  const enableCopilot = options.enableCopilot ?? false;
  const pollIntervalMs = options.pollIntervalMs ?? LIVE_EVENTS_POLL_INTERVAL_MS;
  const loadLatestSeq = options.latestEventSeq ?? latestEventSeqFromCli;
  const loadEventsSinceSeq = options.readEventsSinceSeq ?? readEventsSinceSeqFromCli;
  const loadCopilotFingerprint = options.copilotFingerprint ?? liveCopilotFingerprint;
  const resourceReadiness = options.resourceReadiness ?? requireAgentTrustReadiness;
  const sendResourceUpdated = options.sendResourceUpdated ??
    ((uri: string) => mcpServer.server.sendResourceUpdated({ uri }));
  const onError = options.onError ?? ((error: unknown) => {
    console.error(`[Minutes] live resource subscription failed: ${error instanceof Error ? error.message : String(error)}`);
  });

  let cursor = 0;
  let eventCursorInitialized = false;
  let copilotFingerprint: string | null = null;
  let pollTimer: NodeJS.Timeout | null = null;
  let pollInFlight = false;
  let lifecycleEpoch = 0;
  let controllerStopped = false;

  function epochIsCurrent(epoch: number): boolean {
    return !controllerStopped && lifecycleEpoch === epoch;
  }

  mcpServer.server.registerCapabilities({
    resources: { subscribe: true },
  });

  async function initializeSubscribedResources(epoch: number): Promise<void> {
    if (eventSubscriptions.size > 0 && !eventCursorInitialized) {
      try {
        const initialCursor = await afterContentResourceReadiness(
          "live_events",
          loadLatestSeq,
          resourceReadiness
        );
        await resourceReadiness();
        if (!epochIsCurrent(epoch) || eventSubscriptions.size === 0) return;
        cursor = initialCursor;
        eventCursorInitialized = true;
      } catch (error) {
        onError(error);
      }
    }
    if (copilotSubscriptions.size > 0 && copilotFingerprint === null) {
      try {
        const initialFingerprint = await afterContentResourceReadiness(
          "live_copilot",
          loadCopilotFingerprint,
          resourceReadiness
        );
        await resourceReadiness();
        if (!epochIsCurrent(epoch) || copilotSubscriptions.size === 0) return;
        copilotFingerprint = initialFingerprint;
      } catch (error) {
        onError(error);
      }
    }
  }

  async function ensurePollerStarted(): Promise<void> {
    const epoch = lifecycleEpoch;
    await initializeSubscribedResources(epoch);
    if (
      !epochIsCurrent(epoch) ||
      (eventSubscriptions.size === 0 && copilotSubscriptions.size === 0) ||
      pollTimer
    ) return;
    pollTimer = setInterval(() => {
      void pollOnce();
    }, pollIntervalMs);
    pollTimer.unref?.();
  }

  function stopPollerIfIdle(): void {
    if (eventSubscriptions.size > 0 || copilotSubscriptions.size > 0) return;
    if (pollTimer) clearInterval(pollTimer);
    pollTimer = null;
    eventCursorInitialized = false;
    copilotFingerprint = null;
  }

  async function notifySubscriptions(
    subscriptions: Set<string>,
    resourceName: "live_events" | "live_copilot",
    epoch: number
  ): Promise<boolean> {
    const subscribedUris = [...subscriptions];
    if (subscribedUris.length === 0 || !epochIsCurrent(epoch)) return false;
    const results = await Promise.all(subscribedUris.map(async (uri) => {
      try {
        await afterContentResourceReadiness(
          resourceName,
          async () => {
            if (!epochIsCurrent(epoch) || !subscriptions.has(uri)) {
              throw new Error("live resource subscription changed during notification");
            }
            await sendResourceUpdated(uri);
          },
          resourceReadiness
        );
        return epochIsCurrent(epoch) && subscriptions.has(uri);
      } catch (error) {
        onError(error);
        return false;
      }
    }));
    return epochIsCurrent(epoch) && results.every(Boolean);
  }

  async function pollOnce(): Promise<void> {
    if (
      pollInFlight ||
      (eventSubscriptions.size === 0 && copilotSubscriptions.size === 0)
    ) {
      return;
    }
    const epoch = lifecycleEpoch;
    pollInFlight = true;
    try {
      await initializeSubscribedResources(epoch);
      if (!epochIsCurrent(epoch)) return;

      if (eventSubscriptions.size > 0 && eventCursorInitialized) {
        try {
          const readCursor = cursor;
          const events = await afterContentResourceReadiness(
            "live_events",
            () => loadEventsSinceSeq(readCursor, LIVE_EVENTS_DEFAULT_CURSOR_LIMIT),
            resourceReadiness
          );
          await resourceReadiness();
          if (!epochIsCurrent(epoch) || eventSubscriptions.size === 0) return;
          const nextCursor = maxEventSeq(events, readCursor);
          if (nextCursor > readCursor) {
            // Authorization may be revoked while the source read is pending.
            // Do not advance the durable subscription baseline unless every
            // notification passed a fresh post-read readiness boundary.
            if (await notifySubscriptions(eventSubscriptions, "live_events", epoch)) {
              cursor = nextCursor;
            }
          }
        } catch (error) {
          onError(error);
        }
      }

      if (copilotSubscriptions.size > 0 && copilotFingerprint !== null) {
        try {
          const nextFingerprint = await afterContentResourceReadiness(
            "live_copilot",
            loadCopilotFingerprint,
            resourceReadiness
          );
          await resourceReadiness();
          if (!epochIsCurrent(epoch) || copilotSubscriptions.size === 0) return;
          if (copilotFingerprint !== null && nextFingerprint !== copilotFingerprint) {
            if (await notifySubscriptions(copilotSubscriptions, "live_copilot", epoch)) {
              copilotFingerprint = nextFingerprint;
            }
          } else {
            copilotFingerprint = nextFingerprint;
          }
        } catch (error) {
          onError(error);
        }
      }
    } finally {
      pollInFlight = false;
    }
  }

  function normalizeSubscriptionUri(
    rawUri: string
  ): { kind: "events" | "copilot"; uri: string } {
    if (enableLiveEvents) {
      const parsed = parseLiveEventsResourceUri(rawUri);
      if (parsed) {
        return {
          kind: "events",
          uri: parsed.sinceSeq === null && parsed.limit === LIVE_EVENTS_DEFAULT_RECENT_LIMIT
            ? LIVE_EVENTS_RESOURCE_URI
            : parsed.uri,
        };
      }
    }
    if (enableCopilot && rawUri === LIVE_COPILOT_RESOURCE_URI) {
      return { kind: "copilot", uri: LIVE_COPILOT_RESOURCE_URI };
    }
    const supported = [
      ...(enableLiveEvents ? [LIVE_EVENTS_RESOURCE_URI] : []),
      ...(enableCopilot ? [LIVE_COPILOT_RESOURCE_URI] : []),
    ];
    throw new McpError(
      ErrorCode.InvalidParams,
      `Only live resource subscriptions are supported: ${supported.join(", ")}`
    );
  }

  mcpServer.server.setRequestHandler(SubscribeRequestSchema, async (request) => {
    if (controllerStopped) {
      throw new McpError(ErrorCode.InvalidRequest, "Live resource subscriptions are stopped");
    }
    const subscription = normalizeSubscriptionUri(request.params.uri);
    const subscriptions = subscription.kind === "events"
      ? eventSubscriptions
      : copilotSubscriptions;
    if (!subscriptions.has(subscription.uri)) {
      subscriptions.add(subscription.uri);
      lifecycleEpoch += 1;
    }
    await ensurePollerStarted();
    return {};
  });

  mcpServer.server.setRequestHandler(UnsubscribeRequestSchema, async (request) => {
    const subscription = normalizeSubscriptionUri(request.params.uri);
    const subscriptions = subscription.kind === "events"
      ? eventSubscriptions
      : copilotSubscriptions;
    if (subscriptions.delete(subscription.uri)) {
      lifecycleEpoch += 1;
      if (subscription.kind === "events" && eventSubscriptions.size === 0) {
        eventCursorInitialized = false;
      }
      if (subscription.kind === "copilot" && copilotSubscriptions.size === 0) {
        copilotFingerprint = null;
      }
    }
    stopPollerIfIdle();
    return {};
  });

  return {
    stop: () => {
      controllerStopped = true;
      lifecycleEpoch += 1;
      eventSubscriptions.clear();
      copilotSubscriptions.clear();
      stopPollerIfIdle();
    },
    subscriptionCount: () => eventSubscriptions.size + copilotSubscriptions.size,
  };
}

// ── MCP Server ──────────────────────────────────────────────

crashTrace("pre-mcp-server-construct");
const server = new McpServer({
  name: "minutes",
  version: MCP_SERVER_VERSION,
});
crashTrace("post-mcp-server-construct");

// ── Server instance registry ────────────────────────────────
// Every registration below is recorded as a builder so an identical surface can
// be materialized on a second McpServer. stdio keeps using the singleton above.
// HTTP needs one McpServer per session: `Protocol.connect()` throws on a second
// transport, and StreamableHTTPServerTransport refuses to be shared. Handlers
// close over module state, so instances share the CLI bridge and caches — only
// protocol state is per-instance.

/** A recorded registration. May return a teardown for per-instance state. */
type ServerBuilder = (target: McpServer) => (() => void) | void;

const SERVER_BUILDERS: ServerBuilder[] = [];

/** Record a registration and apply it to the singleton immediately. */
function forEachServer(build: ServerBuilder): void {
  SERVER_BUILDERS.push(build);
  build(server);
}

/**
 * Recorded equivalent of `server.resource(...)`. Typed as the McpServer method
 * so call sites keep their contextual handler types; the return value is not
 * usable (a registration now spans every instance) and no call site reads it.
 */
const registerResource = ((...args: any[]) => {
  forEachServer((target) => {
    (target as any).resource(...args);
  });
}) as unknown as McpServer["resource"];

/** Recorded equivalent of `registerAppResource(server, ...)`. */
function registerAppResourceOnEveryServer(...args: any[]): void {
  forEachServer((target) => {
    (registerAppResource as any)(target, ...args);
  });
}

export type ServerSurface = {
  tools: string[];
  resources: string[];
  resourceTemplates: string[];
};

/**
 * Everything registered on an McpServer, read from the SDK's registries. Lets
 * a factory-built instance be compared against the stdio singleton without
 * standing up a transport.
 */
export function describeServerSurface(target: McpServer): ServerSurface {
  const internals = target as any;
  return {
    tools: Object.keys(internals._registeredTools ?? {}).sort(),
    resources: Object.keys(internals._registeredResources ?? {}).sort(),
    resourceTemplates: Object.keys(
      internals._registeredResourceTemplates ?? {}
    ).sort(),
  };
}

/** The surface the stdio transport serves — the backward-compatibility baseline. */
export function describeStdioServerSurface(): ServerSurface {
  return describeServerSurface(server);
}

export type MinutesServerInstance = {
  server: McpServer;
  /** Stop per-instance background work (live-resource pollers). */
  dispose: () => void;
};

/**
 * Build a fresh McpServer exposing the same tools, resources, and request
 * handlers as the stdio singleton, by replaying every recorded registration in
 * declaration order. Used by the HTTP transport, one instance per session.
 */
export function createMinutesServer(): MinutesServerInstance {
  const instance = new McpServer({
    name: "minutes",
    version: MCP_SERVER_VERSION,
  });
  const teardowns: Array<() => void> = [];
  for (const build of SERVER_BUILDERS) {
    const teardown = build(instance);
    if (typeof teardown === "function") {
      teardowns.push(teardown);
    }
  }
  return {
    server: instance,
    dispose: () => {
      for (const teardown of teardowns) {
        try {
          teardown();
        } catch {
          // Teardown is best-effort; a failed poller stop must not block close.
        }
      }
    },
  };
}

// Declare MCP Apps extension support so hosts classify this server as interactive.
// The `extensions` field is part of the draft MCP spec (SEP-1724) — not yet in the
// stable SDK types, so we cast through `any`.
forEachServer((target) => {
  (target.server as any).registerCapabilities({
    extensions: { [EXTENSION_ID]: {} },
  } as any);
});

// Configurable directories — override via env vars in Claude Desktop extension settings
const MINUTES_HOME = canonicalizeRoot(
  expandHomeLikePath(process.env.MINUTES_HOME || join(homedir(), ".minutes"))
);

const MEETINGS_ROOT_ERROR = "The live meeting root could not be safely resolved.";

function parseMeetingsRootSnapshotValue(stdout: string): string {
  let parsed: unknown;
  try {
    parsed = JSON.parse(stdout);
  } catch {
    throw new Error(MEETINGS_ROOT_ERROR);
  }
  if (
    parsed === null ||
    typeof parsed !== "object" ||
    Object.keys(parsed).sort().join("\0") !== ["output_dir", "schema_version"].sort().join("\0") ||
    (parsed as any).schema_version !== 1 ||
    typeof (parsed as any).output_dir !== "string" ||
    (parsed as any).output_dir.trim() === "" ||
    (parsed as any).output_dir.includes("\0")
  ) {
    throw new Error(MEETINGS_ROOT_ERROR);
  }
  return (parsed as any).output_dir;
}

export function parseMeetingsRootSnapshot(stdout: string): string {
  return canonicalizeRoot(parseMeetingsRootSnapshotValue(stdout));
}

/** Resolve the live corpus for each operation; never retain a stale root. */
export async function getEffectiveMeetingsDir(
  runner: MinutesRunner = runMinutes,
  cliAvailability: () => Promise<boolean> = isCliAvailable,
  envOverride: string | undefined = process.env.MEETINGS_DIR
): Promise<string> {
  if (envOverride?.trim()) {
    return canonicalizeRoot(expandHomeLikePath(envOverride.trim()));
  }
  if (!(await cliAvailability())) {
    return canonicalizeRoot(join(homedir(), "meetings"));
  }
  try {
    const { stdout } = await runner(["meetings-root", "--json"]);
    return parseMeetingsRootSnapshot(stdout);
  } catch {
    throw new Error(MEETINGS_ROOT_ERROR);
  }
}

/**
 * Resolve only the lexical live-root value for the isolated process_audio
 * helper. Canonicalization and every filesystem syscall belong in that
 * bounded helper process, not in the long-lived MCP event loop.
 */
async function getEffectiveMeetingsDirForIsolatedAudio(
  runner: MinutesRunner = runMinutes,
  cliAvailability: () => Promise<boolean> = isCliAvailable,
  envOverride: string | undefined = process.env.MEETINGS_DIR
): Promise<string> {
  if (envOverride?.trim()) {
    return resolve(expandHomeLikePath(envOverride.trim()));
  }
  if (!(await cliAvailability())) {
    return join(homedir(), "meetings");
  }
  try {
    const { stdout } = await runner(["meetings-root", "--json"]);
    return resolve(expandHomeLikePath(parseMeetingsRootSnapshotValue(stdout)));
  } catch {
    throw new Error(MEETINGS_ROOT_ERROR);
  }
}

// ── UI Resource: MCP App dashboard ──────────────────────────

registerAppResourceOnEveryServer(
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

registerTool(
 "start_recording",
  "Start recording audio with call-aware preflight. When a known call app is active, Minutes can infer call intent and block silent mic-only call captures unless explicitly allowed. Note: this server does not listen to audio content. Recordings are stopped by invoking stop_recording after the user types a request in chat — never promise the user they can speak a 'stop recording' voice command.",
  {
    title: z.string().optional().describe("Optional title for this recording"),
    mode: z
      .enum(["meeting", "quick-thought"])
      .optional()
      .default("meeting")
      .describe("Live capture mode"),
    intent: z
      .enum(["memo", "room", "call"])
      .optional()
      .describe("Optional recording intent. If omitted and a known call app is active, Minutes may infer call intent."),
    allow_degraded: z
      .boolean()
      .optional()
      .default(false)
      .describe("Allow a mic-only capture to continue even if Minutes detects a call but no system-audio route is configured."),
    language: z.string().optional().describe("Transcription language code (e.g. 'en', 'ur', 'es', 'zh'). Overrides config.toml setting."),
    skip_audio_probe_reason: z
      .string()
      .min(1)
      .optional()
      .describe("Per-call reason to skip the system-audio readiness probe. This is not persisted and is written into recording_health."),
  },
  { title: "Start Recording", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async ({ title, mode, intent, allow_degraded, language, skip_audio_probe_reason }) => {
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

    const preflightArgs = ["preflight-record", "--json", "--mode", mode, "--intent", intent || "auto"];
    if (allow_degraded) preflightArgs.push("--allow-degraded");
    const { stdout: preflightOut } = await runMinutes(preflightArgs);
    const preflight = parseJsonOutput(preflightOut);

    // In extension mode, always delegate to the desktop app — the extension
    // runtime's audit session severs TCC mic grants for child processes.
    // For non-extension mode, still delegate call recordings to the desktop app
    // (it has system audio capture that the CLI can't do).
    if (isExtensionRuntime || preflight.intent === "call") {
      if (skip_audio_probe_reason) {
        return {
          content: [{
            type: "text" as const,
            text: "skip_audio_probe_reason cannot be honored for desktop-delegated recordings yet. Start the recording from the CLI with --skip-audio-probe \"<reason>\" if you intentionally want to bypass the system-audio readiness probe.",
          }],
          structuredContent: { preflight },
          isError: true,
        };
      }

      let response: DesktopControlResponse | null;
      try {
        response = await delegateRecordingToDesktop({
          title,
          mode,
          intent: intent || preflight.intent,
          allow_degraded,
          language,
        });
      } catch (e: any) {
        return {
          content: [{
            type: "text" as const,
            text: `Failed to delegate recording to the Minutes desktop app: ${e.message}\n\n` +
              "Check if Minutes.app is responding, or restart it and try again.",
          }],
          isError: true,
        };
      }
      if (response) {
        if (!response.accepted) {
          return {
            content: [{ type: "text" as const, text: response.detail }],
            structuredContent: { preflight, desktop_response: response },
          };
        }

        await new Promise((r) => setTimeout(r, 750));
        const { stdout: newStatus } = await runMinutes(["status"]);
        const result = parseJsonOutput(newStatus);
        let desktopLiveMsg = "";
        try {
          const { stdout: ltOut } = await runMinutes(["transcript", "--status", "--format", "json"], 5000);
          const ltStatus = parseJsonOutput(ltOut);
          if (ltStatus?.active) {
            desktopLiveMsg = " A live transcript is streaming — use read_live_transcript to follow along.";
          }
        } catch { /* sidecar may not have started yet */ }
        return {
          content: [
            {
              type: "text" as const,
              text: result.recording
                ? `Recording started in the running Minutes desktop app (PID: ${result.pid}).${Array.isArray(preflight.warnings) && preflight.warnings.length ? ` ${preflight.warnings[0]}` : ""}${desktopLiveMsg} When the user asks to finish (typed in chat), invoke stop_recording to process the transcript and summary. This server does not listen to audio content, so do not tell the user they can speak a stop command.`
                : response.detail,
            },
          ],
          structuredContent: { preflight, desktop_response: response },
        };
      }

      // Desktop app not running — in extension mode this means audio capture won't work.
      if (isExtensionRuntime) {
        return {
          content: [
            {
              type: "text" as const,
              text: "The Minutes desktop app is not running. The Claude Desktop extension " +
                "cannot capture audio directly (macOS blocks microphone access for " +
                "processes spawned from the extension runtime).\n\n" +
                "To fix: launch Minutes.app and try again. The extension will " +
                "delegate recording to the desktop app, which has its own " +
                "microphone permission.\n\n" +
                "Download: https://github.com/silverstein/minutes/releases/latest",
            },
          ],
          isError: true,
        };
      }
    }

    if (preflight.blocking_reason) {
      return {
        content: [
          {
            type: "text" as const,
            text: preflight.blocking_reason,
          },
        ],
        structuredContent: { preflight },
      };
    }

    // Spawn recording as a child process (not detached).
    // detached: true calls setsid() which creates a new macOS audit session,
    // severing the TCC microphone grant inherited from the host app (Claude Desktop).
    // CoreAudio then delivers all-zero samples — silent recordings.
    // The MCP server is long-lived, and the recording process ignores SIGTERM,
    // so child.unref() alone is sufficient.
    const args = ["record", "--mode", mode];
    if (title) args.push("--title", title);
    if (intent) args.push("--intent", intent);
    if (allow_degraded) args.push("--allow-degraded");
    if (skip_audio_probe_reason) args.push("--skip-audio-probe", skip_audio_probe_reason);
    if (language) args.push("--language", language);

    const child = spawn(MINUTES_BIN, args, {
      stdio: "ignore",
      env: mcpCliChildEnv({ RUST_LOG: "info" }),
    });
    child.unref();

    // Wait for PID file to appear
    await new Promise((r) => setTimeout(r, 1000));

    const { stdout: newStatus } = await runMinutes(["status"]);
    const result = parseJsonOutput(newStatus);

    // Check if the live transcript sidecar started (may still be loading the whisper model)
    let liveMsg = "";
    try {
      const { stdout: ltOut } = await runMinutes(["transcript", "--status", "--format", "json"], 5000);
      const ltStatus = parseJsonOutput(ltOut);
      if (ltStatus?.active) {
        liveMsg = " A live transcript is streaming — use read_live_transcript to follow along.";
      }
    } catch { /* sidecar may not have started yet — omit the message */ }

    return {
      content: [
        {
          type: "text" as const,
          text: result.recording
            ? `${result.recording_mode === "quick-thought" ? "Quick thought" : "Recording"} started (PID: ${result.pid}).${Array.isArray(preflight.warnings) && preflight.warnings.length ? ` ${preflight.warnings[0]}` : ""}${liveMsg} When the user asks to finish (typed in chat), invoke stop_recording to process the transcript and summary. This server does not listen to audio content, so do not tell the user they can speak a stop command.`
            : "Recording failed to start. Check `minutes logs` for details.",
        },
      ],
    };
  }
);

// ── Tool: stop_recording ────────────────────────────────────

export function verifiedStopRecordingSummary(snapshot: {
  path: string;
  meeting: {
    body?: string;
    frontmatter: {
      title?: unknown;
      duration?: unknown;
      people?: unknown;
      action_items?: unknown;
      decisions?: unknown;
    };
  };
}): string {
  const fm = snapshot.meeting.frontmatter;
  const title = typeof fm.title === "string" && fm.title.trim() ? fm.title : "Recording";
  const words = (snapshot.meeting.body ?? "").trim().split(/\s+/).filter(Boolean).length;
  let summary = `## ${title}\n\n**Saved:** ${snapshot.path}\n`;
  if (words > 0) summary += `**Words:** ${words}\n`;
  if (typeof fm.duration === "string" && fm.duration) {
    summary += `**Duration:** ${fm.duration}\n`;
  }
  if (Array.isArray(fm.people) && fm.people.length) {
    summary += `**People:** ${fm.people.map(String).join(", ")}\n`;
  }

  const actions = Array.isArray(fm.action_items)
    ? fm.action_items.filter((item: any) => item?.status === "open")
    : [];
  if (actions.length > 0) {
    summary += "\n### Action Items\n";
    for (const item of actions) {
      summary += `- [ ] ${String(item.task ?? "")}`;
      if (item.assignee) summary += ` (${String(item.assignee)})`;
      if (item.due) summary += ` — due ${String(item.due)}`;
      summary += "\n";
    }
  }

  if (Array.isArray(fm.decisions) && fm.decisions.length) {
    summary += "\n### Decisions\n";
    for (const decision of fm.decisions) {
      summary += `- ${String(decision?.text ?? "")}\n`;
    }
  }
  return summary;
}

registerTool(
  "stop_recording",
  "Stop the current recording and process it (transcribe, diarize, summarize).",
  {},
  { title: "Stop Recording", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async () => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }
    try {
      const stopped = await terminalControlBeforeContentReadiness(() =>
        runMinutes(["stop"], 180000)
      );
      if (!stopped.mayRevealContent) {
        return {
          content: [{
            type: "text" as const,
            text: "Recording stopped. The result is withheld until the agent trust boundary is ready.",
          }],
        };
      }
      const { stdout } = stopped.result;
      const result = parseJsonOutput(stdout);

      if (result.status === "queued") {
        return {
          content: [
            {
              type: "text" as const,
              text: "Recording stopped. Processing queued.",
            },
          ],
        };
      }

      if (!result.file) {
        return { content: [{ type: "text" as const, text: "Recording stopped." }] };
      }

      try {
        const meetingsRoot = await getEffectiveMeetingsDir();
        const snapshot = await policyVerifiedExactMeetingSnapshot(
          result.file,
          meetingsRoot,
          false
        );
        if (!snapshot) {
          return {
            content: [{ type: "text" as const, text: "Recording stopped. Processing finished, but the saved meeting is unavailable under the current privacy policy." }],
          };
        }

        const summary = verifiedStopRecordingSummary(snapshot);
        if (!(await policySnapshotsStillAuthorized(meetingsRoot, false, [snapshot]))) {
          return {
            content: [{ type: "text" as const, text: "Recording stopped. Processing finished, but the saved meeting is unavailable under the current privacy policy." }],
          };
        }

        // CLI fields are never surfaced; the response is derived only from
        // the exact live snapshot that survived final authorization.
        return { content: [{ type: "text" as const, text: summary }] };
      } catch {
        return {
          content: [{ type: "text" as const, text: "Recording stopped. Processing finished, but the saved meeting is unavailable under the current privacy policy." }],
        };
      }
    } catch {
      return {
        content: [{ type: "text" as const, text: "Stop failed. Check Minutes logs locally." }],
      };
    }
  }
);

// ── Tool: get_status ────────────────────────────────────────

registerTool(
  "get_status",
  "Check if a recording is currently in progress.",
  {},
  { title: "Recording Status", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async () => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: `No recording in progress (read-only mode).\n\n${CLI_INSTALL_MSG}` }] };
    }
    try {
      const { stdout } = await runMinutes(["status"]);
      return {
        content: [
          {
            type: "text" as const,
            text: buildPrivacySafeStatusText(JSON.parse(stdout)),
          },
        ],
      };
    } catch {
      return {
        content: [
          { type: "text" as const, text: "Recording status is unavailable." },
        ],
        isError: true,
      };
    }
  }
);

registerTool(
  "list_processing_jobs",
  "List background processing jobs for recent recordings, including queued, transcript-ready, needs-review, failed, and completed work.",
  {
    limit: z
      .number()
      .int()
      .min(1)
      .max(MCP_PROCESSING_JOB_RESULT_MAX)
      .optional()
      .default(10)
      .describe(`Maximum number of jobs (1-${MCP_PROCESSING_JOB_RESULT_MAX})`),
    include_completed: z.boolean().optional().default(true).describe("Include completed and failed jobs"),
  },
  { title: "Processing Jobs", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async ({ limit, include_completed }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }

    const args = ["jobs", "--json", "--limit", String(limit)];
    if (include_completed) args.push("--all");

    try {
      const { stdout } = await runMinutes(args);
      return buildPrivacySafeProcessingJobsResult(JSON.parse(stdout));
    } catch {
      return {
        content: [
          {
            type: "text" as const,
            text: "Processing jobs could not be safely read.",
          },
        ],
        isError: true,
      };
    }
  }
);

// ── Tool: list_meetings ─────────────────────────────────────

registerDocsAppTool(
  "list_meetings",
  {
    description: "List recent meetings and voice memos. Restricted meetings are excluded. An override requires both an operator launch grant and include_restricted=true, and is durably audited.",
    inputSchema: {
      limit: z
        .number()
        .int()
        .min(1)
        .max(MCP_MEETING_RESULT_MAX)
        .optional()
        .default(10)
        .describe(`Maximum results (1-${MCP_MEETING_RESULT_MAX})`),
      type: z.enum(["meeting", "memo"]).optional().describe("Filter by type"),
      include_restricted: z
        .boolean()
        .optional()
        .default(false)
        .describe("Include restricted meetings only when a human launched the server with MINUTES_MCP_RESTRICTED_POLICY=logged-override; every request is durably audited"),
    },
    annotations: { title: "List Meetings", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ limit, type: contentType, include_restricted }) => {
    const boundedLimit = normalizeMcpMeetingResultLimit(limit);
    // Agent-facing meeting content always comes from strict live snapshots.
    // The CLI/search indexes remain operator surfaces, never authorization
    // sources for MCP responses.
    const meetingsDir = await getEffectiveMeetingsDir();
    const meetings = await policyListMeetings(
      meetingsDir,
      MCP_POLICY_MEETING_RESULT_MAX,
      include_restricted
    );
      const limited: PolicyVerifiedMeeting[] = [];
      for (const meeting of meetings) {
        if (contentType && meeting.frontmatter.type !== contentType) continue;
        limited.push(meeting);
        if (limited.length >= boundedLimit) break;
      }
      const openActions = openActionsFromMeetings(meetings, MCP_ACTION_RESULT_MAX);

      if (limited.length === 0) {
        return {
          content: [{ type: "text" as const, text: "No meetings or memos found." }],
          structuredContent: { meetings: [], actions: [], view: "dashboard" },
          _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "dashboard" },
        };
      }

      const meetingsJson = limited.map(meetingListItem);
      const text = meetingsJson
        .map((m) => `${m.date} — ${m.title} [${m.content_type}]\n  ${m.path}`)
        .join("\n\n");

    return {
      content: [{ type: "text" as const, text: boundedMcpText(text) }],
      structuredContent: { meetings: meetingsJson, actions: openActions.map((a) => boundedActionItem(a.item)), view: "dashboard" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "dashboard" },
    };
  }
);

// ── Tool: search_meetings ───────────────────────────────────

registerDocsAppTool(
  "search_meetings",
  {
    description: "Search meeting transcripts and voice memos. Restricted meetings are excluded. An override requires both an operator launch grant and include_restricted=true, and is durably audited.",
    inputSchema: {
      query: z.string().max(MCP_QUERY_MAX_CHARS).describe("Text to search for"),
      type: z.enum(["meeting", "memo"]).optional().describe("Filter by type"),
      since: z.string().optional().describe("Only results after this date (ISO)"),
      limit: z
        .number()
        .int()
        .min(1)
        .max(MCP_MEETING_RESULT_MAX)
        .optional()
        .default(10)
        .describe(`Maximum results (1-${MCP_MEETING_RESULT_MAX})`),
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
      include_restricted: z
        .boolean()
        .optional()
        .default(false)
        .describe("Include restricted meetings only when a human launched the server with MINUTES_MCP_RESTRICTED_POLICY=logged-override; every request is durably audited"),
    },
    annotations: { title: "Search Meetings", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ query, type: contentType, since, limit, intent_kind, owner, intents_only, include_restricted }) => {
    const boundedLimit = normalizeMcpMeetingResultLimit(limit);
    // Search indexes and CLI output provide no authorization guarantee. Scan
    // strict live snapshots here and derive every returned field from those
    // bytes. This also makes metadata/intent filters fail closed instead of
    // routing around the policy boundary.
    const meetingsDir = await getEffectiveMeetingsDir();
    const intentMode = intents_only || !!intent_kind || !!owner;
    const meetings = await policyToolSearchMeetings(
      meetingsDir,
      include_restricted,
      {
        query,
        contentType,
        since,
        intentKind: intent_kind,
        owner,
        intentsOnly: intents_only,
      }
    );

      let results: Array<PolicyIntentResult | ReturnType<typeof meetingSearchItem> & { snippet: string }>;
      if (intentMode) {
        results = policyIntentResults(
          meetings,
          query,
          intent_kind,
          owner,
          boundedLimit
        );
      } else {
        const matches: Array<ReturnType<typeof meetingSearchItem> & { snippet: string }> = [];
        for (const meeting of meetings) {
            matches.push({
              ...meetingSearchItem(meeting),
              snippet: liveMeetingSnippet(meeting.body, query),
            });
            if (matches.length >= boundedLimit) break;
        }
        results = matches;
      }

      if (results.length === 0) {
        return {
          content: [{ type: "text" as const, text: boundedMcpText(`No results for "${query}".`) }],
          structuredContent: { results: [], view: "search" },
          _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "search" },
        };
      }

      const text = intentMode
        ? results
            .map(
              (result: any) =>
                `${result.date} — ${result.title} [${result.content_type}]\n  ${result.kind}: ${result.what}${result.who ? ` (@${result.who})` : ""}${result.by_date ? ` by ${result.by_date}` : ""}\n  ${result.path}`
            )
            .join("\n\n")
        : results
            .map(
              (result: any) =>
                `${result.date} — ${result.title} [${result.content_type}]\n  ${result.snippet}\n  ${result.path}`
            )
            .join("\n\n");

    return {
      content: [{ type: "text" as const, text: boundedMcpText(text) }],
      structuredContent: {
        results,
        view: "search",
      },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "search" },
    };
  }
);

// ── Tool: activity_summary ──────────────────────────────────
// Feature-gated (#183 phase 2). Hidden when an already-installed CLI does not
// report activity_summary support. If the CLI is missing at boot, the tool
// stays visible so first-run auto-install can still make it usable without a
// server restart.

if (hasFeature(CLI_CAPABILITIES, "activity_summary"))
registerDocsAppTool(
  "activity_summary",
  {
    description: "Summarize meeting-adjacent desktop context bound to one exact normal meeting source.",
    inputSchema: {
      path: z.string().describe("Exact normal meeting Markdown path linked to the context session"),
    },
    annotations: { title: "Activity Summary", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ path }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: `Desktop-context summaries require the full CLI.\n\n${CLI_INSTALL_MSG}` }] };
    }

    const meetingsDir = await getEffectiveMeetingsDir();
    const parsed = await withPolicyBoundContextPath(
      path,
      meetingsDir,
      async (canonicalPath, timeoutMs, signal) => {
        const { stdout } = await runMinutes([
          "context",
          "activity-summary",
          "--json",
          "--path",
          canonicalPath,
        ], timeoutMs, signal);
        const value = parseJsonOutput(stdout);
        if (!value || typeof value !== "object") {
          throw new Error("Desktop context summary could not be safely read.");
        }
        return value as Record<string, unknown>;
      },
      (value, sessionId, source) => {
        assertContextSession(value, sessionId);
        assertContextItemsSession((value as any).events, sessionId, "Desktop context events");
        assertContextItemsSession((value as any).links, sessionId, "Desktop context links");
        return withoutContextSourceAuthorization({
          ...value,
          links: assistantSafeContextLinks((value as any).links, source.path),
        });
      }
    );

    const apps = Array.isArray((parsed as any).top_apps) ? (parsed as any).top_apps : [];
    const windows = Array.isArray((parsed as any).top_windows) ? (parsed as any).top_windows : [];
    const events = Array.isArray((parsed as any).events) ? (parsed as any).events : [];
    const lines = [
      `Desktop context summary: ${(parsed as any).window?.start || "?"} -> ${(parsed as any).window?.end || "?"}`,
      apps.length ? `Top apps: ${apps.map((entry: any) => `${entry.name} (${entry.count})`).join(", ")}` : "",
      windows.length ? `Top windows: ${windows.map((entry: any) => `${entry.name} (${entry.count})`).join(", ")}` : "",
      events.length ? `Events: ${events.length}` : "Events: 0",
    ].filter(Boolean);

    return {
      content: [{ type: "text" as const, text: lines.join("\n") }],
      structuredContent: { ...(parsed as any), kind: "activity_summary", view: "context" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "context", kind: "activity_summary" },
    };
  }
);

// ── Tool: search_context ────────────────────────────────────
// Feature-gated (#183 phase 2). See activity_summary comment above.

if (hasFeature(CLI_CAPABILITIES, "search_context"))
registerDocsAppTool(
  "search_context",
  {
    description: "Search desktop-context events bound to one exact normal meeting source.",
    inputSchema: {
      path: z.string().describe("Exact normal meeting Markdown path linked to the context session"),
      query: z.string().describe("Text query for app names, bundle ids, or captured window titles"),
      limit: z.number().optional().default(20).describe("Maximum results"),
    },
    annotations: { title: "Search Context", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ path, query, limit }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: `Desktop-context search requires the full CLI.\n\n${CLI_INSTALL_MSG}` }] };
    }

    const meetingsDir = await getEffectiveMeetingsDir();
    const parsed = await withPolicyBoundContextPath(
      path,
      meetingsDir,
      async (canonicalPath, timeoutMs, signal) => {
        const { stdout } = await runMinutes([
          "context",
          "search",
          query,
          "--path",
          canonicalPath,
          "--limit",
          String(limit),
          "--json",
        ], timeoutMs, signal);
        const value = parseJsonOutput(stdout);
        if (!value || typeof value !== "object") {
          throw new Error("Desktop context search could not be safely read.");
        }
        return value as Record<string, unknown>;
      },
      (value, sessionId) => {
        assertContextItemsSession((value as any).results, sessionId, "Desktop context search results");
        return withoutContextSourceAuthorization(value);
      }
    );

    const results = Array.isArray((parsed as any).results) ? (parsed as any).results : [];
    const text = results.length === 0
      ? `No desktop-context events found for "${query}".`
      : results
          .map(
            (event: any) =>
              `${event.observed_at} — ${event.app_name || event.bundle_id || "unknown"}${event.window_title ? ` :: ${event.window_title}` : ""}`
          )
          .join("\n");

    return {
      content: [{ type: "text" as const, text }],
      structuredContent: { query, results, view: "context", kind: "search_context" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "context", kind: "search_context" },
    };
  }
);

// ── Tool: get_moment ────────────────────────────────────────
// Feature-gated (#183 phase 2). See activity_summary comment above.

if (hasFeature(CLI_CAPABILITIES, "get_moment"))
registerDocsAppTool(
  "get_moment",
  {
    description: "Show the local rewind bound to one exact normal meeting source.",
    inputSchema: {
      path: z.string().describe("Exact normal meeting Markdown path linked to the context session"),
      at: z.string().optional().describe("Explicit anchor timestamp (RFC3339)"),
      before_minutes: z.number().optional().default(10).describe("Minutes before the anchor"),
      after_minutes: z.number().optional().default(10).describe("Minutes after the anchor"),
    },
    annotations: { title: "Get Moment", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ path, at, before_minutes, after_minutes }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: `Desktop-context rewind requires the full CLI.\n\n${CLI_INSTALL_MSG}` }] };
    }

    const meetingsDir = await getEffectiveMeetingsDir();
    const parsed = await withPolicyBoundContextPath(
      path,
      meetingsDir,
      async (canonicalPath, timeoutMs, signal) => {
        const args = [
          "context",
          "get-moment",
          "--json",
          "--path",
          canonicalPath,
          "--before-minutes",
          String(before_minutes),
          "--after-minutes",
          String(after_minutes),
        ];
        if (at) args.push("--at", at);
        const { stdout } = await runMinutes(args, timeoutMs, signal);
        const value = parseJsonOutput(stdout);
        if (!value || typeof value !== "object") {
          throw new Error("Desktop context moment could not be safely read.");
        }
        return value as Record<string, unknown>;
      },
      (value, sessionId, source) => {
        assertContextSession(value, sessionId);
        assertContextItemsSession((value as any).events, sessionId, "Desktop context events");
        assertContextItemsSession((value as any).links, sessionId, "Desktop context links");
        return withoutContextSourceAuthorization({
          ...value,
          links: assistantSafeContextLinks((value as any).links, source.path),
        });
      }
    );

    const events = Array.isArray((parsed as any).events) ? (parsed as any).events : [];
    const text = [
      `Moment window: ${(parsed as any).window?.start || "?"} -> ${(parsed as any).window?.end || "?"}`,
      ...events.map(
        (event: any) =>
          `${event.observed_at} — ${event.app_name || event.bundle_id || "unknown"}${event.window_title ? ` :: ${event.window_title}` : ""}`
      ),
    ].join("\n");

    return {
      content: [{ type: "text" as const, text }],
      structuredContent: { ...(parsed as any), view: "context", kind: "get_moment" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "context", kind: "get_moment" },
    };
  }
);

// ── Tool: consistency_report ───────────────────────────────

// ── Tool: get_screen_context ───────────────────────────────
// Direct image content is returned only for paths that the CLI resolved from
// ScreenshotRef events and that this process independently canonicalizes under
// ~/.minutes/screens. This is intentionally not a generic local-file tool.

const MAX_SCREEN_CONTEXT_IMAGE_BYTES = 10 * 1024 * 1024;
const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

export async function readVerifiedScreenImage(
  imagePath: string,
  expectedByteSize: number,
  expectedSha256: string,
  screenRoot = join(homedir(), ".minutes", "screens"),
  hooks: BoundReadHooks = {}
): Promise<Buffer> {
  if (
    !Number.isSafeInteger(expectedByteSize) ||
    expectedByteSize <= 0 ||
    expectedByteSize > MAX_SCREEN_CONTEXT_IMAGE_BYTES
  ) {
    throw new Error("Screen-context image has an invalid capture-time byte bound");
  }
  if (!/^[0-9a-f]{64}$/.test(expectedSha256)) {
    throw new Error("Screen-context image has an invalid capture-time digest");
  }
  const resolved = validatePathInDirectory(imagePath, screenRoot, [".png"]);
  const bytes = await readTextFileFromBoundParent(resolved, {
    ...hooks,
    maxBytes: MAX_SCREEN_CONTEXT_IMAGE_BYTES,
  });
  if (bytes.length > MAX_SCREEN_CONTEXT_IMAGE_BYTES) {
    throw new Error("Screen-context image exceeds the 10 MiB delivery limit");
  }
  if (bytes.length < PNG_SIGNATURE.length || !bytes.subarray(0, PNG_SIGNATURE.length).equals(PNG_SIGNATURE)) {
    throw new Error("Screen-context image is not a verified PNG");
  }
  if (bytes.length !== expectedByteSize) {
    throw new Error("Screen-context image no longer matches its capture-time byte bound");
  }
  const actualSha256 = createHash("sha256").update(bytes).digest("hex");
  if (actualSha256 !== expectedSha256) {
    throw new Error("Screen-context image no longer matches its capture-time digest");
  }
  return bytes;
}

if (hasFeature(CLI_CAPABILITIES, "screen_context"))
registerDocsAppTool(
  "get_screen_context",
  {
    description: "Retrieve up to three verified PNG screenshots bound to one exact normal meeting source, optionally nearest a timestamp.",
    inputSchema: {
      path: z.string().describe("Exact normal meeting Markdown path linked to the context session"),
      at: z.string().optional().describe("Nearest-image anchor timestamp (RFC3339)"),
      limit: z.number().int().min(1).max(3).optional().default(1).describe("Maximum verified images (1-3)"),
    },
    annotations: { title: "Get Screen Context", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ path, at, limit }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: `Screen-context retrieval requires the full CLI.\n\n${CLI_INSTALL_MSG}` }] };
    }

    const meetingsDir = await getEffectiveMeetingsDir();
    const screenRoot = join(homedir(), ".minutes", "screens");
    const { parsed, verifiedImages } = await withPolicyBoundContextPath(
      path,
      meetingsDir,
      async (canonicalPath, timeoutMs, signal) => {
        const args = [
          "context",
          "screen",
          "--json",
          "--limit",
          String(limit),
          "--path",
          canonicalPath,
        ];
        if (at) args.push("--at", at);
        const { stdout } = await runMinutes(args, timeoutMs, signal);
        const value = parseJsonOutput(stdout);
        if (!value || typeof value !== "object") {
          throw new Error("Screen context could not be safely read.");
        }
        return value as Record<string, unknown>;
      },
      async (value, sessionId, _source, signal) => {
        assertContextSession(value, sessionId);
        const status = (value as any).status || {};
        if (
          status.context_session_id !== undefined &&
          status.context_session_id !== sessionId
        ) {
          throw new Error("Screen context status escaped its authorized session.");
        }
        const images = Array.isArray((value as any).images)
          ? (value as any).images.slice(0, 3)
          : [];
        const verifiedImages: Buffer[] = [];
        for (const image of images) {
          if (signal.aborted) {
            throw new Error("Screen-context authorization deadline elapsed");
          }
          if (
            !image ||
            typeof image.path !== "string" ||
            typeof image.byte_size !== "number" ||
            typeof image.sha256 !== "string"
          ) {
            throw new Error("Screen-context image is missing its capture-time attestation");
          }
          verifiedImages.push(
            await readVerifiedScreenImage(
              image.path,
              image.byte_size,
              image.sha256,
              screenRoot,
              { signal, timeoutMs: 10_000 }
            )
          );
        }
        return {
          parsed: withoutContextSourceAuthorization(value),
          verifiedImages,
        };
      }
    );

    const status = (parsed as any).status || {};
    const reason = typeof (parsed as any).reason === "string" ? (parsed as any).reason : "";
    const text = [
      `Screen context state: ${status.state || "unknown"}`,
      `Verified images delivered: ${verifiedImages.length}`,
      reason,
      "An image must be inspected before making any visual claim; app/window metadata alone is not sight.",
    ].filter(Boolean).join("\n");

    const content: Array<
      | { type: "text"; text: string }
      | { type: "image"; data: string; mimeType: string }
    > = [{ type: "text", text }];
    for (const bytes of verifiedImages) {
      content.push({
        type: "image",
        data: bytes.toString("base64"),
        mimeType: "image/png",
      });
    }

    return {
      content,
      structuredContent: { ...(parsed as any), view: "context", kind: "get_screen_context" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "context", kind: "get_screen_context" },
    };
  }
);

// ── Tool: consistency_report ───────────────────────────────

registerDocsAppTool(
  "consistency_report",
  {
    description: "Flag conflicting decisions and stale commitments across meetings using structured intent data. Meetings designated `sensitivity: restricted` are always excluded from this report.",
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

registerDocsAppTool(
  "get_person_profile",
  {
    description: "Get a relationship profile derived from live, policy-authorized meeting snapshots within the supported corpus bounds. Restricted meetings are excluded unless an operator launch grant plus include_restricted=true is durably audited.",
    inputSchema: {
      name: z.string().trim().min(1).max(MCP_QUERY_MAX_CHARS).describe("Person / attendee name to profile"),
      include_restricted: z
        .boolean()
        .optional()
        .default(false)
        .describe("Include restricted meetings only when a human launched the server with MINUTES_MCP_RESTRICTED_POLICY=logged-override; every request is durably audited"),
    },
    annotations: { title: "Person Profile", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ name, include_restricted }) => {
    if (!include_restricted) {
      const capabilities = await ensureCliCapabilities([
        "person_profile_policy_fresh_v1",
        "policy_projection_worker_v1",
      ]);
      if (
        capabilities.kind !== "report" ||
        !hasFeature(capabilities, "person_profile_policy_fresh_v1") ||
        !hasFeature(capabilities, "policy_projection_worker_v1")
      ) {
        throw new Error(
          "Person profiles require a Minutes CLI with policy-fresh correction-aware identity resolution. Update Minutes, then try again."
        );
      }
      const { stdout } = await runPolicyGraphMinutes(["person", name]);
      const profile = boundedCorePersonProfile(parseJsonOutput(stdout));
      const sections = [];
      if (profile.topicCounts.length > 0) {
        sections.push(
          "Topics: " + profile.topicCounts.map((topic) => `${topic.topic} (${topic.count})`).join(", ")
        );
      }
      if (profile.meetings.length > 0) {
        sections.push("Meetings:\n" + profile.meetings.map((meeting) => `- ${meeting.date} — ${meeting.title}`).join("\n"));
      }
      if (profile.openIntents.length > 0) {
        sections.push("Open commitments:\n" + profile.openIntents.map((intent) => `- ${intent.what} (${intent.status})`).join("\n"));
      }
      if (profile.recentDecisions.length > 0) {
        sections.push("Recent decisions:\n" + profile.recentDecisions.map((decision) => `- ${decision.what} (${decision.date})`).join("\n"));
      }
      const text = sections.length > 0
        ? sections.join("\n\n")
        : `No profile data found for ${profile.name}.`;
      return {
        content: [{ type: "text" as const, text: boundedMcpText(text) }],
        structuredContent: {
          name: profile.name,
          top_topics: profile.topicCounts,
          open_intents: profile.openIntents,
          recent_decisions: profile.recentDecisions,
          recent_meetings: profile.meetings,
          view: "person",
        },
        _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "person" },
      };
    }
    // Profile every field from strict live snapshots. Cached graph and legacy
    // CLI profile output remain operator conveniences, not authorization
    // sources for an agent-facing response.
    const meetingsDir = await getEffectiveMeetingsDir();
    const meetings = await policyListMeetings(
        meetingsDir,
        MCP_POLICY_MEETING_RESULT_MAX,
        include_restricted
    );
    const profile = personProfileFromMeetings(meetings, name);
    const sections = [];
    if (profile.topics.length > 0) sections.push("Topics: " + profile.topics.join(", "));
    if (profile.meetings.length > 0) sections.push("Meetings:\n" + profile.meetings.map((m) => `- ${m.date} — ${m.title}`).join("\n"));
    if (profile.openActions.length > 0) sections.push("Open commitments:\n" + profile.openActions.map((a) => `- ${a.what} (${a.status})`).join("\n"));
    if (profile.recentDecisions.length > 0) sections.push("Recent decisions:\n" + profile.recentDecisions.map((decision) => `- ${decision.what} (${decision.date})`).join("\n"));
    const text = sections.length > 0 ? sections.join("\n\n") : `No profile data found for ${profile.name}.`;
    return {
      content: [{ type: "text" as const, text: boundedMcpText(text) }],
      structuredContent: { name: profile.name, top_topics: profile.topicCounts, open_intents: profile.openActions, recent_decisions: profile.recentDecisions, recent_meetings: profile.meetings, view: "person" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "person" },
    };
  }
);

// ── Tool: research_topic ────────────────────────────────────

registerTool(
  "research_topic",
  "Research a topic across policy-authorized meetings within the supported corpus bounds. Restricted meetings are excluded. An override requires an operator launch grant plus include_restricted=true and is durably audited.",
  {
    query: z.string().max(MCP_QUERY_MAX_CHARS).describe("Topic or question to investigate across meetings"),
    type: z.enum(["meeting", "memo"]).optional().describe("Filter by type"),
    since: z.string().optional().describe("Only results after this date (ISO)"),
    attendee: z.string().optional().describe("Filter by attendee / person"),
    include_restricted: z
      .boolean()
      .optional()
      .default(false)
      .describe("Include restricted meetings only when a human launched the server with MINUTES_MCP_RESTRICTED_POLICY=logged-override; every request is durably audited"),
  },
  { title: "Research Topic", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async ({ query, type: contentType, since, attendee, include_restricted }) => {
    const needle = query.trim().toLowerCase();
    const meetingsDir = await getEffectiveMeetingsDir();
    const sourceMeetings = await policyListMeetings(
        meetingsDir,
        MCP_POLICY_MEETING_RESULT_MAX,
        include_restricted
      );
    const meetings: PolicyVerifiedMeeting[] = [];
    if (needle.length > 0) {
      for (const meeting of sourceMeetings) {
        if (
          (contentType && meeting.frontmatter.type !== contentType) ||
          !meetingMatchesSince(meeting, since) ||
          !meetingMatchesPerson(meeting, attendee) ||
          (!meeting.frontmatter.title.toLowerCase().includes(needle) &&
            !meeting.body.toLowerCase().includes(needle))
        ) {
          continue;
        }
        meetings.push(meeting);
        if (meetings.length >= MCP_RESEARCH_MEETING_RESULT_MAX) break;
      }
    }
    const projection = researchTopicProjection(meetings, query);
    return { content: [{ type: "text" as const, text: projection.text }] };
  }
);

// ── Tool: get_meeting ───────────────────────────────────────

export function restrictedMeetingStubResult(meeting: PolicyVerifiedMeeting) {
  const stubText = [
    `${meeting.frontmatter.title}`,
    `date: ${meeting.frontmatter.date}`,
    "sensitivity: restricted",
    "",
    "Content excluded by default: this meeting is designated `sensitivity: restricted`.",
    "A human operator must launch the MCP server with MINUTES_MCP_RESTRICTED_POLICY=logged-override, then pass include_restricted: true. The request is durably audited.",
  ].join("\n");
  return {
    content: [{ type: "text" as const, text: stubText }],
    structuredContent: {
      title: meeting.frontmatter.title,
      date: meeting.frontmatter.date,
      type: meeting.frontmatter.type,
      sensitivity: "restricted",
      restricted_stub: true,
      view: "detail",
    },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "detail" },
  };
}

registerDocsAppTool(
  "get_meeting",
  {
    description: "Get a full meeting transcript with speaker overlays. A restricted meeting returns a stub. Full access requires an operator launch grant plus include_restricted=true and is durably audited.",
    inputSchema: {
      path: z.string().describe("Path to the meeting markdown file"),
      include_restricted: z
        .boolean()
        .optional()
        .default(false)
        .describe("Return restricted content only when a human launched the server with MINUTES_MCP_RESTRICTED_POLICY=logged-override; every request is durably audited"),
    },
    annotations: { title: "View Meeting", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ path: filePath, include_restricted }) => {
    try {
      const meetingsDir = await getEffectiveMeetingsDir();
      const { path: resolved, content: rawContent } = await readTextFileInDirectory(
        filePath,
        meetingsDir,
        [".md"]
      );
      if (!isActiveCorpusMeetingPath(resolved, meetingsDir)) {
        throw new Error("Meeting is outside the active corpus.");
      }

      // Sensitivity enforcement (consent layer Wave 2): a restricted meeting
      // fetched by exact path returns a minimal stub — title, date, and the
      // designation — never the transcript, unless the caller overrides.
      const sensitivityParsed = parsePolicyVerifiedMeeting(rawContent, resolved);
      if (!sensitivityParsed) {
        throw new Error(
          "Meeting frontmatter is missing, malformed, or has an unsupported sensitivity value."
        );
      }
      if (sensitivityParsed && meetingSensitivity(sensitivityParsed) === "restricted") {
        if (!include_restricted) {
          return restrictedMeetingStubResult(sensitivityParsed);
        }
        // The central registration wrapper already appended the durable audit
        // record before this handler ran. Do not duplicate the path on stderr.
      }

      // Ask the CLI for an overlay-applied structured view. Raw markdown on
      // disk is never mutated — the CLI just layers ~/.minutes/overlays.db on
      // top of the parsed frontmatter. If the CLI is unavailable or the call
      // fails, degrade gracefully to raw content.
      //
      // structuredContent mirrors what is on disk: the transcript body plus the
      // synthesized fields (summary, action_items, decisions, intents). The raw
      // markdown still rides along in content[0].text, but structured-content
      // consumers and MCP-App hosts that surface structuredContent over the text
      // block must not be left with an envelope only (issue #255).
      const rawParsed = sensitivityParsed;
      let structured = meetingDetailPayload({
        path: resolved,
        speaker_map: (rawParsed?.frontmatter as any)?.speaker_map ?? [],
        recording_health: (rawParsed?.frontmatter as any)?.recording_health,
        overlay_applied: false,
        title: (rawParsed?.frontmatter as any)?.title,
        summary: extractMarkdownSection(rawParsed?.body, "Summary"),
        action_items: (rawParsed?.frontmatter as any)?.action_items ?? [],
        decisions: (rawParsed?.frontmatter as any)?.decisions ?? [],
        intents: (rawParsed?.frontmatter as any)?.intents ?? [],
        body: rawParsed?.body ?? rawContent,
      });

      const restrictedSource =
        meetingSensitivity(sensitivityParsed) === "restricted";
      if (!restrictedSource && await isCliAvailable()) {
        try {
          const { stdout } = await runMinutes(["get", resolved, "--json"], 10000);
          const parsed = parseJsonOutput(stdout);
          const verifiedOverlay = verifiedCliSpeakerOverlay(parsed, rawContent);
          if (verifiedOverlay) {
            structured = {
              ...structured,
              ...verifiedOverlay,
            };
          }
        } catch {
          // Non-fatal: fall through to raw content with no speaker_map enrichment.
        }
      }

      let structuredOut: Record<string, unknown> = structured;
      if (include_restricted && meetingSensitivity(sensitivityParsed) === "restricted") {
        structuredOut = {
          ...structured,
          sensitivity: "restricted",
          sensitivity_override: { applied: true, logged: "durable-jsonl" },
        };
      }

      // The CLI overlay read above is a second path-based operation. Recheck
      // the exact live bytes before returning so a concurrent sensitivity
      // change cannot swap restricted content into this already-authorized
      // response. On any change the caller retries from a fresh snapshot.
      const { content: finalContent } = await readTextFileInDirectory(
        resolved,
        meetingsDir,
        [".md"]
      );
      if (!isActiveCorpusMeetingPath(resolved, meetingsDir) || finalContent !== rawContent) {
        throw new Error("Meeting changed while its sensitivity was being verified; retry.");
      }

      return {
        content: [{ type: "text" as const, text: rawContent }],
        structuredContent: structuredOut,
        _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "detail", path: resolved },
      };
    } catch {
      return {
        content: [{ type: "text" as const, text: "Meeting could not be safely read." }],
        isError: true,
      };
    }
  }
);

// ── Tool: process_audio ─────────────────────────────────────

export function validateMcpProcessAudioInput(
  filePath: string,
  allowedDirs: string[],
  meetingsDir: string,
  audioExts: string[]
): string {
  const resolved = validatePathInDirectories(filePath, allowedDirs, audioExts);
  if (isPathWithinCanonicalRoot(resolved, meetingsDir)) {
    throw new Error(
      "Retained meeting audio cannot be reprocessed through an agent surface."
    );
  }
  return resolved;
}

type MeetingsRootAttestation = {
  canonicalPath: string;
  identity: string;
};

export type EffectiveMeetingsRootResolver = () => Promise<string>;

export type AudioDigest = {
  byteLength: number;
};

export type AuthorizedMcpProcessAudioInput = {
  /** Retained read-only source capability. Child stdio maps this exact fd to 3. */
  fd: number;
  digest: AudioDigest;
  format: string;
  /** Sanitized title; the operation never receives a caller-controlled path. */
  safeTitle: string;
};

export type McpProcessAudioAuthorizationHooks = {
  /** Test/diagnostic byte ceiling; production is always capped at 2 GiB. */
  maxBytes?: number;
  /** Test/diagnostic aggregate retained-input ceiling. */
  maxAggregateBytes?: number;
  /** Test/diagnostic hash/authorization deadline. */
  timeoutMs?: number;
  /** Test-only monotonic clock override. */
  nowMs?: () => number;
  /** Test-only race injection after the source fd has been retained. */
  afterValidation?: () => void | Promise<void>;
  /** Test-only observation used to prove fd closure on every branch. */
  onRetainedFd?: (fd: number) => void;
  /** Test-only race injection after the exact fd length has been declared. */
  afterHash?: (digest: AudioDigest) => void | Promise<void>;
  /** Test-only race injection before the final path/config attestation. */
  beforeFinalAttestation?: () => void | Promise<void>;
  /** Test-only simulation of a helper that never publishes `close`. */
  ignoreHelperCloseForTest?: boolean;
};

type ProcessAudioBudget = {
  maxBytes: number;
  maxAggregateBytes: number;
  deadlineMs: number;
  nowMs: () => number;
};

export const MCP_PROCESS_AUDIO_MAX_BYTES = 2 * 1024 * 1024 * 1024;
export const MCP_PROCESS_AUDIO_AUTHORIZATION_TIMEOUT_MS = 120_000;
export const MCP_PROCESS_AUDIO_MAX_ACTIVE_JOBS = 2;
export const MCP_PROCESS_AUDIO_MAX_AGGREGATE_BYTES =
  4 * 1024 * 1024 * 1024;
export const MCP_PROCESS_AUDIO_CLI_TIMEOUT_MS = 300_000;
export const MCP_PROCESS_AUDIO_MAX_STDOUT_BYTES = 1024 * 1024;
export const MCP_PROCESS_AUDIO_MAX_STDERR_BYTES = 1024 * 1024;
const MCP_AUDIO_BUDGET_ERROR =
  "Access denied: process_audio resource budget exceeded";

let activeProcessAudioJobs = 0;
let reservedProcessAudioBytes = 0;
let processAudioIsolationPoisoned = false;

/**
 * Bind the live meetings root to both its canonical location and directory
 * identity. A missing first-run root is bound to its nearest existing
 * canonical ancestor plus the exact missing suffix.
 */
function attestMeetingsRoot(root: string): MeetingsRootAttestation {
  let cursor = resolve(expandHomeLikePath(root));
  const missing: string[] = [];
  while (!existsSync(cursor)) {
    const parent = dirname(cursor);
    if (parent === cursor) throw new Error(MEETINGS_ROOT_ERROR);
    missing.unshift(basename(cursor));
    cursor = parent;
  }

  const canonicalAncestor = realpathSync(cursor);
  const ancestor = statSync(canonicalAncestor, { bigint: true });
  if (!ancestor.isDirectory()) throw new Error(MEETINGS_ROOT_ERROR);
  return {
    canonicalPath:
      missing.length === 0
        ? canonicalAncestor
        : join(canonicalAncestor, ...missing),
    identity:
      (missing.length === 0 ? "present:" : "absent:") +
      String(ancestor.dev) +
      ":" +
      String(ancestor.ino) +
      ":" +
      missing.join("/"),
  };
}

function sameMeetingsRoot(
  initial: MeetingsRootAttestation,
  final: MeetingsRootAttestation
): boolean {
  return (
    initial.canonicalPath === final.canonicalPath &&
    initial.identity === final.identity
  );
}

function expectationsMatch(
  left: BoundReadExpectation,
  right: BoundReadExpectation
): boolean {
  return (
    left.parentIdentity === right.parentIdentity &&
    left.leafFingerprint === right.leafFingerprint
  );
}

function requirePathExpectation(
  path: string,
  expected: BoundReadExpectation
): void {
  if (!expectationsMatch(captureBoundReadExpectation(path), expected)) {
    throw new Error("Access denied: authorized audio identity changed");
  }
}

function openBoundSingleLinkAudio(
  path: string,
  expected: BoundReadExpectation
): number {
  requirePathExpectation(path, expected);
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const info = fstatSync(fd, { bigint: true });
    if (
      !info.isFile() ||
      info.nlink !== 1n ||
      boundReadFingerprint(info) !== expected.leafFingerprint
    ) {
      throw new Error(
        "Access denied: audio is not the expected unique regular file"
      );
    }
    requirePathExpectation(path, expected);
    return fd;
  } catch (error) {
    closeSync(fd);
    throw error;
  }
}

function createProcessAudioBudget(
  hooks: McpProcessAudioAuthorizationHooks
): ProcessAudioBudget {
  const maxBytes = hooks.maxBytes ?? MCP_PROCESS_AUDIO_MAX_BYTES;
  const maxAggregateBytes =
    hooks.maxAggregateBytes ?? MCP_PROCESS_AUDIO_MAX_AGGREGATE_BYTES;
  const timeoutMs =
    hooks.timeoutMs ?? MCP_PROCESS_AUDIO_AUTHORIZATION_TIMEOUT_MS;
  if (
    !Number.isSafeInteger(maxBytes) ||
    maxBytes < 1 ||
    !Number.isSafeInteger(maxAggregateBytes) ||
    maxAggregateBytes < 1 ||
    !Number.isSafeInteger(timeoutMs) ||
    timeoutMs < 1
  ) {
    throw new Error(MCP_AUDIO_BUDGET_ERROR);
  }
  const nowMs = hooks.nowMs ?? (() => performance.now());
  const startedMs = nowMs();
  const deadlineMs = startedMs + timeoutMs;
  if (!Number.isFinite(startedMs) || !Number.isFinite(deadlineMs)) {
    throw new Error(MCP_AUDIO_BUDGET_ERROR);
  }
  return {
    maxBytes: Math.min(maxBytes, MCP_PROCESS_AUDIO_MAX_BYTES),
    maxAggregateBytes: Math.min(
      maxAggregateBytes,
      MCP_PROCESS_AUDIO_MAX_AGGREGATE_BYTES
    ),
    deadlineMs,
    nowMs,
  };
}

function requireProcessAudioBudget(
  budget: ProcessAudioBudget,
  byteLength?: bigint | number
): void {
  const now = budget.nowMs();
  const size =
    byteLength === undefined
      ? undefined
      : typeof byteLength === "bigint"
        ? byteLength
        : BigInt(byteLength);
  if (
    !Number.isFinite(now) ||
    now >= budget.deadlineMs ||
    (size !== undefined && size > BigInt(budget.maxBytes))
  ) {
    throw new Error(MCP_AUDIO_BUDGET_ERROR);
  }
}

function reserveProcessAudioBytes(
  byteLength: number,
  maxAggregateBytes: number
): () => void {
  if (reservedProcessAudioBytes > maxAggregateBytes - byteLength) {
    throw new Error(MCP_AUDIO_BUDGET_ERROR);
  }
  reservedProcessAudioBytes += byteLength;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    reservedProcessAudioBytes = Math.max(
      0,
      reservedProcessAudioBytes - byteLength
    );
  };
}

function acquireProcessAudioJob(): () => void {
  if (activeProcessAudioJobs >= MCP_PROCESS_AUDIO_MAX_ACTIVE_JOBS) {
    throw new Error(MCP_AUDIO_BUDGET_ERROR);
  }
  activeProcessAudioJobs += 1;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    activeProcessAudioJobs = Math.max(0, activeProcessAudioJobs - 1);
  };
}

function sanitizeAudioTitle(value: string): string {
  return (
    value
      .replace(/[\u0000-\u001f\u007f]/g, " ")
      .replace(/[\\/]/g, " ")
      .replace(/\s+/g, " ")
      .trim()
      .slice(0, 200) || "Untitled Recording"
  );
}

function authorizedAudioTitle(
  canonicalSourcePath: string,
  callerPath: string,
  requestedTitle?: string
): string {
  const fallback = sanitizeAudioTitle(
    basename(canonicalSourcePath, extname(canonicalSourcePath))
  );
  if (requestedTitle === undefined) return fallback;
  if (
    requestedTitle.includes(canonicalSourcePath) ||
    requestedTitle.includes(callerPath) ||
    requestedTitle.includes("/") ||
    requestedTitle.includes("\\")
  ) {
    return fallback;
  }
  const candidate = sanitizeAudioTitle(requestedTitle);
  // Titles remain useful, but a path-shaped title must never smuggle the
  // caller's source capability into argv or a child diagnostic.
  if (
    candidate.includes(canonicalSourcePath) ||
    candidate.includes(callerPath)
  ) {
    return fallback;
  }
  return candidate;
}

function authorizedAudioFormat(path: string): string {
  const format = extname(path).slice(1).toLowerCase();
  if (format !== "wav") {
    throw new Error(
      "Access denied: exact-byte agent audio currently supports bounded WAV input only"
    );
  }
  return format;
}

/**
 * Retain one exact input revision without creating any named copy,
 * request directory, durable registry, or cleanup pathname. The operation
 * receives only an fd capability and path-free metadata.
 */
export async function withAuthorizedMcpProcessAudioInput<T>(
  filePath: string,
  allowedDirs: string[],
  audioExts: string[],
  resolveEffectiveMeetingsRoot: EffectiveMeetingsRootResolver,
  requestedTitle: string | undefined,
  operation: (input: AuthorizedMcpProcessAudioInput) => Promise<T>,
  hooks: McpProcessAudioAuthorizationHooks = {}
): Promise<T> {
  if (process.platform !== "linux" && process.platform !== "darwin") {
    throw new Error("Access denied: private audio processing is unavailable");
  }
  const budget = createProcessAudioBudget(hooks);
  const initialRoot = attestMeetingsRoot(await resolveEffectiveMeetingsRoot());
  const canonicalSourcePath = validateMcpProcessAudioInput(
    filePath,
    allowedDirs,
    initialRoot.canonicalPath,
    audioExts
  );
  const expectation = captureBoundReadExpectation(canonicalSourcePath);
  const fd = openBoundSingleLinkAudio(canonicalSourcePath, expectation);
  let releaseBytes: (() => void) | undefined;
  try {
    hooks.onRetainedFd?.(fd);
    const initial = fstatSync(fd, { bigint: true });
    requireProcessAudioBudget(budget, initial.size);
    // Reserve the full per-input ceiling. A retained inode can grow until the
    // final proof rejects it; pessimistic admission keeps aggregate memory and
    // IO work bounded throughout that race.
    releaseBytes = reserveProcessAudioBytes(
      budget.maxBytes,
      budget.maxAggregateBytes
    );
    await hooks.afterValidation?.();
    // Do not read source bytes in the MCP event-loop process. The exact fd and
    // declared length cross the inherited-descriptor boundary; the outer-time-
    // bounded CLI performs the SHA-256 copy and before/after inode attestation.
    const digest: AudioDigest = { byteLength: Number(initial.size) };
    requirePathExpectation(canonicalSourcePath, expectation);
    await hooks.afterHash?.(digest);
    await hooks.beforeFinalAttestation?.();

    // Re-resolve live config and re-authorize the source path immediately
    // before spawning. The child independently rechecks fd metadata, copies
    // the exact bytes, and computes the digest within the outer deadline.
    const finalRoot = attestMeetingsRoot(await resolveEffectiveMeetingsRoot());
    requireProcessAudioBudget(budget);
    if (!sameMeetingsRoot(initialRoot, finalRoot)) {
      throw new Error(
        "Access denied: the live meeting root changed during authorization"
      );
    }
    const finalSourcePath = validateMcpProcessAudioInput(
      canonicalSourcePath,
      allowedDirs,
      finalRoot.canonicalPath,
      audioExts
    );
    if (finalSourcePath !== canonicalSourcePath) {
      throw new Error("Access denied: the authorized audio path changed");
    }
    requirePathExpectation(canonicalSourcePath, expectation);
    const finalInfo = fstatSync(fd, { bigint: true });
    if (
      !finalInfo.isFile() ||
      finalInfo.nlink !== 1n ||
      finalInfo.size !== BigInt(digest.byteLength) ||
      boundReadFingerprint(finalInfo) !== expectation.leafFingerprint
    ) {
      throw new Error("Access denied: authorized audio changed before dispatch");
    }

    const result = await operation({
      fd,
      digest,
      format: authorizedAudioFormat(canonicalSourcePath),
      safeTitle: authorizedAudioTitle(
        canonicalSourcePath,
        filePath,
        requestedTitle
      ),
    });
    let serialized = "";
    try {
      serialized = JSON.stringify(result) ?? "";
    } catch {
      throw new Error("Access denied: process_audio result was not serializable");
    }
    if (
      serialized.includes(canonicalSourcePath) ||
      serialized.includes(filePath)
    ) {
      throw new Error("Access denied: process_audio result exposed its source");
    }
    return result;
  } finally {
    releaseBytes?.();
    closeSync(fd);
  }
}

export function buildMcpProcessAudioArgs(
  input: AuthorizedMcpProcessAudioInput,
  contentType: "meeting" | "memo",
  language?: string
): string[] {
  validateAuthorizedProcessAudioInput(input);
  if (
    (contentType !== "meeting" && contentType !== "memo") ||
    (language !== undefined && !/^[A-Za-z0-9_-]{1,32}$/.test(language))
  ) {
    throw new Error("Access denied: process_audio arguments are invalid");
  }
  const syntheticPath = "authorized-input." + input.format;
  const args = [
    "process",
    syntheticPath,
    "-t",
    contentType,
    "--title",
    input.safeTitle,
  ];
  if (language) args.push("--language", language);
  args.push(
    "--authorized-input-fd",
    "3",
    "--authorized-input-bytes",
    String(input.digest.byteLength),
    "--authorized-input-format",
    input.format
  );
  return args;
}

function validateAuthorizedProcessAudioInput(
  input: AuthorizedMcpProcessAudioInput
): void {
  if (
    !Number.isSafeInteger(input.fd) ||
    input.fd < 0 ||
    !Number.isSafeInteger(input.digest.byteLength) ||
    input.digest.byteLength < 0 ||
    input.digest.byteLength > MCP_PROCESS_AUDIO_MAX_BYTES ||
    input.format !== "wav" ||
    input.safeTitle.length < 1 ||
    input.safeTitle.length > 200 ||
    sanitizeAudioTitle(input.safeTitle) !== input.safeTitle ||
    input.safeTitle.includes("/") ||
    input.safeTitle.includes("\\")
  ) {
    throw new Error("Access denied: authorized audio capability is invalid");
  }
  try {
    const info = fstatSync(input.fd, { bigint: true });
    if (
      !info.isFile() ||
      info.nlink !== 1n ||
      info.size !== BigInt(input.digest.byteLength)
    ) {
      throw new Error("invalid");
    }
  } catch {
    throw new Error("Access denied: authorized audio capability is invalid");
  }
}

export type McpProcessAudioCliOptions = {
  /** Test-only binary override. Production always uses the resolved Minutes CLI. */
  binary?: string;
  /** Overrides may only lower production limits. */
  timeoutMs?: number;
  maxStdoutBytes?: number;
  maxStderrBytes?: number;
  extraEnv?: Record<string, string>;
};

function boundedProcessAudioLimit(
  requested: number | undefined,
  productionLimit: number
): number {
  const value = requested ?? productionLimit;
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new Error(MCP_AUDIO_BUDGET_ERROR);
  }
  return Math.min(value, productionLimit);
}

function killProcessAudioGroup(child: ReturnType<typeof spawn>): void {
  const pid = child.pid;
  if (
    process.platform !== "win32" &&
    Number.isSafeInteger(pid) &&
    (pid ?? 0) > 0
  ) {
    try {
      process.kill(-(pid as number), "SIGKILL");
      return;
    } catch {
      // A start race may precede process-group creation; fall back to the
      // direct child handle without ever targeting an unvalidated PID.
    }
  }
  try {
    child.kill("SIGKILL");
  } catch {
    // It may already have exited. The close/error handler settles the promise.
  }
}

function processAudioHelperInvocation(): { binary: string; args: string[] } {
  const currentModule = fileURLToPath(import.meta.url);
  const sourceMode = extname(currentModule) === ".ts";
  const helper = fileURLToPath(
    new URL(
      sourceMode ? "./process-audio-helper.ts" : "./process-audio-helper.js",
      import.meta.url
    )
  );
  return sourceMode
    ? { binary: process.execPath, args: ["--import", "tsx", helper] }
    : { binary: process.execPath, args: [helper] };
}

function writeBoundedProcessAudioHelperRequest(
  child: ReturnType<typeof spawn>,
  value: unknown
): void {
  const serialized = JSON.stringify(value);
  if (Buffer.byteLength(serialized) > 64 * 1024 || !child.stdin) {
    throw new Error(MCP_AUDIO_BUDGET_ERROR);
  }
  child.stdin.write(serialized + "\n");
}

/**
 * Run the complete process_audio authorization and CLI tree in one detached,
 * bounded helper process. No caller-controlled filesystem operation occurs in
 * the long-lived MCP event loop. The helper is the process-group leader; the
 * CLI and all of its decoders remain in that same group, so one kill(-pgid)
 * contains a stalled FUSE lookup, retained source fd, CLI, and descendants.
 */
export async function runIsolatedMcpProcessAudio(
  filePath: string,
  allowedDirs: string[],
  audioExts: string[],
  resolveEffectiveMeetingsRoot: EffectiveMeetingsRootResolver,
  requestedTitle: string | undefined,
  contentType: "meeting" | "memo",
  language?: string,
  options: McpProcessAudioCliOptions = {},
  hooks: McpProcessAudioAuthorizationHooks = {}
): Promise<{ stdout: string; stderr: string }> {
  if (process.platform !== "linux" && process.platform !== "darwin") {
    throw new Error("Access denied: private audio processing is unavailable");
  }
  if (processAudioIsolationPoisoned) {
    throw new Error(
      "Access denied: private audio processing requires an MCP restart"
    );
  }
  const budget = createProcessAudioBudget(hooks);
  const timeoutMs = boundedProcessAudioLimit(
    options.timeoutMs,
    MCP_PROCESS_AUDIO_CLI_TIMEOUT_MS
  );
  const maxStdoutBytes = boundedProcessAudioLimit(
    options.maxStdoutBytes,
    MCP_PROCESS_AUDIO_MAX_STDOUT_BYTES
  );
  const maxStderrBytes = boundedProcessAudioLimit(
    options.maxStderrBytes,
    MCP_PROCESS_AUDIO_MAX_STDERR_BYTES
  );
  const releaseBytes = reserveProcessAudioBytes(
    budget.maxBytes,
    budget.maxAggregateBytes
  );
  try {
    const initialMeetingsRoot = await resolveEffectiveMeetingsRoot();
    requireProcessAudioBudget(budget);
    const helper = processAudioHelperInvocation();
    const safeExtraEnv = { ...(options.extraEnv ?? {}) };
    delete safeExtraEnv.MINUTES_MCP_OUTER_PROCESS_GROUP;

    return await new Promise((resolveRun, rejectRun) => {
      let child: ReturnType<typeof spawn>;
      try {
        child = spawn(helper.binary, helper.args, {
          detached: true,
          stdio: ["pipe", "pipe", "pipe", "pipe"],
          env: nodeChildEnvironment(mcpCliChildEnv()),
        });
      } catch {
        rejectRun(new Error("process_audio helper could not be started safely"));
        return;
      }

      let stdoutBytes = 0;
      let stderrBytes = 0;
      let controlBytes = 0;
      const stdoutChunks: Buffer[] = [];
      const stderrChunks: Buffer[] = [];
      const controlChunks: Buffer[] = [];
      let failure: string | undefined;
      let settled = false;
      let authorized = false;
      let rootUpdateStarted = false;
      let timer: NodeJS.Timeout;

      const requestFailure = (message: string): void => {
        if (settled) return;
        if (failure === undefined) failure = message;
        // A helper killed while blocked in an uninterruptible kernel
        // filesystem operation may remain pending until that kernel request
        // returns. Permanently refusing another helper bounds this MCP process
        // to one doomed out-of-process authorization and zero retained parent
        // descriptors until the host restarts it.
        processAudioIsolationPoisoned = true;
        settled = true;
        clearTimeout(timer);
        killProcessAudioGroup(child);
        // Do not wait for `close`: a helper wedged in an uninterruptible
        // filesystem syscall may never emit it.  The doomed process remains
        // contained in its poisoned process group while the MCP request and
        // aggregate-byte reservation are released immediately.
        rejectRun(new Error(failure));
      };
      child.stdin?.on("error", () => {
        requestFailure("process_audio helper request channel failed safely");
      });
      const resetTimer = (durationMs: number, message: string): void => {
        clearTimeout(timer);
        timer = setTimeout(() => requestFailure(message), durationMs);
      };
      timer = setTimeout(
        () => requestFailure("process_audio authorization exceeded its time budget"),
        Math.max(1, Math.min(hooks.timeoutMs ?? MCP_PROCESS_AUDIO_AUTHORIZATION_TIMEOUT_MS, MCP_PROCESS_AUDIO_AUTHORIZATION_TIMEOUT_MS))
      );

      child.stdout?.on("data", (value: Buffer | string) => {
        const bytes = Buffer.isBuffer(value) ? value : Buffer.from(value);
        stdoutBytes += bytes.byteLength;
        if (stdoutBytes > maxStdoutBytes) {
          requestFailure("process_audio CLI stdout exceeded its byte budget");
          return;
        }
        stdoutChunks.push(bytes);
      });
      child.stderr?.on("data", (value: Buffer | string) => {
        const bytes = Buffer.isBuffer(value) ? value : Buffer.from(value);
        stderrBytes += bytes.byteLength;
        if (stderrBytes > maxStderrBytes) {
          requestFailure("process_audio CLI stderr exceeded its byte budget");
          return;
        }
        stderrChunks.push(bytes);
      });

      const control = child.stdio[3];
      if (!control || typeof (control as NodeJS.ReadableStream).on !== "function") {
        requestFailure("process_audio helper control channel was unavailable");
      } else {
        (control as NodeJS.ReadableStream).on(
          "data",
          (value: Buffer | string) => {
            if (authorized || rootUpdateStarted) {
              requestFailure("process_audio helper protocol was invalid");
              return;
            }
            const bytes = Buffer.isBuffer(value) ? value : Buffer.from(value);
            controlBytes += bytes.byteLength;
            if (controlBytes > 8 * 1024) {
              requestFailure("process_audio helper protocol exceeded its budget");
              return;
            }
            controlChunks.push(bytes);
            const serialized = Buffer.concat(controlChunks).toString("utf8");
            const newline = serialized.indexOf("\n");
            if (newline < 0) return;
            if (serialized.slice(newline + 1).length > 0) {
              requestFailure("process_audio helper protocol was invalid");
              return;
            }
            let message: unknown;
            try {
              message = JSON.parse(serialized.slice(0, newline));
            } catch {
              requestFailure("process_audio helper protocol was invalid");
              return;
            }
            if (
              !message ||
              typeof message !== "object" ||
              Array.isArray(message) ||
              Object.keys(message).sort().join("\0") !==
                ["byteLength", "status"].sort().join("\0") ||
              (message as any).status !== "authorized" ||
              !Number.isSafeInteger((message as any).byteLength) ||
              (message as any).byteLength < 0 ||
              (message as any).byteLength > budget.maxBytes
            ) {
              requestFailure("process_audio helper protocol was invalid");
              return;
            }
            authorized = true;
            rootUpdateStarted = true;
            void (async () => {
              await hooks.afterValidation?.();
              const digest = { byteLength: (message as any).byteLength };
              await hooks.afterHash?.(digest);
              await hooks.beforeFinalAttestation?.();
              const finalMeetingsRoot = await resolveEffectiveMeetingsRoot();
              requireProcessAudioBudget(budget);
              writeBoundedProcessAudioHelperRequest(child, {
                finalMeetingsRoot,
              });
              child.stdin?.end();
              resetTimer(timeoutMs, "process_audio CLI exceeded its time budget");
            })().catch(() => {
              requestFailure("process_audio final authorization failed safely");
            });
          }
        );
      }

      child.once("error", () => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        killProcessAudioGroup(child);
        rejectRun(new Error("process_audio helper could not be started safely"));
      });
      child.once("close", (code) => {
        if (hooks.ignoreHelperCloseForTest) return;
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        killProcessAudioGroup(child);
        if (failure !== undefined) {
          rejectRun(new Error(failure));
          return;
        }
        if (code !== 0 || !authorized) {
          rejectRun(new Error("process_audio authorization or CLI failed safely"));
          return;
        }
        const result = {
          stdout: Buffer.concat(stdoutChunks).toString("utf8").trim(),
          stderr: Buffer.concat(stderrChunks).toString("utf8").trim(),
        };
        const serialized = JSON.stringify(result);
        if (serialized.includes(filePath)) {
          rejectRun(new Error("Access denied: process_audio result exposed its source"));
          return;
        }
        resolveRun(result);
      });

      try {
        writeBoundedProcessAudioHelperRequest(child, {
          schemaVersion: 1,
          filePath,
          allowedDirs,
          audioExts,
          initialMeetingsRoot,
          requestedTitle,
          contentType,
          language,
          cliBinary: options.binary ?? MINUTES_BIN,
          maxBytes: budget.maxBytes,
          extraEnv: safeExtraEnv,
        });
      } catch {
        requestFailure("process_audio helper request was invalid");
      }
    });
  } finally {
    releaseBytes();
  }
}
export const MCP_PROCESS_AUDIO_WINDOWS_UNAVAILABLE_MESSAGE =
  "process_audio is unavailable on Windows because its agent-facing inherited-descriptor boundary is not supported there. No audio was read or passed to the CLI; use the Minutes desktop app or CLI directly.";
export const MCP_PROCESS_AUDIO_UNSUPPORTED_UNIX_MESSAGE =
  "process_audio is unavailable on this platform because its private-audio capability is supported only on macOS and Linux. No audio was read or passed to the CLI.";

export function mcpProcessAudioPlatformPolicy(
  platform: NodeJS.Platform
): { available: true } | { available: false; error: string } {
  if (platform === "win32") {
    return {
      available: false,
      error: MCP_PROCESS_AUDIO_WINDOWS_UNAVAILABLE_MESSAGE,
    };
  }
  if (platform !== "darwin" && platform !== "linux") {
    return {
      available: false,
      error: MCP_PROCESS_AUDIO_UNSUPPORTED_UNIX_MESSAGE,
    };
  }
  return { available: true };
}

export type McpProcessAudioToolInput = {
  file_path: string;
  type: "meeting" | "memo";
  title?: string;
  language?: string;
};

export type McpProcessAudioToolDependencies = {
  isCliAvailable: () => Promise<boolean>;
  execute: (input: McpProcessAudioToolInput) => Promise<{ stdout: string }>;
};

type McpProcessAudioSuccess = {
  status: "done";
  file: string;
  title: string;
  words: number;
};

function parseMcpProcessAudioSuccess(stdout: string): McpProcessAudioSuccess | null {
  let value: unknown;
  try {
    value = JSON.parse(stdout);
  } catch {
    return null;
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const result = value as Record<string, unknown>;
  if (
    result.status !== "done" ||
    typeof result.file !== "string" ||
    result.file.trim() === "" ||
    typeof result.title !== "string" ||
    result.title.trim() === "" ||
    !Number.isSafeInteger(result.words) ||
    (result.words as number) < 0
  ) {
    return null;
  }
  return {
    status: "done",
    file: result.file.trim(),
    title: result.title.trim(),
    words: result.words as number,
  };
}

function mcpProcessAudioError(error: string, message: string) {
  return {
    content: [{ type: "text" as const, text: message }],
    structuredContent: { available: false, error },
    isError: true,
  };
}

export async function handleMcpProcessAudioRequest(
  input: McpProcessAudioToolInput,
  dependencies: McpProcessAudioToolDependencies,
  platform: NodeJS.Platform = process.platform
) {
  // This gate is intentionally first: on unsupported platforms no caller path or ambient
  // filesystem/CLI dependency is inspected before the path-free denial.
  const platformPolicy = mcpProcessAudioPlatformPolicy(platform);
  if (!platformPolicy.available) {
    return mcpProcessAudioError(
      platform === "win32"
        ? "windows-agent-audio-fd-unavailable"
        : "private-audio-capability-unavailable",
      platformPolicy.error
    );
  }

  try {
    const releaseJob = acquireProcessAudioJob();
    try {
      if (!(await dependencies.isCliAvailable())) {
        return mcpProcessAudioError(
          "cli-unavailable",
          "process_audio is unavailable because the Minutes CLI is not ready. No audio was read or passed to the CLI."
        );
      }
      const { stdout } = await dependencies.execute(input);
      const result = parseMcpProcessAudioSuccess(stdout);
      if (!result) {
        return mcpProcessAudioError(
          "invalid-cli-response",
          "process_audio failed because the Minutes CLI returned an invalid completion response. No result was accepted."
        );
      }
      return {
        content: [
          {
            type: "text" as const,
            text: `Processed: ${result.file}\nTitle: ${result.title}\nWords: ${result.words}`,
          },
        ],
        structuredContent: { available: true, ...result },
      };
    } finally {
      releaseJob();
    }
  } catch (error) {
    return mcpProcessAudioError(
      "processing-failed",
      "process_audio failed safely during authorization or execution. No CLI error details or input paths were returned."
    );
  }
}

registerTool(
  "process_audio",
  "On macOS and Linux, process a bounded WAV file from the Minutes inbox or Downloads through the transcription pipeline. Compressed/private containers and Windows fail closed without reading audio. Retained meeting-library audio is unavailable to agent surfaces.",
  {
    file_path: z.string().describe("Path to an inbox/Downloads WAV file (.wav)"),
    type: z.enum(["meeting", "memo"]).optional().default("memo").describe("Content type"),
    title: z.string().optional().describe("Optional title"),
    language: z.string().optional().describe("Transcription language code (e.g. 'en', 'ur', 'es', 'zh'). Overrides config.toml setting."),
  },
  { title: "Process Audio", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async ({ file_path, type: contentType, title, language }) => {
    const allowedDirs = [join(MINUTES_HOME, "inbox"), join(homedir(), "Downloads")];
    const audioExts = [".wav"];
    return handleMcpProcessAudioRequest(
      { file_path, type: contentType, title, language },
      {
        isCliAvailable,
        execute: (input) =>
          runIsolatedMcpProcessAudio(
            input.file_path,
            allowedDirs,
            audioExts,
            () => getEffectiveMeetingsDirForIsolatedAudio(),
            input.title,
            input.type,
            input.language
          ),
      }
    );
  }
);

// ── Tool: add_note ───────────────────────────────────────────

export const MCP_ADD_NOTE_INPUT_SCHEMA = Object.freeze({
  text: z.string().describe("The note text (plain text, no markdown needed)"),
});

registerTool(
  "add_note",
  "Add a note to the current active recording. Notes are timestamped and included in that recording's meeting summary. Existing meeting files cannot be mutated from this assistant tool.",
  MCP_ADD_NOTE_INPUT_SCHEMA,
  { title: "Add Note", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async ({ text }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }
    try {
      const args = ["note", text];
      const { stdout, stderr } = await runMinutes(args);
      return {
        content: [{ type: "text" as const, text: stderr || stdout || "Note added." }],
      };
    } catch {
      return {
        content: [{ type: "text" as const, text: "Note could not be safely added." }],
        isError: true,
      };
    }
  }
);

// ── Tool: track_commitments ─────────────────────────────────

export function historicalCommitmentRows(commitments: readonly PolicyIntentResult[]) {
  return commitments.map((commitment) => ({
    text: commitment.what,
    status: commitment.status,
    due_date: commitment.by_date ?? null,
    created_at: commitment.date,
    commitment_type: commitment.kind === "action-item" ? "action_item" : "intent",
    meeting_title: commitment.title,
    meeting_date: commitment.date,
    person_name: commitment.who ?? null,
  }));
}

export function relationshipMapStructuredContent<T>(people: readonly T[]) {
  return { people: [...people], view: "relationship_map" as const };
}

registerDocsAppTool(
  "track_commitments",
  {
    description: "List open and stale action items and explicit intent commitments from live meeting frontmatter. Optionally filter by person. Answers: 'What did I promise Sarah?' or 'What's overdue?' Meetings designated `sensitivity: restricted` never enter this live-source view.",
    inputSchema: {
      person: z.string().trim().min(1).max(MCP_QUERY_MAX_CHARS).optional().describe("Filter by person name or slug (optional — omit for all commitments)"),
    },
    annotations: { title: "Track Commitments", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ person }) => {
    const capabilities = await ensureCliCapabilities([
      "track_commitments",
      "policy_projection_worker_v1",
    ]);
    if (
      capabilities.kind !== "report" ||
      !hasFeature(capabilities, "track_commitments") ||
      !hasFeature(capabilities, "policy_projection_worker_v1")
    ) {
      throw new Error(
        "Commitment tracking requires a Minutes CLI with the supervised policy-projection boundary. Update Minutes, then try again."
      );
    }
    const args = ["commitments", "--json", "--limit", String(MCP_INTENT_RESULT_MAX)];
    if (person) args.push("--person", person);
    const { stdout } = await runPolicyGraphMinutes(args);
    const parsed = parseJsonOutput(stdout);
    if (!Array.isArray(parsed) || parsed.length > MCP_INTENT_RESULT_MAX) {
      throw new Error("Minutes returned an invalid bounded commitment projection");
    }
    const commitments = parsed.map((value: unknown) => {
      if (!value || typeof value !== "object" || Array.isArray(value)) {
        throw new Error("Minutes returned an invalid commitment row");
      }
      const row = value as Record<string, unknown>;
      const field = (raw: unknown, label: string, optional = false): string | null => {
        if (raw === null && optional) return null;
        if (typeof raw !== "string") {
          throw new Error(`Minutes returned an invalid commitment ${label}`);
        }
        return boundedMcpField(raw) ?? "";
      };
      const status = field(row.status, "status") as string;
      if (status !== "open" && status !== "stale") {
        throw new Error("Minutes returned an invalid commitment status");
      }
      const commitmentType = field(row.commitment_type, "type") as string;
      if (commitmentType !== "action_item" && commitmentType !== "intent") {
        throw new Error("Minutes returned an invalid commitment type");
      }
      return {
        text: field(row.text, "text") as string,
        status,
        due_date: field(row.due_date, "due date", true),
        created_at: field(row.created_at, "created at") as string,
        commitment_type: commitmentType,
        meeting_title: field(row.meeting_title, "meeting title") as string,
        meeting_date: field(row.meeting_date, "meeting date") as string,
        person_name: field(row.person_name, "person name", true),
      };
    });
    const boundedPerson = boundedMcpField(person) || null;

    if (commitments.length === 0) {
      const scope = boundedPerson ? ` for ${boundedPerson}` : "";
      return {
        content: [{ type: "text" as const, text: boundedMcpText(`No open commitments found${scope}.`) }],
        structuredContent: { commitments: [], person: boundedPerson, view: "commitments" },
        _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "commitments" },
      };
    }

    // Group by status
    const stale = commitments.filter((commitment) => commitment.status === "stale");
    const open = commitments.filter((commitment) => commitment.status === "open");
    const structuredCommitments = commitments;

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

    const text = `Commitments${boundedPerson ? ` for ${boundedPerson}` : ""}:\n\n${lines.join("\n")}`;

    return {
      content: [{ type: "text" as const, text: boundedMcpText(text) }],
      structuredContent: { commitments: structuredCommitments, person: boundedPerson, stale_count: stale.length, open_count: open.length, view: "commitments" },
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "commitments" },
    };
  }
);

// ── Tool: relationship_map ──────────────────────────────────

registerDocsAppTool(
  "relationship_map",
  {
    description: "Show contacts with relationship scores, meeting frequency, and losing-touch alerts from the bounded process-private graph projection. Restricted meetings never enter the projection.",
    inputSchema: {
      limit: z
        .number()
        .int()
        .min(1)
        .max(MCP_RELATIONSHIP_RESULT_MAX)
        .optional()
        .default(15)
        .describe(`Max people to return (1-${MCP_RELATIONSHIP_RESULT_MAX})`),
    },
    annotations: { title: "Relationship Map", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    _meta: { ui: { resourceUri: UI_RESOURCE_URI } },
  },
  async ({ limit }) => {
    const graphCapabilities = await ensureCliCapabilities([
      "relationship_map_policy_fresh_v1",
      "policy_projection_worker_v1",
    ]);
    if (
      graphCapabilities.kind !== "report" ||
      !hasFeature(graphCapabilities, "relationship_map_policy_fresh_v1") ||
      !hasFeature(graphCapabilities, "policy_projection_worker_v1")
    ) {
      throw new Error(
        "Relationship Map requires a Minutes CLI with the policy-fresh process-private graph boundary. Update Minutes, then try again."
      );
    }
    const { stdout } = await runPolicyGraphMinutes([
      "people",
      "--json",
      "--limit",
      String(limit),
    ]);
    const parsed = parseJsonOutput(stdout);
    if (!Array.isArray(parsed) || parsed.length > limit) {
      throw new Error("Minutes returned an invalid bounded relationship projection");
    }
    const people = parsed.map((raw: unknown) => {
      const finiteNumber = (
        value: unknown,
        field: string,
        integer: boolean = false
      ): number => {
        if (
          typeof value !== "number" ||
          !Number.isFinite(value) ||
          value < 0 ||
          (integer && !Number.isSafeInteger(value))
        ) {
          throw new Error(`Minutes returned an invalid relationship ${field}`);
        }
        return value;
      };
      const boundedString = (value: unknown, field: string): string => {
        if (typeof value !== "string") {
          throw new Error(`Minutes returned an invalid relationship ${field}`);
        }
        return boundedMcpField(value) ?? "";
      };
      if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
        throw new Error("Minutes returned an invalid relationship row");
      }
      const row = raw as Record<string, unknown>;
      if (!Array.isArray(row.top_topics) || typeof row.losing_touch !== "boolean") {
        throw new Error("Minutes returned an invalid relationship row");
      }
      const topTopics = row.top_topics
        .slice(0, 3)
        .map((topic: unknown) => boundedString(topic, "topic"));
      return {
        slug: boundedString(row.slug, "slug"),
        name: boundedString(row.name, "name"),
        meeting_count: finiteNumber(row.meeting_count, "meeting count", true),
        last_seen: boundedString(row.last_seen, "last seen"),
        days_since: finiteNumber(row.days_since, "days since"),
        open_commitments: finiteNumber(row.open_commitments, "open commitments", true),
        top_topics: topTopics,
        score: finiteNumber(row.score, "score"),
        losing_touch: row.losing_touch,
      };
    });
    if (people.length === 0) {
      return {
        content: [{ type: "text" as const, text: "No relationship data found in policy-authorized meetings." }],
        structuredContent: relationshipMapStructuredContent([]),
        _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "relationship_map" },
      };
    }
    const lines: string[] = [];
    const losingTouch: string[] = [];
    for (const person of people) {
      const daysSince = Math.round(person.days_since);
      const last = daysSince < 1 ? "today" : daysSince < 2 ? "yesterday" : `${daysSince}d ago`;
      const status = person.losing_touch
        ? "⚠ losing touch"
        : person.open_commitments > 0
          ? `${person.open_commitments} open commitment${person.open_commitments === 1 ? "" : "s"}`
          : "✓ all clear";
      lines.push(`${person.name} — ${person.meeting_count} meetings, last: ${last}, ${status} (score: ${person.score.toFixed(1)})`);
      if (person.losing_touch) {
        losingTouch.push(`${person.name} — ${person.meeting_count} meetings total, last seen ${daysSince}d ago`);
      }
    }
    let text = `Relationship Map (${people.length} contacts):\n\n${lines.join("\n")}`;
    if (losingTouch.length > 0) {
      text += `\n\nLosing Touch:\n${losingTouch.join("\n")}`;
    }
    return {
      content: [{ type: "text" as const, text: boundedMcpText(text) }],
      structuredContent: relationshipMapStructuredContent(people),
      _meta: { ui: { resourceUri: UI_RESOURCE_URI }, view: "relationship_map" },
    };
  }
);

// ── Resources ───────────────────────────────────────────────

registerResource(
  "recent_meetings",
  "minutes://meetings/recent",
  { description: "List of recent meetings and memos" },
  async () => afterContentResourceReadiness("recent_meetings", async () => {
    const meetingsDir = await getEffectiveMeetingsDir();
    const meetings = await policyListMeetings(meetingsDir, 20, false);
    const json = boundedMcpJsonArray(meetings.map(meetingListItem));
    return { contents: [{ uri: "minutes://meetings/recent", mimeType: "application/json", text: json }] };
  })
);

registerResource(
  "recording_status",
  "minutes://status",
  { description: "Current recording status" },
  async () => {
    if (!(await isCliAvailable())) {
      return buildPrivacySafeStatusResource({ recording: false, processing: false });
    }
    try {
      const { stdout } = await runMinutes(["status"]);
      return buildPrivacySafeStatusResource(JSON.parse(stdout));
    } catch {
      return buildPrivacySafeStatusResource(null);
    }
  }
);

registerResource(
  "open_actions",
  "minutes://actions/open",
  { description: "All open action items across meetings" },
  async () => afterContentResourceReadiness("open_actions", async () => {
    const meetingsDir = await getEffectiveMeetingsDir();
    const meetings = await policyListMeetings(
      meetingsDir,
      MCP_POLICY_MEETING_RESULT_MAX,
      false
    );
    const actions = openActionsFromMeetings(meetings);
    const boundedActions = actions.map(({ path, item }) => ({
      path: boundedMcpField(path) ?? "",
      item: boundedActionItem(item),
    }));
    return { contents: [{ uri: "minutes://actions/open", mimeType: "application/json", text: boundedMcpJsonArray(boundedActions) }] };
  })
);

registerResource(
  "recent_events",
  "minutes://events/recent",
  { description: "Recent pipeline events with meeting-derived content withheld until source policy provenance is available" },
  async () => {
    const payload = {
      events: [],
      unavailable:
        "Event records are withheld from MCP until each content-bearing record carries live-verifiable meeting policy provenance.",
    };
    return { contents: [{ uri: "minutes://events/recent", mimeType: "application/json", text: JSON.stringify(payload) }] };
  }
);

registerResource(
  "agent_annotations",
  "minutes://events/agent-annotations",
  { description: "Agent annotations are withheld until their source policy provenance can be revalidated" },
  async () => {
    return {
      contents: [{
        uri: "minutes://events/agent-annotations",
        mimeType: "application/json",
        text: JSON.stringify({ annotations: [], unavailable: "Source policy provenance is required before annotations can be exposed to agents." }),
      }],
    };
  }
);

if (LIVE_EVENTS_SUPPORTED) {
  registerResource(
    "live_events",
    LIVE_EVENTS_RESOURCE_URI,
    {
      description:
        "Live events are currently withheld because raw cursors can reveal restricted activity; reads return a constant unavailable response.",
    },
    async (uri) => readLiveEventsResource(uri)
  );

  registerResource(
    "live_events_since_seq",
    new ResourceTemplate(LIVE_EVENTS_URI_TEMPLATE, { list: undefined }),
    {
      description:
        "Live event cursor reads are currently withheld because raw cursors can reveal restricted activity; reads return a constant unavailable response.",
    },
    async (uri) => readLiveEventsResource(uri)
  );
} else {
  crashTrace("live-events-resource-disabled", { reason: "missing events_since_seq CLI capability" });
}

if (COPILOT_SUPPORTED) {
  registerResource(
    "live_copilot",
    LIVE_COPILOT_RESOURCE_URI,
    {
      description:
        "Current copilot state and latest observed nudge. Subscribe for notifications/resources/updated or poll this URI; MCP only controls and observes the independent minutes copilot engine.",
    },
    async (uri) => readLiveCopilotResource(uri)
  );
} else {
  crashTrace("live-copilot-resource-disabled", { reason: "missing copilot_realtime CLI capability" });
}

if (COPILOT_SUPPORTED) {
  // Each server instance gets its own controller — the poller and subscription
  // set are per-connection state, and the returned stop() is the instance
  // teardown so a closed HTTP session leaves no poller running.
  forEachServer((target) => {
    const controller = registerLiveEventsSubscriptionHandlers(target, {
      // Reads remain registered as an honest constant-unavailable resource, but
      // subscriptions must not poll raw sequence numbers or notify on hidden
      // restricted events.
      enableLiveEvents: LIVE_EVENTS_SUBSCRIPTIONS_ENABLED,
      enableCopilot: COPILOT_SUPPORTED,
    });
    return () => controller.stop();
  });
}

registerResource(
  "meeting",
  new ResourceTemplate("minutes://meetings/{slug}", { list: undefined }),
  { description: "Get a specific meeting by its filename slug" },
  async (uri, variables) => afterContentResourceReadiness("meeting", async () => {
    const slug = String(variables.slug);
    const meetingsDir = await getEffectiveMeetingsDir();
    const snapshots = await policyVerifiedMeetingSnapshots(
      meetingsDir,
      false
    );
    const match = snapshots.find(
      (snapshot) => basename(snapshot.path, ".md") === slug
    );
    if (match) {
      return {
        contents: [{
          uri: uri.href,
          mimeType: "text/markdown",
          text: match.content,
        }],
      };
    }
    return { contents: [{ uri: uri.href, mimeType: "text/plain", text: `Meeting not found: ${slug}` }] };
  })
);

// ── Resource: recent_ideas (voice memos from last N days) ──

registerResource(
  "recent-ideas",
  "minutes://ideas/recent",
  { description: "Recent voice memos and ideas captured from any device (last 14 days)" },
  async (uri) => afterContentResourceReadiness("recent-ideas", async () => {
    const meetingsDir = await getEffectiveMeetingsDir();
    const meetings = await policyListMeetings(
      meetingsDir,
      200,
      false
    );
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
  })
);

// ── Tool: start_dictation ──────────────────────────────────

export interface DictationModelMissingError {
  model: string;
  expectedPath: string;
  setupCommand: string;
}

export function parseDictationModelMissingError(
  stderr: string
): DictationModelMissingError | null {
  const model = stderr.match(/Dictation model not installed:\s*([^\r\n]+)/)?.[1]?.trim();
  const expectedPath = stderr.match(/Expected:\s*([^\r\n]+)/)?.[1]?.trim();
  const setupCommand = stderr.match(/Fix:\s*([^\r\n]+)/)?.[1]?.trim();
  if (!model || !expectedPath || !setupCommand) return null;
  return { model, expectedPath, setupCommand };
}

registerTool(
  "start_dictation",
  "Start dictation mode. Speak naturally — text accumulates across pauses and the combined result is written when dictation ends. Runs until stop_dictation is called or silence timeout.",
  {
    language: z.string().optional().describe("Transcription language code (e.g. 'en', 'ur', 'es', 'zh'). Overrides config.toml setting."),
  },
  { title: "Start Dictation", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async ({ language }) => {
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

    // Extension runtime: mic won't work for spawned child processes.
    // Desktop delegation for dictation requires a future Tauri extension.
    if (isExtensionRuntime) {
      return {
        content: [
          {
            type: "text" as const,
            text: "Dictation is not yet supported via the Claude Desktop extension. " +
              "The extension runtime cannot pass microphone access to child processes.\n\n" +
              "Workaround: run `minutes dictate` from your terminal, or use start_recording instead " +
              "(recording delegates to the Minutes desktop app when it's running).",
          },
        ],
        isError: true,
      };
    }

    try {
      await execFileAsync(MINUTES_BIN, ["dictate", "--preflight"], {
        timeout: 5000,
        env: mcpCliChildEnv(),
      });
    } catch (error: any) {
      const stderr = typeof error?.stderr === "string" ? error.stderr : "";
      const missing = parseDictationModelMissingError(stderr);
      if (missing) {
        return {
          content: [
            {
              type: "text" as const,
              text:
                `Dictation model '${missing.model}' is not installed. ` +
                `Expected it at ${missing.expectedPath}.\n\nFix: ${missing.setupCommand}`,
            },
          ],
          structuredContent: {
            status: "error",
            code: "MODEL_MISSING",
            model: missing.model,
            expectedPath: missing.expectedPath,
            setupCommand: missing.setupCommand,
          },
          isError: true,
        };
      }
      return {
        content: [
          {
            type: "text" as const,
            text: `Dictation could not pass startup checks: ${stderr.trim() || error?.message || error}`,
          },
        ],
        structuredContent: {
          status: "error",
          code: "DICTATION_PREFLIGHT_FAILED",
        },
        isError: true,
      };
    }

    // Spawn dictation as child (not detached — preserves macOS TCC mic grant)
    const dictArgs = ["dictate"];
    if (language) dictArgs.push("--language", language);
    const child = spawn(MINUTES_BIN, dictArgs, {
      stdio: "ignore",
      env: mcpCliChildEnv({ RUST_LOG: "info" }),
    });
    child.unref();

    // Wait briefly for startup
    await new Promise((r) => setTimeout(r, 500));

    return {
      content: [
        {
          type: "text" as const,
          text: "Dictation started. Speak naturally — text accumulates across pauses and will be copied when dictation ends. Say \"stop dictation\" when done.",
        },
      ],
    };
  }
);

// ── Tool: stop_dictation ───────────────────────────────────

registerTool(
  "stop_dictation",
  "Stop the current dictation session.",
  {},
  { title: "Stop Dictation", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async () => {
    const stopped = await terminalControlBeforeContentReadiness(async () => {
      // Send stop signal by killing the dictation process via PID file.
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
          // Process already dead or PID file invalid.
        }
      }
    });
    if (!stopped.mayRevealContent) {
      return {
        content: [{
          type: "text" as const,
          text: "Dictation stop requested. Any derived result is withheld until the agent trust boundary is ready.",
        }],
      };
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

registerTool(
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

registerTool(
  "confirm_speaker",
  "Compatibility registration for speaker confirmation. Agent-controlled mutation is intentionally unavailable; confirm in the Minutes app or a human CLI session so identity changes cannot race policy authorization.",
  {
    meeting: z.string().describe("Path to the meeting markdown file"),
    speaker_label: z.string().describe("Speaker label to confirm (e.g., SPEAKER_1)"),
    name: z.string().describe("Real name to assign to this speaker"),
  },
  { title: "Confirm Speaker", readOnlyHint: false, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async () => ({
    content: [{
      type: "text" as const,
      text: "Speaker confirmation is unavailable in agent sessions. Use the Minutes app or run minutes confirm directly in a human terminal.",
    }],
    isError: true,
  })
);

// ── Tool: add_agent_annotation ─────────────────────────────

registerTool(
  "add_agent_annotation",
  "Append attributed agent commentary as an agent.annotation event. This never edits meeting markdown/frontmatter and is rejected unless the agent_id is allowed in ~/.minutes/agents.allow.",
  {
    agent_id: z.string().describe("Stable agent identifier listed in ~/.minutes/agents.allow"),
    tools: z.array(z.string()).optional().default([]).describe("Tool or model names used to produce the annotation"),
    subkind: z.string().optional().default("commentary").describe("Annotation subtype, e.g. coaching, correction, risk, summary"),
    meeting_id: z.string().optional().describe("Target meeting identifier, if known"),
    meeting_path: z.string().optional().describe("Target meeting markdown path, if known"),
    span_start_ms: z.number().optional().describe("Start offset of the target span in milliseconds"),
    span_end_ms: z.number().optional().describe("End offset of the target span in milliseconds"),
    body: z.string().describe("Annotation body"),
    citations: z.array(z.string()).optional().default([]).describe("Source citations or event references"),
    confidence: z.enum(["low", "medium", "high", "tentative", "inferred", "strong", "explicit"]).optional().default("medium"),
    provenance: z.any().optional().describe("JSON-serializable provenance object"),
  },
  { title: "Add Agent Annotation", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async ({
    agent_id,
    tools,
    subkind,
    meeting_id,
    meeting_path,
    span_start_ms,
    span_end_ms,
    body,
    citations,
    confidence,
    provenance,
  }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }], isError: true };
    }

    const args = [
      "agent-annotate",
      "--agent-id",
      agent_id,
      "--subkind",
      subkind,
      "--body",
      body,
      "--confidence",
      confidence,
      "--provenance",
      JSON.stringify(provenance ?? { via: "minutes-mcp", tool: "add_agent_annotation" }),
    ];
    for (const tool of tools ?? []) args.push("--tool", tool);
    for (const citation of citations ?? []) args.push("--citation", citation);
    if (meeting_id) args.push("--meeting-id", meeting_id);
    if (meeting_path) args.push("--meeting-path", meeting_path);
    if (span_start_ms !== undefined || span_end_ms !== undefined) {
      if (span_start_ms !== undefined) args.push("--span-start-ms", String(span_start_ms));
      if (span_end_ms !== undefined) args.push("--span-end-ms", String(span_end_ms));
    }

    try {
      const { stdout } = await runMinutes(args, 10000);
      const event = parseJsonOutput(stdout);
      return {
        content: [{ type: "text" as const, text: `Appended agent.annotation seq ${event?.seq ?? "unknown"}.` }],
        structuredContent: { event },
      };
    } catch (error: any) {
      const message = error?.message || String(error);
      const structured = parseStructuredCliError(message);
      return {
        content: [{ type: "text" as const, text: structured?.message || `Failed to append agent.annotation: ${message}` }],
        structuredContent: structured ? { error: structured } : undefined,
        isError: true,
      };
    }
  }
);

// ── Tool: get_agent_annotations ────────────────────────────

/**
 * Annotations stay unavailable, and the reason is architectural rather than
 * incidental.
 *
 * An annotation's only link to a meeting is `target.meeting_path`, which is
 * free-form caller input to the live `add_agent_annotation` tool; the CLI
 * validator never checks that the target exists, sits in the corpus, or bears
 * any relationship to the body. Its `body` and `citations` are likewise
 * free text. Revalidating the target therefore proves nothing about the
 * content: an agent that read a restricted meeting under an audited override
 * can write an annotation targeting a normal meeting with the restricted
 * content in its body, and every later read releases it with no override and
 * no audit. One audited grant becomes a permanent unaudited channel.
 *
 * This is the same defect the lane cites for disabling QMD persistence, that
 * revocation cannot be guaranteed after an external meeting-policy change.
 * Closing it needs provenance the writer cannot choose, which does not exist
 * today, so the honest answer remains unavailable.
 *
 * Insights are different in exactly the way that matters: `source_meeting` is
 * written by the pipeline as the path it processed, not by a caller.
 */
export const MCP_AGENT_ANNOTATIONS_UNAVAILABLE_DESCRIPTION =
  "Compatibility name only: unavailable in MCP because an annotation's source pointer and body are both author-supplied, so revalidating the pointer cannot bound what the body discloses.";
export const MCP_MEETING_INSIGHTS_DESCRIPTION =
  "Query structured insights extracted from meetings, decisions, commitments and questions with confidence levels. Each insight records a path to the meeting it was derived from; that path is resolved to a meeting in the live corpus, and the resolved meeting is re-read from disk and re-verified against the live sensitivity policy before the insight is released. A path that cannot be resolved into this corpus is withheld. Released records carry the source title and path as they were recorded, which may name a location outside this corpus. Withheld records are reported as a partial view rather than an empty one.";

function unavailableDerivedRecordResult(message: string) {
  return {
    content: [{ type: "text" as const, text: message }],
    structuredContent: {
      available: false,
      error: {
        code: "source-policy-provenance-required",
        message,
      },
    },
    isError: true,
  };
}

/** Why a derived record could not be released to an agent surface. */
export type WithheldSourceReason =
  | "no-source-provenance"
  | "source-policy-denied";

/**
 * Resolve a derived record's recorded source pointer to a live corpus path.
 *
 * The pipeline writes `source_meeting` as the absolute path it processed, so
 * the recorded value is only meaningful on the machine and in the directory
 * layout that produced it. A corpus that has since moved — a different
 * machine, a different home directory, a restored backup — leaves every
 * historical record naming a path that does not exist here, and the exact-path
 * check then withholds the entire projection while reporting it as a policy
 * denial. Identifying the source by its path *relative to the live corpus
 * root* survives that move.
 *
 * Two shapes are accepted:
 *   - a relative value, resolved directly against the live root;
 *   - an absolute value, normalised by stripping a recognised root prefix.
 *
 * "Recognised" means the value carries exactly one path segment matching the
 * live root's own final segment, case-insensitively, and does not still exist
 * on this machine; the remainder after that segment is taken as the
 * corpus-relative tail, and that tail must name an active corpus location.
 * Nothing to the LEFT of the anchor is screened: it describes where the old
 * corpus lived, not where the record sat inside it.
 *
 * Normalisation only ever applies to a recorded path that is meaningless here.
 * If the recorded path still resolves to a real file, it is used as recorded,
 * because a path that exists is evidence that this is not a moved corpus, and
 * rebinding it would evaluate some other meeting's policy.
 *
 * This remains a heuristic with one known residual limit, which Option A — a
 * stable identifier the pipeline writes into frontmatter, plus an index —
 * removes rather than narrows, and which is tracked as its own block. State it
 * precisely, because the consequence is not merely a confusing answer:
 *
 *   A foreign path that no longer exists here and happens to contain one
 *   directory sharing the live root's final segment will bind to whatever live
 *   meeting sits at the same relative tail. If that live meeting is not
 *   restricted and the vanished one was, the insight is RELEASED under the
 *   live meeting's policy, and the released record carries the VANISHED
 *   meeting's recorded title and path, so restricted metadata travels with it.
 *
 *   State the reach honestly: the ancestors of the foreign path are not
 *   screened, so this covers paths under a trash, archive, backup-volume or
 *   dot-directory ancestor as readily as any other. What keeps it rare rather
 *   than routine is anchoring on the root segment rather than the bare
 *   filename: `/var/folders/<tmp>/output/2026-04-07-test-meeting.md` names no
 *   corpus root and resolves to nothing instead of adopting the identity of a
 *   live meeting sharing its filename. A path like
 *   `/var/folders/<tmp>/minutes-<run>/meetings/memos/x.md` does carry an
 *   anchor and does normalise; one such value exists in the real event log.
 *
 * Several shapes are withheld. Each fails closed, and each presents as the same
 * conflated withheld reason:
 *
 *   - a corpus whose final segment was renamed as well as moved, and one
 *     reached through a symlink whose realpath ends in a different segment,
 *     since the anchor is compared against the realpath's basename. The
 *     symlink case is likely the commonest unrecovered layout: a `~/meetings`
 *     pointing at an external volume recovers nothing. Option A's business;
 *     widening the anchor set is not worth doing to a function that has twice
 *     leaked by being made more permissive;
 *   - a record that genuinely sat in an inactive or hidden part of its own
 *     corpus, since the tail carries that component over and the active-corpus
 *     check refuses it. A corpus that merely LIVED under such a directory is
 *     recognised normally, because only the tail is screened;
 *   - a corpus moved between Windows and POSIX. Here nothing is refused: `\`
 *     is a separator only on Windows, so a recorded `C:\Users\me\meetings\x.md`
 *     read on POSIX is not absolute, takes the relative branch, and yields the
 *     candidate `<root>/C:\Users\me\meetings\x.md`. Backslash and colon are
 *     legal POSIX filename characters, so that is a real path rather than an
 *     impossible one; it withholds because no such file exists, not because
 *     anything rejected it. Someone able to create a file with that literal
 *     name inside the corpus could make it bind, which needs corpus write
 *     access and is therefore not the weakest link.
 *
 * One limit is INHERENT to identifying a meeting by path, and only Option A
 * removes it: a path is a mutable locator, not an identity. If the recorded
 * file is deleted and an unrelated meeting later takes its name, or a symlink
 * is retargeted, the re-read validates whatever now occupies the locator and
 * releases the old record under that file's policy. Calling this "revalidating
 * the source" overstates it; what is revalidated is the current occupant.
 *
 * A second widening is OPTIONAL and is kept deliberately rather than because it
 * is forced: a relative recorded value is accepted although today's pipeline
 * only ever writes absolute paths. It is retained because the anchored branch
 * already grants the same reach to any crafted absolute value, so refusing
 * relative closes no distinct hole, and because a corrupt event log is outside
 * this surface's threat model. If a versioned producer ever emits an explicit
 * identifier, that should be its own field rather than a reinterpreted path,
 * and this branch should go.
 *
 * Anchor matching is case-insensitive, which cuts both ways and is stated here
 * because only one direction is obvious. It refuses a case-variant second
 * anchor that exact matching would have missed, and it also RECOGNISES
 * `/nope/MEETINGS/x.md` on a case-sensitive filesystem, where `MEETINGS` is a
 * genuinely different directory. The widening is deliberate: on the
 * case-insensitive filesystems this corpus mostly lives on, the variant spelling
 * names the same directory.
 *
 * Returns null when no corpus-relative identity can be established. Callers
 * must treat null as withheld. Binding a record to the wrong meeting would
 * release it under the wrong meeting's policy, and this lane's standing rule
 * is that a wrong binding is worse than none.
 */
/**
 * Whether a recorded path can be PROVEN absent from this machine.
 *
 * `existsSync` is the wrong instrument: it answers false for every stat
 * failure, so "I am not permitted to look" is indistinguishable from "it is not
 * there". That distinction is load-bearing here, because treating an
 * unreadable path as absent hands it to the normaliser and reopens the
 * duplicate-corpus rebinding this guard exists to prevent. The unreadable case
 * is not exotic: another user's home on a shared Mac is `drwxr-x---`, and a
 * desktop-spawned MCP server without Full Disk Access cannot stat `~/Documents`
 * or a Time Machine volume at all.
 *
 * Only a clean "no entry" counts as absence. Every other outcome, including
 * EACCES, ELOOP and EIO, means the question could not be answered and the
 * recorded path is treated as still present.
 */
function recordedPathIsProvablyAbsent(recordedPath: string): boolean {
  try {
    return statSync(recordedPath, { throwIfNoEntry: false }) === undefined;
  } catch {
    return false;
  }
}

export function resolveCorpusRelativeSourcePath(
  value: unknown,
  meetingsDir: string
): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (trimmed === "" || trimmed.includes("\0")) return null;
  // Surrounding whitespace is legal in POSIX and macOS filenames, so trimming
  // can turn one real file's path into another real file's path: a recorded
  // `<root>/notes.md ` would bind to `<root>/notes.md`, a different meeting
  // judged under different policy. Trim only to detect emptiness; if it changed
  // anything, refuse rather than guess which file was meant.
  if (trimmed !== value) return null;
  // Refuse `.` and `..` components before anything normalises them away.
  //
  // `join` and `resolve` cancel `..` LEXICALLY, which is not what the kernel
  // does. Measured on Linux: `missing/../board.md` fails ENOENT and
  // `afile/../board.md` fails ENOTDIR, so neither denotes any file at all,
  // while `alink/../board.md` where `alink` is a symlink out of the corpus
  // opens a file OUTSIDE it. Lexical cancellation turns all three into
  // `<root>/board.md`, binding a record to a meeting its recorded path never
  // named, and in the symlink case laundering an out-of-corpus file into the
  // corpus behind the active-corpus check. Splitting on both separators here
  // over-refuses on POSIX, which is the safe direction.
  if (trimmed.split(/[\\/]+/).some((part) => part === "." || part === "..")) {
    return null;
  }

  // Every filesystem touch below can throw: `canonicalizeRoot` calls
  // `realpathSync`, and a record whose source is unlinked between the stat and
  // the realpath would otherwise throw out of the tool handler, which returns
  // the raw message, and with it the recorded source path, to the agent.
  try {
    return resolveCorpusRelativeSourcePathInner(trimmed, meetingsDir);
  } catch {
    return null;
  }
}

function resolveCorpusRelativeSourcePathInner(
  trimmed: string,
  meetingsDir: string
): string | null {
  const canonicalRoot = canonicalizeRoot(meetingsDir);
  const rootSegment = basename(canonicalRoot);

  let candidate: string;
  if (!isAbsolute(trimmed)) {
    candidate = join(canonicalRoot, trimmed);
  } else if (isPathWithinCanonicalRoot(trimmed, canonicalRoot)) {
    // Already names this corpus: keep the recorded path as written.
    candidate = canonicalizeRoot(trimmed);
  } else if (!recordedPathIsProvablyAbsent(trimmed)) {
    // The recorded path may still name a real file here, so this is not the
    // moved-corpus case normalisation exists for. Rebinding it to a same-named
    // file under the live root would evaluate a DIFFERENT meeting's policy: a
    // restored or duplicated corpus has the same relative tails as the live one
    // and may carry different `sensitivity:` frontmatter, so the tail lookup
    // could release a restricted meeting's insight under an unrestricted
    // namesake. Keep exact semantics instead, which withholds anything outside
    // the live active corpus.
    candidate = canonicalizeRoot(trimmed);
  } else {
    // `\` is a separator only where it is one. Splitting on it unconditionally
    // re-segments a POSIX filename that legitimately contains a backslash, and
    // binds `<root>/sub\x.md` to `<root>/sub/x.md`, a different meeting.
    const segments = trimmed
      .split(process.platform === "win32" ? /[\\/]+/ : /\/+/)
      .filter((segment) => segment !== "");
    // Anchors are matched case-insensitively for the ambiguity count. Exact-case
    // matching would see one anchor in `/elsewhere/Meetings/meetings/x.md` and
    // bind the inner tail, and on a case-insensitive filesystem `Meetings` is an
    // ordinary spelling of the same directory.
    const foldedRoot = rootSegment.toLowerCase();
    const anchors =
      rootSegment === ""
        ? []
        : segments.reduce<number[]>((found, segment, index) => {
            if (segment.toLowerCase() === foldedRoot) found.push(index);
            return found;
          }, []);
    // Exactly one anchor, or the tail is ambiguous. With two, the last-anchor
    // rule silently prefers the inner one: a recorded
    // `/elsewhere/meetings/meetings/x.md` would bind to `<root>/x.md` rather
    // than `<root>/meetings/x.md`, which is a different meeting.
    if (anchors.length !== 1) return null;
    const anchor = anchors[0];
    // An anchor as the final segment names the root itself rather than a
    // meeting, and produces an empty tail. No explicit guard for that here: the
    // empty tail joins to the root, and `isActiveCorpusMeetingPath` refuses a
    // candidate equal to the root. A guard here would be unreachable, and an
    // unreachable guard invites a comment claiming a check that never runs.
    // Only the tail is carried over, and only the tail decides whether the
    // record sat in an active part of its own corpus: everything left of the
    // anchor describes where that corpus lived, not where the record sat
    // inside it. The `isActiveCorpusMeetingPath` check below sees exactly this
    // tail under the live root, so an archived, hidden or traversing tail is
    // refused there. Screening the discarded left-hand side as well would
    // reject a perfectly ordinary corpus that had lived under `~/Archive/` or
    // `~/.local/share/` without closing anything, and a record whose corpus
    // path re-enters through a SECOND anchor is already refused above as
    // ambiguous.
    candidate = join(canonicalRoot, ...segments.slice(anchor + 1));
  }

  // This is confinement, not identity. It proves whatever the candidate denotes
  // sits inside the live root and outside every inactive or hidden directory.
  // It cannot prove the candidate preserves what the RECORDED path denoted,
  // which is why `..` is refused above rather than relied on being caught here.
  return isActiveCorpusMeetingPath(candidate, canonicalRoot) ? candidate : null;
}

/**
 * Re-verify one derived record's source meeting against live on-disk policy.
 *
 * This is the "canonical source policy provenance that can be revalidated
 * live" requirement in concrete form. Provenance recorded when the annotation
 * was written is never trusted on its own: the source meeting is re-read from
 * disk now, re-parsed, confirmed to sit inside the active corpus, and checked
 * against the current sensitivity designation. A meeting that has since been
 * marked restricted, moved out of the corpus, deleted, or had its frontmatter
 * corrupted therefore withholds its annotations on this read, not on the next
 * restart.
 *
 * A record with no resolvable source fails closed. Without a source there is
 * nothing to revalidate, and an annotation may quote restricted content.
 *
 * The returned `reason` is informational and is NOT what keeps the published
 * tally coarse. `releaseRecordsWithLiveSourcePolicy` discards it, caching only
 * the boolean and counting every refusal into `sourcePolicyDenied`, so that
 * bucket already conflates "could not be resolved into this corpus" with
 * "resolved and refused". That conflation is deliberate, because separating
 * them would publish the number of restricted source meetings in the window as
 * a clean count to a caller holding no override, but it is enforced at the
 * caller, not here. `no-source-provenance` is counted separately there, and is
 * safe to report because it describes the record's own shape rather than any
 * meeting's policy. Consequently the `source-policy-denied` value returned
 * below is never distinguishable in output; it exists so the two refusal paths
 * read differently at this call site and can be asserted in tests.
 */
export async function revalidateDerivedRecordSource(
  meetingPath: unknown,
  meetingsDir: string,
  includeRestricted: boolean
): Promise<{ allowed: true } | { allowed: false; reason: WithheldSourceReason }> {
  if (typeof meetingPath !== "string" || meetingPath.trim() === "") {
    return { allowed: false, reason: "no-source-provenance" };
  }
  const resolved = resolveCorpusRelativeSourcePath(meetingPath, meetingsDir);
  if (resolved === null) {
    return { allowed: false, reason: "source-policy-denied" };
  }
  const snapshot = await policyVerifiedExactMeetingSnapshot(
    resolved,
    meetingsDir,
    includeRestricted
  );
  return snapshot
    ? { allowed: true }
    : { allowed: false, reason: "source-policy-denied" };
}

/**
 * Filter annotations to those whose source survives live revalidation.
 *
 * Returns the withheld tally alongside the released records. The lane's
 * standing rule is that an agent surface must distinguish "unavailable" from
 * "empty", so a partial view is always reported as partial.
 */
export async function releaseRecordsWithLiveSourcePolicy(
  records: any[],
  selectSourcePath: (record: any) => unknown,
  meetingsDir: string,
  includeRestricted: boolean
): Promise<{
  released: any[];
  withheld: { total: number; noSourceProvenance: number; sourcePolicyDenied: number };
}> {
  const released: any[] = [];
  const withheld = { total: 0, noSourceProvenance: 0, sourcePolicyDenied: 0 };
  // Sources repeat heavily across records from the same meeting; one verdict
  // per distinct path keeps a large read from restatting the same file.
  const verdicts = new Map<string, boolean>();
  for (const record of records) {
    const source = selectSourcePath(record);
    if (typeof source !== "string" || source.trim() === "") {
      withheld.total += 1;
      withheld.noSourceProvenance += 1;
      continue;
    }
    let allowed = verdicts.get(source);
    if (allowed === undefined) {
      const verdict = await revalidateDerivedRecordSource(
        source,
        meetingsDir,
        includeRestricted
      );
      allowed = verdict.allowed;
      verdicts.set(source, allowed);
    }
    if (allowed) {
      released.push(record);
    } else {
      withheld.total += 1;
      withheld.sourcePolicyDenied += 1;
    }
  }
  return { released, withheld };
}

/**
 * Insights carry `source_meeting`, the path to the markdown they were derived
 * from, so they revalidate through exactly the same live policy check.
 */
/** Confidence ladder, lowest first, matching the CLI's ordering. */
const INSIGHT_CONFIDENCE_ORDER = ["tentative", "inferred", "strong", "explicit"] as const;

/**
 * Whether an insight meets a minimum confidence floor.
 *
 * Applied in this process rather than by the CLI so that policy revalidation
 * runs before any caller-controlled selector, which is what keeps the withheld
 * tally from becoming an oracle.
 */
export function meetsInsightConfidence(observed: unknown, minimum: unknown): boolean {
  const observedIndex = INSIGHT_CONFIDENCE_ORDER.indexOf(observed as never);
  const minimumIndex = INSIGHT_CONFIDENCE_ORDER.indexOf(minimum as never);
  if (minimumIndex < 0) return true;
  if (observedIndex < 0) return false;
  return observedIndex >= minimumIndex;
}

const INSIGHT_SINCE_ERROR = "insight since must be a calendar date in YYYY-MM-DD form";

/**
 * Parse the `since` floor the way `minutes insights` does: local midnight of
 * the given calendar date.
 *
 * Applied in this process rather than by the CLI. `since` is a window-shaping
 * argument, and any caller-shaped window makes the withheld tally differenceable
 * on that axis exactly as `limit` was. With this in process, no caller-supplied
 * value reaches the CLI at all.
 *
 * A malformed value is refused rather than ignored. The CLI warns on stderr and
 * then shows everything, which through this surface would silently widen a
 * query the caller believed was narrowed.
 *
 * One deliberate divergence: on a date whose local midnight does not exist,
 * `chrono`'s `single()` returns None and the CLI drops the `since` filter
 * altogether, while this applies the floor Date rolls forward to. Narrowing
 * where the CLI widens is the safe direction for a policy surface.
 */
export function parseInsightSinceFloor(since: unknown): number | null {
  if (since === undefined || since === null) return null;
  if (typeof since !== "string") {
    throw new McpError(ErrorCode.InvalidParams, INSIGHT_SINCE_ERROR);
  }
  const trimmed = since.trim();
  if (trimmed === "") return null;
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(trimmed);
  if (!match) {
    throw new McpError(ErrorCode.InvalidParams, INSIGHT_SINCE_ERROR);
  }
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const midnight = new Date(year, month - 1, day, 0, 0, 0, 0);
  // Rejects 2026-02-30 and two-digit-year coercion, both of which Date rolls
  // over silently.
  if (
    midnight.getFullYear() !== year ||
    midnight.getMonth() !== month - 1 ||
    midnight.getDate() !== day
  ) {
    throw new McpError(ErrorCode.InvalidParams, INSIGHT_SINCE_ERROR);
  }
  return midnight.getTime();
}

/**
 * Whether one insight falls on or after the requested floor.
 *
 * A record this process cannot date is excluded whenever a floor was asked
 * for: answering a time-bounded question with a record of unknown time would
 * be a claim the data does not support.
 */
export function insightIsSince(insight: any, floor: number | null): boolean {
  if (floor === null) return true;
  const raw = insight?.timestamp;
  if (typeof raw !== "string") return false;
  const at = Date.parse(raw);
  return Number.isFinite(at) && at >= floor;
}

/** Case-insensitive partial match over participants and owner, as the CLI does. */
export function insightMentionsParticipant(insight: any, participant: string): boolean {
  const needle = participant.toLowerCase();
  if (!needle) return true;
  const owner = typeof insight?.owner === "string" ? insight.owner : "";
  if (owner.toLowerCase().includes(needle)) return true;
  const participants = Array.isArray(insight?.participants) ? insight.participants : [];
  return participants.some(
    (value: unknown) => typeof value === "string" && value.toLowerCase().includes(needle)
  );
}

export function releaseInsightsWithLiveSourcePolicy(
  insights: any[],
  meetingsDir: string,
  includeRestricted: boolean
) {
  return releaseRecordsWithLiveSourcePolicy(
    insights,
    (insight) => insight?.source_meeting,
    meetingsDir,
    includeRestricted
  );
}

/**
 * Injection points for the insight handler, defaulted to the live ones.
 *
 * The same handler body runs in production and under test; only these four
 * bindings differ. `MINUTES_BIN` is chosen from a fixed candidate list with no
 * environment override (it is reassigned only by the auto-install path), so
 * without this seam a test could not put known records in front of the handler
 * and assert the argv it builds.
 *
 * `readiness` is here because insights are a content-bearing agent tool, so
 * every call is routed through the trust bridge before the handler body runs,
 * and the live bridge shells out to the CLI. Without an override the handler
 * tests would need a built `minutes` binary and a healthy registry, which CI
 * has neither of: the `mcp` job runs `npm ci` and vitest with no cargo build.
 * They would fail there rather than pass vacuously, but they would fail.
 */
export type InsightToolDeps = {
  runner?: MinutesRunner;
  cliAvailable?: () => Promise<boolean>;
  meetingsDir?: () => Promise<string>;
  readiness?: () => Promise<unknown>;
};

/**
 * Resolve the live bindings for anything the caller did not override.
 *
 * Exported so a test can assert that an un-overridden registration really does
 * bind the production functions. Nothing else asserts that, and a default
 * silently rebound to a stub would otherwise ship green as a tool that returns
 * nothing or claims the CLI is missing forever.
 */
export function resolveInsightToolDeps(deps: InsightToolDeps = {}) {
  return {
    runCli: deps.runner ?? runMinutes,
    cliIsAvailable: deps.cliAvailable ?? isCliAvailable,
    resolveMeetingsDir: deps.meetingsDir ?? getEffectiveMeetingsDir,
    readiness: deps.readiness ?? requireAgentTrustReadiness,
  };
}

export function registerUnavailableCompatibilityTools(
  serverArg: McpServer,
  deps: InsightToolDeps = {}
) {
  const { runCli, cliIsAvailable, resolveMeetingsDir, readiness } =
    resolveInsightToolDeps(deps);

  registerToolWithRestrictedPolicy(
    serverArg,
    "get_agent_annotations",
    MCP_AGENT_ANNOTATIONS_UNAVAILABLE_DESCRIPTION,
    {
      limit: z.number().optional().default(50).describe("Compatibility argument; the tool is unavailable"),
      agent_id: z.string().optional().describe("Compatibility argument; the tool is unavailable"),
      meeting_id: z.string().optional().describe("Compatibility argument; the tool is unavailable"),
      meeting_path: z.string().optional().describe("Compatibility argument; the tool is unavailable"),
    },
    { title: "Get Agent Annotations (Unavailable)", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    async () => unavailableDerivedRecordResult(
      "Agent annotations are unavailable to MCP. An annotation's source pointer and its body are both written by the annotation's author, so revalidating the pointer cannot bound what the body discloses."
    )
  );

  registerToolWithRestrictedPolicy(
    serverArg,
    "get_meeting_insights",
    MCP_MEETING_INSIGHTS_DESCRIPTION,
    {
      kind: z.enum(MEETING_INSIGHT_KINDS).optional().describe("Filter by insight type"),
      confidence: z.enum(["tentative", "inferred", "strong", "explicit"]).optional().describe("Minimum confidence level"),
      participant: z.string().optional().describe("Filter by participant name (partial match)"),
      since: z.string().optional().describe("Only insights on or after this calendar date (YYYY-MM-DD)"),
      limit: z
        .number()
        .int()
        .min(1)
        .max(MCP_INSIGHT_RESULT_MAX)
        .optional()
        .default(50)
        .describe(
          `Maximum number of results to return (1-${MCP_INSIGHT_RESULT_MAX}). Every read examines the newest ${MCP_INSIGHT_SCAN_WINDOW} records regardless; this caps the answer, not the search.`
        ),
      actionable_only: z.boolean().optional().default(false).describe("Only return actionable insights (Strong or Explicit confidence)"),
      include_restricted: z
        .boolean()
        .optional()
        .default(false)
        .describe(
          "Include insights whose source meeting is designated sensitivity: restricted. Requires the server to have been launched with MINUTES_MCP_RESTRICTED_POLICY=logged-override; the request is durably audited. This does not recover insights withheld for any other reason, such as a source that was archived or deleted, or one whose recorded path cannot be resolved to a meeting in this corpus."
        ),
    },
    { title: "Get Meeting Insights", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    async ({ kind, confidence, participant, since, limit, actionable_only, include_restricted }: any) => {
      if (!(await cliIsAvailable())) {
        return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }], isError: true };
      }

      const requested = normalizeMcpResultLimit(limit ?? 50, MCP_INSIGHT_RESULT_MAX, "insight");
      const sinceFloor = parseInsightSinceFloor(since);
      // No caller-supplied value reaches the CLI. Every selector runs in this
      // process, after policy revalidation, so the withheld tally is computed
      // over a window the caller cannot shape.
      //
      // Content selectors moved here first, because computing the tally after
      // a caller's participant or kind filter let an agent with no content
      // access learn who attended a restricted meeting by sweeping the filter.
      // `limit` and `since` are here for the same reason: both shape the
      // window, and a tally over a caller-shaped window can be differenced
      // across two calls to read one record's policy verdict.
      const args = ["insights", "--limit", String(MCP_INSIGHT_SCAN_WINDOW)];

      let fetched: unknown[];
      try {
        const { stdout } = await runCli(args, 30000);
        const parsed = parseJsonOutput(stdout);
        if (!Array.isArray(parsed)) {
          // Never coerce unparseable output to an empty array: reporting
          // `partial: false` for it would be an affirmative claim of
          // completeness built on output we could not read.
          throw new Error("Minutes returned an unreadable insight projection");
        }
        // Enforce the window here rather than trusting the CLI to honour
        // `--limit`. Both notes below state the window as fact, so their truth
        // should not depend on a cross-language assumption: an over-long
        // projection would otherwise make the capped note promise a `limit` the
        // schema will not accept.
        fetched =
          parsed.length > MCP_INSIGHT_SCAN_WINDOW
            ? parsed.slice(parsed.length - MCP_INSIGHT_SCAN_WINDOW)
            : parsed;
      } catch {
        // The raw error can carry the CLI's stdout, which for this command is
        // the unfiltered insight projection, and would bypass revalidation
        // entirely through the error channel. Never echo it.
        return {
          content: [{
            type: "text" as const,
            text: "Failed to query insights. Run `minutes insights` directly to see the underlying error.",
          }],
          isError: true,
        };
      }

      // Each insight names the markdown the pipeline derived it from. That
      // source is re-read and re-checked against live policy before release.
      const meetingsDir = await resolveMeetingsDir();
      const { released, withheld } = await releaseInsightsWithLiveSourcePolicy(
        fetched,
        meetingsDir,
        include_restricted === true
      );

      const minimumConfidence = actionable_only ? "strong" : confidence;
      const selected = released.filter((insight: any) => {
        if (!insightIsSince(insight, sinceFloor)) return false;
        if (kind && insight?.kind !== kind) return false;
        if (minimumConfidence && !meetsInsightConfidence(insight?.confidence, minimumConfidence)) {
          return false;
        }
        if (participant && !insightMentionsParticipant(insight, String(participant))) return false;
        return true;
      });

      // `limit` caps the answer, not the search. Sizing the scanned window with
      // it was what cost a filtered query its reach: asking for one
      // participant's commitments examined only the newest `limit` records and
      // reported whatever few of them matched. The window is now fixed, so a
      // narrow filter sees all of it.
      //
      // The CLI emits oldest-first and takes its own limit from the tail, so
      // keep the newest matches and preserve that order.
      const matching =
        selected.length > requested
          ? selected.slice(selected.length - requested)
          : selected;

      // `partial` must account for truncation as well as policy. The previous
      // shape reported `partial: false` on a truncated window, which let an
      // agent answer "there are no decisions about X" from an incomplete read.
      const truncated = fetched.length >= MCP_INSIGHT_SCAN_WINDOW;
      const capped = selected.length > requested;
      const partial = withheld.total > 0 || truncated || capped;

      const notes: string[] = [];
      if (withheld.total > 0) {
        // Covers every bucket the tally counts, including records carrying no
        // source pointer at all. Naming only the policy case would misdescribe
        // those.
        // Says what the number counts. The tally is computed over the whole
        // scanned window before any caller filter, which is what stops it being
        // differenceable, and the cost of that is it bears no relation to the
        // caller's query: someone asking for one participant since May is still
        // told about every unreleasable record in the window. Reporting it as a
        // query-scoped count would be the false reading.
        notes.push(
          `Of the ${fetched.length} most recent record(s) examined, ${withheld.total} could not be released, independently of the filters in this request: the record names no source meeting, or its source could not be resolved to a meeting in the active corpus, or that meeting is designated restricted, archived, or deleted.`
        );
      }
      if (truncated) {
        // Deliberately offers no remedy. `since` is a lower bound, so no
        // argument this tool accepts reaches past the newest scanned record;
        // saying otherwise would advertise a recovery that cannot happen.
        // Hedged on whether older records exist: this compares the returned
        // count against the window, which cannot distinguish a log of exactly
        // the window size from a larger one.
        notes.push(
          `This view examined the newest ${MCP_INSIGHT_SCAN_WINDOW} record(s); any older ones were not examined, and are not reachable through this tool.`
        );
      }
      if (capped) {
        notes.push(
          `${selected.length} releasable record(s) matched; showing the most recent ${requested}. Raise limit for the rest.`
        );
      }
      const suffix = notes.length === 0 ? "" : `\n\n${notes.join("\n")}`;

      if (matching.length === 0) {
        return {
          content: [{
            type: "text" as const,
            // "Releasable" is load-bearing. Filters run only over records that
            // already survived policy, so when everything was withheld the
            // filter was never evaluated against them and this cannot claim
            // they failed to match. Saying "nothing matched your filter" there
            // would hand the agent a reason the code never established.
            // The "not filter-tested" caveat belongs only where records were
            // actually withheld. Emitting it on a genuinely empty result makes
            // a complete answer sound partial, which is the same class of false
            // statement in the other direction.
            text: `No releasable meeting insights matched the filter criteria.${
              withheld.total > 0
                ? " Records withheld by policy are not filter-tested, so this is not evidence that no such insight exists."
                : ""
            } Insights are extracted when meetings are processed with summarization enabled.${suffix}`,
          }],
          structuredContent: {
            available: true,
            count: 0,
            matched: selected.length,
            insights: [],
            withheld,
            truncated,
            capped,
            partial,
          },
        };
      }

      return {
        content: [{
          type: "text" as const,
          text: `Found ${matching.length} insight(s):\n\n${JSON.stringify(matching, null, 2)}${suffix}`,
        }],
        structuredContent: {
          available: true,
          count: matching.length,
          matched: selected.length,
          insights: matching,
          withheld,
          truncated,
          capped,
          partial,
        },
      };
    },
    readiness
  );
}

forEachServer((target) => {
  registerUnavailableCompatibilityTools(target);
});

// ── Tools: real-time copilot control + observation ──────────

if (COPILOT_SUPPORTED) {
  registerTool(
    "start_copilot",
    "Start the independent Minutes real-time copilot for a goal. This launches `minutes copilot start`; MCP does not run model inference, own capture, or need to stay connected for the engine to produce nudges. The stdout surface is recommended for complete structured nudge observation.",
    {
      goal: z
        .string()
        .trim()
        .min(1)
        .max(1000)
        .describe("Outcome the copilot should help achieve, for example 'land owners and deadlines'."),
      surface: z
        .enum(["stdout", "tui"])
        .optional()
        .default("stdout")
        .describe("CLI presentation surface. Use stdout for complete structured nudge reads; tui is compact text."),
    },
    { title: "Start Copilot", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
    async ({ goal, surface }) => {
      if (!(await isCliAvailable())) {
        return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
      }

      const before = await readCopilotStatusFromCli();
      if (!before.available) {
        return {
          content: [{ type: "text" as const, text: before.error || "Copilot status is unavailable." }],
          structuredContent: { status: before },
          isError: true,
        };
      }
      if (before.active) {
        const snapshot = buildLiveCopilotResourcePayload(
          before,
          await readCopilotNudgeObservation(before)
        );
        return {
          content: [{
            type: "text" as const,
            text: `Copilot is already active (${before.state}). Use copilot_status or stop_copilot before starting a different session.`,
          }],
          structuredContent: snapshot,
        };
      }

      try {
        const observerSession = await spawnCopilotCli(goal, surface);
        const status = await waitForCopilotStatus(
          (candidate) => candidate.active || !processIsAlive(observerSession.pid),
          5000
        );
        const observation = await readCopilotNudgeObservation(status);
        const snapshot = buildLiveCopilotResourcePayload(status, observation);

        if (status.active && observerMatchesStatus(observerSession, status)) {
          return {
            content: [{
              type: "text" as const,
              text:
                `Copilot started (${status.state}) for goal: ${goal}. ` +
                "The engine is an independent minutes CLI process; use read_copilot_nudges or minutes://live/copilot to observe it.",
            }],
            structuredContent: snapshot,
          };
        }

        if (status.active) {
          return {
            content: [{
              type: "text" as const,
              text:
                "A copilot session became active, but it belongs to a different process. " +
                "No second engine was attached; use copilot_status to inspect the active session.",
            }],
            structuredContent: snapshot,
          };
        }

        if (processIsAlive(observerSession.pid)) {
          return {
            content: [{
              type: "text" as const,
              text:
                "Copilot start is still arming and has not published active status yet. " +
                "The independent CLI process is running; use copilot_status to follow startup.",
            }],
            structuredContent: snapshot,
          };
        }

        const stderr = await readCopilotStderrTail();
        return {
          content: [{
            type: "text" as const,
            text: `Copilot failed to start${stderr ? `: ${stderr}` : ". Check Minutes configuration and the local Ollama provider."}`,
          }],
          structuredContent: snapshot,
          isError: true,
        };
      } catch (error: unknown) {
        return {
          content: [{
            type: "text" as const,
            text: `Failed to start copilot: ${error instanceof Error ? error.message : String(error)}`,
          }],
          isError: true,
        };
      }
    }
  );

  registerTool(
    "stop_copilot",
    "Request that the active Minutes copilot stop. This invokes `minutes copilot stop` and never stops recording or live transcription. Calling it while no copilot is active returns a calm not-active status.",
    {},
    { title: "Stop Copilot", readOnlyHint: false, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    async () => {
      if (!(await isCliAvailable())) {
        return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
      }

      try {
        const stopped = await stopCopilotBeforeStatusRead();
        if (!stopped.mayRevealContent) {
          return {
            content: [{
              type: "text" as const,
              text: "Copilot stop requested. Status is withheld until the agent trust boundary is ready.",
            }],
          };
        }
        const status = stopped.status;
        if (!status.active) {
          await rm(copilotObserverPaths().session, { force: true }).catch(() => {});
        }
        const snapshot = buildLiveCopilotResourcePayload(
          status,
          await readCopilotNudgeObservation(status)
        );
        return {
          content: [{
            type: "text" as const,
            text: status.active
              ? "Copilot stop requested; the engine is still shutting down. Capture continues unchanged."
              : "Copilot stopped. Recording and live transcription were not changed.",
          }],
          structuredContent: snapshot,
        };
      } catch {
        return {
          content: [{
            type: "text" as const,
            text: "Failed to request copilot stop safely.",
          }],
          isError: true,
        };
      }
    }
  );

  registerTool(
    "copilot_status",
    "Read the current operational copilot state from the strict `minutes copilot status --json` contract. This is observation only and returns a clear Off/not-active payload when no engine is running.",
    {},
    { title: "Copilot Status", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    async () => {
      const snapshot = await getLiveCopilotSnapshot();
      if (!snapshot.available) {
        return {
          content: [{
            type: "text" as const,
            text: snapshot.status.error || "Copilot status is unavailable.",
          }],
          structuredContent: snapshot,
          isError: true,
        };
      }
      return {
        content: [{
          type: "text" as const,
          text: snapshot.active
            ? `Copilot is ${snapshot.state}. ${snapshot.nudge_stream.note}`
            : "Copilot is not active (Off).",
        }],
        structuredContent: snapshot,
      };
    }
  );

  registerTool(
    "read_copilot_nudges",
    "Read nudges observed from the active real CLI copilot stream. Use cursor (or a numeric since value) for lossless delta reads; since also accepts durations such as '5m'/'30s' and ISO timestamps. MCP never generates nudges itself.",
    {
      cursor: z
        .number()
        .int()
        .min(0)
        .optional()
        .describe("Return nudge lines after this zero-based observation cursor."),
      since: z
        .string()
        .trim()
        .min(1)
        .max(64)
        .optional()
        .describe("Cursor string, duration such as '5m' or '30s', or ISO timestamp. Do not combine with cursor."),
      limit: z
        .number()
        .int()
        .min(1)
        .max(COPILOT_READ_MAX_LIMIT)
        .optional()
        .default(COPILOT_READ_DEFAULT_LIMIT)
        .describe("Maximum nudges to return (1-200)."),
    },
    { title: "Read Copilot Nudges", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
    async ({ cursor, since, limit }) => {
      const status = await readCopilotStatusFromCli();
      if (!status.active) {
        return {
          content: [{
            type: "text" as const,
            text: "Copilot is not active. Start it with start_copilot before waiting for new nudges.",
          }],
          structuredContent: {
            active: false,
            state: status.state,
            cursor: 0,
            next_cursor: 0,
            cursor_reset: false,
            has_more: false,
            nudges: [],
          },
        };
      }

      await requireAgentTrustReadiness();
      const observation = await readCopilotNudgeObservation(status);
      if (!observation.attached) {
        return {
          content: [{ type: "text" as const, text: observation.note }],
          structuredContent: {
            active: true,
            state: status.state,
            attached: false,
            cursor: 0,
            next_cursor: 0,
            cursor_reset: false,
            has_more: false,
            nudges: [],
          },
        };
      }

      try {
        const page = selectCopilotNudges(observation, { cursor, since, limit });
        const text = page.nudges.length > 0
          ? JSON.stringify(page, null, 2)
          : `Copilot is ${status.state}; no new nudges after cursor ${page.next_cursor}.`;
        return {
          content: [{ type: "text" as const, text }],
          structuredContent: {
            active: true,
            state: status.state,
            attached: true,
            ...page,
          },
        };
      } catch (error: unknown) {
        return {
          content: [{
            type: "text" as const,
            text: error instanceof Error ? error.message : String(error),
          }],
          isError: true,
        };
      }
    }
  );
} else {
  crashTrace("copilot-tools-disabled", { reason: "missing copilot_realtime CLI capability" });
}

// ── Tool: start_live_transcript ──────────────────────────────

async function existingCaptureAttachmentMessage(kind: string): Promise<string> {
  try {
    const relay = await attachCaptureRelay();
    return `${kind} already owns the microphone. MCP attached securely to owner PID ${relay.discovery.owner_pid} over the local ${relay.discovery.transport === "windows_named_pipe" ? "named pipe" : "Unix socket"}; Minutes did not open a second capture. Use read_live_transcript to follow along.`;
  } catch (error: any) {
    return `${kind} already owns the microphone, but its secure attachment relay is unavailable (${error?.message ?? String(error)}). Minutes did not open a second capture. Finalized transcript lines may still be available through read_live_transcript; restart or update the capture owner to restore live attachment.`;
  }
}

registerTool(
  "start_live_transcript",
  "Start real-time transcription. If a recording is already running, it already includes a live transcript — use read_live_transcript to read it. Runs until stop is called.",
  {
    language: z.string().optional().describe("Transcription language code (e.g. 'en', 'ur', 'es', 'zh'). Overrides config.toml setting."),
  },
  { title: "Start Live Transcript", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async ({ language }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }
    // Pre-flight checks with short timeouts (these are instant file reads)
    const { stdout: statusOut } = await runMinutes(["status"], 5000);
    const status = parseJsonOutput(statusOut);
    if (status.recording) {
      return {
        content: [{ type: "text" as const, text: await existingCaptureAttachmentMessage("A recording") }],
      };
    }

    // Check if a live transcript is already running
    try {
      const { stdout: ltStatus } = await runMinutes(["transcript", "--status", "--format", "json"], 5000);
      const ltParsed = parseJsonOutput(ltStatus);
      if (ltParsed?.active) {
        return {
          content: [{ type: "text" as const, text: await existingCaptureAttachmentMessage("Live Transcript") }],
        };
      }
    } catch { /* no active session, proceed */ }

    // Extension runtime: mic won't work for spawned child processes.
    if (isExtensionRuntime) {
      return {
        content: [
          {
            type: "text" as const,
            text: "Live transcript is not yet supported via the Claude Desktop extension. " +
              "The extension runtime cannot pass microphone access to child processes.\n\n" +
              "Workaround: run `minutes live` from your terminal, or use start_recording instead " +
              "(recording delegates to the Minutes desktop app when it's running).",
          },
        ],
        isError: true,
      };
    }

    // Spawn live transcript as child (not detached — preserves macOS TCC mic grant)
    const liveArgs = ["live"];
    if (language) liveArgs.push("--language", language);
    const child = spawn(MINUTES_BIN, liveArgs, {
      stdio: "ignore",
      env: mcpCliChildEnv({ RUST_LOG: "info" }),
    });
    child.unref();

    // Verify the session actually started
    await new Promise((r) => setTimeout(r, 1000));
    try {
      const { stdout: verifyOut } = await runMinutes(["transcript", "--status", "--format", "json"], 5000);
      const verifyStatus = parseJsonOutput(verifyOut);
      if (verifyStatus?.active) {
        return {
          content: [{ type: "text" as const, text: "Live transcript started. Use read_live_transcript to read the transcript. Use minutes stop to end the session." }],
        };
      }
    } catch { /* fall through to error */ }

    return {
      content: [{ type: "text" as const, text: "Live transcript may have failed to start. Check minutes health or try again. Common causes: no microphone, whisper model not downloaded, or another session already active." }],
      isError: true,
    };
  }
);

// ── Tool: read_live_transcript ──────────────────────────────

registerTool(
  "read_live_transcript",
  "Read the live transcript — works during both recordings and live transcript sessions. Use 'since' to get new lines after a cursor (line number) or time window (e.g., '5m', '30s'). Use 'status' mode to check if a session is active.",
  {
    since: z.string().optional().describe("Line number (e.g., '42') or duration (e.g., '5m', '30s'). Omit to get all lines."),
    status_only: z.boolean().optional().default(false).describe("If true, return session status instead of transcript lines"),
    relay_cursor: z.object({
      session_id: z.string().optional(),
      transcript_seq: z.number().int().nonnegative(),
      nudge_seq: z.number().int().nonnegative(),
    }).optional().describe("Transient relay cursor returned by a previous read. Reconnects without replaying already-seen transcript or nudge frames."),
  },
  { title: "Read Live Transcript", readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async ({ since, status_only, relay_cursor }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }

    const args = ["transcript", "--format", "json"];
    if (status_only) {
      args.push("--status");
    } else if (since) {
      args.push("--since", since);
    }

    let relay: CaptureRelaySnapshot | undefined;
    try {
      relay = await attachCaptureRelay(relay_cursor as CaptureRelayCursor | undefined);
    } catch {
      // A durable-only or inactive session is still readable. start_live_transcript
      // reports attachment failures explicitly when another process owns capture.
    }

    try {
      const { stdout } = await runMinutes(args, 10000);
      // For status queries, a message is helpful. For transcript reads, empty = no new lines.
      const fallback = status_only ? "No transcript data available." : "";
      if (relay) {
        const payload = {
          durable: parseJsonOutput(stdout || fallback),
          capture_relay: {
            session_id: relay.discovery.session_id,
            owner_pid: relay.discovery.owner_pid,
            evidence_mode: relay.discovery.evidence_mode,
            cursor: relay.cursor,
            frames: relay.frames,
          },
        };
        return {
          content: [{ type: "text" as const, text: JSON.stringify(payload, null, 2) }],
        };
      }
      return {
        content: [{ type: "text" as const, text: stdout || fallback }],
      };
    } catch (error: any) {
      const msg = error?.stderr || error?.message || String(error);
      return {
        content: [{ type: "text" as const, text: `Failed to read transcript: ${msg}` }],
        isError: true,
      };
    }
  }
);

// ── Tool: ingest_meeting ────────────────────────────────────

registerTool(
  "ingest_meeting",
  "Extract facts from a meeting and update the knowledge base (person profiles, log, index). Requires [knowledge] to be configured in config.toml. Uses structured frontmatter data only by default (zero hallucination risk). Set engine to 'agent' for richer LLM-based extraction.",
  {
    path: z.string().optional().describe("Path to a specific policy-authorized normal meeting .md file. Omit to process all normal meetings."),
    all: z.boolean().optional().default(false).describe("Process all policy-authorized normal meetings in the output directory"),
    dry_run: z.boolean().optional().default(false).describe("Show what would be extracted without writing anything"),
  },
  { title: "Ingest Meeting", readOnlyHint: false, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async ({ path, all, dry_run }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }

    try {
      const args = ["ingest"];
      if (path) {
        const meetingsDir = await getEffectiveMeetingsDir();
        const resolved = validatePathInDirectory(path, meetingsDir, [".md"]);
        if (!isActiveCorpusMeetingPath(resolved, meetingsDir)) {
          throw new Error("Meeting is outside the active corpus.");
        }
        args.push(resolved);
      }
      if (all) args.push("--all");
      if (dry_run) args.push("--dry-run");
      if (!path && !all) {
        throw new Error("No meeting selection was provided.");
      }
      const { stdout, stderr } = await runMinutes(args);
      const output = stderr || stdout;
      return { content: [{ type: "text" as const, text: output }] };
    } catch {
      return {
        content: [
          {
            type: "text" as const,
            text: "Knowledge ingestion could not be safely completed.",
          },
        ],
        isError: true,
      };
    }
  }
);

// ── Tool: resummarize_meeting ───────────────────────────────

registerTool(
  "resummarize_meeting",
  "Re-run the AI pass (summary, action items, decisions) on an edited meeting or memo. Preview by default: returns the regenerated content WITHOUT writing — note the summarization model IS still invoked (cost applies even in preview). Set apply=true to write the file (a timestamped backup is created and derived views — graph, search index, vault, QMD — refresh automatically). User edits outside AI-owned sections are never touched; checked action items and decision notes are carried forward by the merge.",
  {
    path: z.string().describe("Path to the meeting/memo .md file to resummarize."),
    apply: z
      .boolean()
      .optional()
      .default(false)
      .describe("Write the regenerated content (default: preview only; the model is invoked either way)."),
    engine: z
      .string()
      .optional()
      .describe("Override the summarization engine for this run (e.g. 'ollama', 'apple', 'agent'). Default: the engine from config.toml."),
    template: z
      .string()
      .optional()
      .describe("Template slug to apply for this run. Default: the template recorded in the file's frontmatter."),
    ingest: z
      .boolean()
      .optional()
      .default(false)
      .describe("With apply=true, also re-ingest the artifact into the knowledge base. Off by default because the knowledge log is append-only — every ingest adds a new log entry."),
    include_restricted: z
      .boolean()
      .optional()
      .default(false)
      .describe("Resummarize a meeting designated `sensitivity: restricted` (refused by default; the override is logged)."),
  },
  { title: "Resummarize Meeting", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
  async ({ path, apply, engine, template, ingest, include_restricted }) => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }

    let resolved: string;
    try {
      resolved = validatePathInDirectory(path, await getEffectiveMeetingsDir(), [".md"]);
    } catch (error: any) {
      return {
        content: [{ type: "text" as const, text: error?.message || String(error) }],
        isError: true,
      };
    }

    let restrictedOverride = false;
    try {
      const rawContent = await readFile(resolved, "utf-8");
      const parsed = reader.parseFrontmatter(rawContent, resolved);
      if (meetingSensitivity(parsed) === "restricted") {
        if (!include_restricted) {
          return {
            content: [
              {
                type: "text" as const,
                text: "This meeting is designated `sensitivity: restricted`; preview output would expose derived content. Pass `include_restricted: true` for an explicit, logged override.",
              },
            ],
            isError: true,
          };
        }
        console.error(
          `[Minutes] include_restricted override: resummarizing restricted meeting ${resolved} via resummarize_meeting`
        );
        restrictedOverride = true;
      }
    } catch (error: any) {
      return {
        content: [
          {
            type: "text" as const,
            text: `Could not read: ${error?.message || String(error)}`,
          },
        ],
        isError: true,
      };
    }

    const args = ["resummarize", resolved, "--json"];
    if (apply) args.push("--apply");
    if (engine) args.push("--engine", engine);
    if (template) args.push("--template", template);
    if (apply && ingest) args.push("--ingest");
    const ingestIgnored = ingest && !apply;

    try {
      const { stdout } = await runMinutes(args, 300000);
      const envelope = parseJsonOutput(stdout);
      const data = envelope?.data;

      if (data?.error) {
        return {
          content: [
            {
              type: "text" as const,
              text: formatResummarizeFailure(data, stdout),
            },
          ],
          isError: true,
        };
      }
      if (!data || typeof data !== "object") {
        return {
          content: [
            {
              type: "text" as const,
              text: `Resummarize returned an unexpected response: ${stdout}`,
            },
          ],
          isError: true,
        };
      }

      const lines = [
        apply ? `Applied: ${data.path || resolved}` : "Preview (no changes written)",
      ];
      let engineLine = `engine: ${data.engine ?? "unknown"} (model: ${data.model ?? "unknown"})`;
      if (data.template) engineLine += `, template: ${data.template}`;
      lines.push(engineLine);

      const sectionsReplaced = Array.isArray(data.sections_replaced)
        ? data.sections_replaced
        : [];
      lines.push(
        sectionsReplaced.length > 0
          ? `sections replaced: ${sectionsReplaced.join(", ")}`
          : "first AI pass"
      );

      if (Array.isArray(data.merge_notes) && data.merge_notes.length > 0) {
        lines.push("merge notes needing eyes:");
        for (const note of data.merge_notes) {
          lines.push(
            `- ${note?.kind ?? "unknown"}: ${note?.previous ?? "unknown"} — ${note?.disposition ?? "unknown"}`
          );
        }
      }

      if (apply) {
        lines.push(`backup: ${data.backup ?? "none"}`);
        if (data.derived_views) {
          const derived = data.derived_views;
          const derivedWarnings = Array.isArray(derived.warnings)
            ? derived.warnings
            : [];
          const refreshed: string[] = [];
          if (derived.graph_rebuilt) {
            refreshed.push(
              derived.meetings_indexed == null
                ? "graph"
                : `graph (${derived.meetings_indexed} meetings indexed)`
            );
          }
          const searchRefreshed =
            typeof derived.search_indexed === "boolean"
              ? derived.search_indexed
              : typeof derived.search_refreshed === "boolean"
                ? derived.search_refreshed
                : !derivedWarnings.some((warning: unknown) =>
                    String(warning).startsWith("search index:")
                  );
          if (searchRefreshed) refreshed.push("search");
          if (derived.vault_path) refreshed.push(`vault (${derived.vault_path})`);
          if (derived.qmd_refreshed) refreshed.push("qmd");
          if (derived.knowledge_ingested) {
            refreshed.push(
              derived.facts_written == null
                ? "knowledge"
                : `knowledge (${derived.facts_written} facts written)`
            );
          }
          lines.push(`refreshed views: ${refreshed.length > 0 ? refreshed.join(", ") : "none"}`);

          if (derivedWarnings.length > 0) {
            lines.push("derived view warnings:");
            for (const warning of derivedWarnings) lines.push(`- ${warning}`);
          }
        }
      } else {
        if (ingestIgnored) lines.push("--ingest ignored in preview");
        lines.push("", "--- regenerated content ---", data.new_ai_body ?? "");
      }

      const { new_ai_body: _newAiBody, ...leanData } = data;
      const structuredContent = apply ? leanData : data;
      return {
        content: [{ type: "text" as const, text: lines.join("\n") }],
        structuredContent: restrictedOverride
          ? {
              ...structuredContent,
              sensitivity_override: { applied: true, logged: "server-log" },
            }
          : structuredContent,
      };
    } catch (error: any) {
      const message = error?.message || String(error);
      // The CLI's failure envelope goes to stdout while anyhow's text goes to
      // stderr (which wins in the thrown message) — check stdout first.
      const envelope =
        parseStructuredCliError(error?.stdout || "") ??
        parseStructuredCliError(message);
      const data = envelope?.data;
      return {
        content: [
          {
            type: "text" as const,
            text: formatResummarizeFailure(data, message),
          },
        ],
        isError: true,
      };
    }
  }
);

// ── Tool: knowledge_status ──────────────────────────────────

registerTool(
  "knowledge_status",
  "Reconcile persistent meeting derivatives and show the privacy-safe knowledge-base status. This cleanup/status tool cannot create or register an external index.",
  {},
  { title: "Knowledge Status", readOnlyHint: false, destructiveHint: false, idempotentHint: true, openWorldHint: false },
  async () => {
    if (!(await isCliAvailable())) {
      return { content: [{ type: "text" as const, text: CLI_INSTALL_MSG }] };
    }

    try {
      const status = await readKnowledgeStatusSnapshot();
      if (!status.configured || !status.enabled) {
        return { content: [{ type: "text" as const, text: "Knowledge base: not configured or disabled.\n\nAdd [knowledge] section to ~/.config/minutes/config.toml with enabled = true and a path." }] };
      }

      const lines = [
        `Knowledge base: **enabled**`,
        `Adapter: ${status.adapter}`,
        `Extraction engine: ${status.engine}`,
        `People profiles: ${status.people_count}`,
        `Log entries: ${status.log_entries}`,
      ];

      return { content: [{ type: "text" as const, text: lines.join("\n") }] };
    } catch (error: any) {
      return {
        content: [{ type: "text" as const, text: "Persistent meeting derivatives could not be safely read." }],
        isError: true,
      };
    }
  }
);

// ── Transport selection ─────────────────────────────────────
// stdio stays the default so every existing client config keeps working
// untouched. `--transport http` is opt-in and documented as localhost-only.

export type MinutesTransportConfig = {
  transport: "stdio" | "http";
  port: number;
  maxSessions: number;
  help: boolean;
};

/**
 * `min` defaults to 0 because `--port 0` is meaningful: it asks the OS for a
 * free port. A count like `--max-sessions` has no such reading, and zero there
 * starts a server that refuses every client, so those pass `min: 1`.
 */
function parsePositiveInt(
  raw: string,
  flag: string,
  max: number,
  min = 0
): number {
  if (!/^\d+$/.test(raw.trim())) {
    throw new Error(`${flag} expects a number, got "${raw}"`);
  }
  const value = Number(raw.trim());
  if (value < min) {
    throw new Error(`${flag} must be >= ${min}, got ${value}`);
  }
  if (value > max) {
    throw new Error(`${flag} must be <= ${max}, got ${value}`);
  }
  return value;
}

/**
 * Parse transport flags. Unknown arguments are ignored rather than rejected —
 * hosts append their own (`--demo` is handled at module load), and a strict
 * parser here would turn a harmless extra argument into a startup failure.
 */
export function parseTransportConfig(
  argv: string[],
  env: NodeJS.ProcessEnv = process.env
): MinutesTransportConfig {
  const config: MinutesTransportConfig = {
    transport: "stdio",
    port: DEFAULT_HTTP_PORT,
    maxSessions: DEFAULT_MAX_SESSIONS,
    help: false,
  };

  const envTransport = env.MINUTES_MCP_TRANSPORT?.trim().toLowerCase();
  if (envTransport) {
    if (envTransport !== "stdio" && envTransport !== "http") {
      throw new Error(
        `MINUTES_MCP_TRANSPORT must be "stdio" or "http", got "${envTransport}"`
      );
    }
    config.transport = envTransport;
  }
  if (env.MINUTES_MCP_PORT?.trim()) {
    config.port = parsePositiveInt(
      env.MINUTES_MCP_PORT,
      "MINUTES_MCP_PORT",
      65535
    );
  }
  if (env.MINUTES_MCP_MAX_SESSIONS?.trim()) {
    config.maxSessions = parsePositiveInt(
      env.MINUTES_MCP_MAX_SESSIONS,
      "MINUTES_MCP_MAX_SESSIONS",
      1024,
      1
    );
  }

  // Accepts both `--flag value` and `--flag=value`.
  const valueOf = (index: number, flag: string, inline: string | null): string => {
    if (inline !== null) return inline;
    const next = argv[index + 1];
    if (next === undefined || next.startsWith("--")) {
      throw new Error(`${flag} requires a value`);
    }
    return next;
  };

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (!arg.startsWith("--")) continue;
    const eq = arg.indexOf("=");
    const flag = eq === -1 ? arg : arg.slice(0, eq);
    const inline = eq === -1 ? null : arg.slice(eq + 1);

    switch (flag) {
      case "--help":
        config.help = true;
        break;
      case "--transport": {
        const value = valueOf(i, flag, inline).trim().toLowerCase();
        if (value !== "stdio" && value !== "http") {
          throw new Error(
            `--transport must be "stdio" or "http", got "${value}"`
          );
        }
        config.transport = value;
        if (inline === null) i++;
        break;
      }
      case "--port":
        config.port = parsePositiveInt(valueOf(i, flag, inline), flag, 65535);
        if (inline === null) i++;
        break;
      case "--max-sessions":
        config.maxSessions = parsePositiveInt(
          valueOf(i, flag, inline),
          flag,
          1024,
          1
        );
        if (inline === null) i++;
        break;
      default:
        // Unknown flag — leave it to whoever passed it.
        break;
    }
  }

  return config;
}

function printUsage(): void {
  console.log(
    [
      "minutes-mcp — MCP server for Minutes (conversation memory for AI assistants)",
      "",
      "Usage: minutes-mcp [options]",
      "",
      "Options:",
      "  --transport <stdio|http>  Transport to serve on (default: stdio)",
      `  --port <number>           HTTP port, 0 for an OS-assigned port (default: ${DEFAULT_HTTP_PORT})`,
      `  --max-sessions <number>   Concurrent HTTP sessions (default: ${DEFAULT_MAX_SESSIONS})`,
      "  --demo                    Install the bundled demo corpus and exit",
      "  --help                    Show this message",
      "",
      "Environment: MINUTES_MCP_TRANSPORT, MINUTES_MCP_PORT,",
      "             MINUTES_MCP_MAX_SESSIONS",
      "",
      "HTTP mode serves Streamable HTTP at /mcp and has no authentication. The",
      `bind address is always ${HTTP_BIND_HOST}, so only this machine can reach it.`,
      "To expose it beyond this machine, put a reverse proxy in front and add",
      "authentication there.",
    ].join("\n")
  );
}

// ── Start server ────────────────────────────────────────────

/**
 * Probe for the engine at startup, install it if possible, and say what
 * happened. Never throws: startup does not depend on the outcome.
 */
async function announceCliAvailability(): Promise<void> {
  let available = false;
  try {
    available = await isCliAvailable();
  } catch {
    // A probe that fails is the same as one that says no.
  }
  crashTrace(available ? "required-cli-ready" : "required-cli-absent");
  if (!available) {
    console.error(`[Minutes] ${cliMissingGuidance()}`);
    console.error(
      "[Minutes] Starting anyway so tools can report this. Tools that need the engine will explain it."
    );
  }
}

async function main() {
  crashTrace("main-start");
  const config = parseTransportConfig(process.argv.slice(2));

  if (config.help) {
    printUsage();
    return;
  }

  if (config.transport === "http") {
    crashTrace("main-transport-http", { port: config.port });
    await announceCliAvailability();
    {
      const httpServer = await startMinutesHttpServer({
        port: config.port,
        maxSessions: config.maxSessions,
        createServer: createMinutesServer,
      });
      crashTrace("transport-connected");
      console.error(`Minutes MCP server listening on ${httpServer.url}`);
      console.error(
        "[Minutes] HTTP transport is unauthenticated — keep it bound to localhost"
      );
      const shutdown = () => {
        void httpServer.close().finally(() => process.exit(0));
      };
      process.once("SIGINT", shutdown);
      process.once("SIGTERM", shutdown);
    }
    return;
  }

  // The handshake is never gated on the engine. Refusing to start buys no
  // safety over refusing each operation, because completing a handshake and
  // listing tools exposes nothing, and every tool that touches content still
  // checks readiness on each call. What it did cost was the entire diagnosis:
  // a user whose auto-install could not reach the network got a server that
  // exited before saying anything, which reads as a broken extension rather
  // than a missing dependency (#774, reported against v0.25.0).
  await announceCliAvailability();
  const transport = new StdioServerTransport();
  crashTrace("transport-created");
  await server.connect(transport);
  crashTrace("transport-connected");
  console.error("Minutes MCP server running on stdio");
}

crashTrace("pre-main-guard", {
  argv1: process.argv[1] ?? null,
  resolvedArgv1: process.argv[1] ? resolve(process.argv[1]) : null,
  __filename,
  match: shouldRunMainEntry(process.argv[1], __filename),
});

if (shouldRunMainEntry(process.argv[1], __filename)) {
  main().catch((error) => {
    crashTrace("main-rejected", error);
    console.error("Fatal error:", error);
    process.exit(1);
  });
} else {
  crashTrace("main-skipped-argv-mismatch");
}
