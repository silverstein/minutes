// minutes-sdk — conversation memory for AI agents
//
// Query meeting transcripts, decisions, and action items from any
// AI agent or application. The "Mem0 for human conversations."
//
// Same functionality as the Rust `minutes-reader` crate.
//
// Architecture:
//   ~/meetings/*.md --> parseFrontmatter() --> MeetingFile
//                                                |
//                            +-------------------+
//                            v                   v
//                      listMeetings()      searchMeetings()

import { readFile, readdir, stat } from "fs/promises";
import { join, extname } from "path";
import { homedir } from "os";
import { parse as parseYaml } from "yaml";

// ── Types ────────────────────────────────────────────────────

export interface ActionItem {
  assignee: string;
  task: string;
  due?: string;
  status: string;
}

export interface Decision {
  text: string;
  topic?: string;
}

export interface Intent {
  kind: string;
  what: string;
  who?: string;
  status: string;
  by_date?: string;
}

export interface SpeakerAttribution {
  speaker_label: string;
  name: string;
  confidence: "high" | "medium" | "low";
  source: "deterministic" | "llm" | "enrollment" | "manual";
}

/**
 * A user-confirmed speaker correction stored in the sidecar overlay store
 * (`~/.minutes/overlays.db`). Overlays layer over raw frontmatter at read
 * time without ever mutating the meeting markdown on disk.
 *
 * Confirmations carry high confidence and `manual` source by definition —
 * they record an explicit user action, not a model inference.
 */
export interface SpeakerConfirmation {
  speaker_label: string;
  name: string;
  /** Optional name the overlay overrode, useful for "undo" UIs. */
  previous_name?: string;
}

export interface Frontmatter {
  title: string;
  type: string;
  date: string;
  duration: string;
  source?: string;
  status?: string;
  device?: string;
  captured_at?: string;
  tags: string[];
  attendees: string[];
  attendees_raw?: string;
  people: string[];
  context?: string;
  calendar_event?: string;
  action_items: ActionItem[];
  decisions: Decision[];
  intents: Intent[];
  speaker_map?: SpeakerAttribution[];
}

export interface MeetingFile {
  frontmatter: Frontmatter;
  body: string;
  path: string;
}

function parseRawAttendees(raw?: string): string[] {
  if (!raw) return [];

  const attendees: string[] = [];
  for (const token of raw.split(",")) {
    const trimmed = token.trim();
    if (!trimmed || trimmed.toLowerCase() === "none") continue;

    const parenMatch = trimmed.match(/^(.*?)\s*\([^)]*\)$/);
    const angleMatch = trimmed.match(/^(.*?)\s*<[^>]*>$/);
    const value = (parenMatch?.[1] || angleMatch?.[1] || trimmed).trim();
    if (!value) continue;
    if (!attendees.some((existing) => existing.toLowerCase() === value.toLowerCase())) {
      attendees.push(value);
    }
  }

  return attendees;
}

// ── Parsing ──────────────────────────────────────────────────

/**
 * Split markdown content into YAML frontmatter and body.
 * Returns null frontmatter string if no valid frontmatter found.
 */
export function splitFrontmatter(content: string): {
  yaml: string | null;
  body: string;
} {
  if (!content.startsWith("---")) {
    return { yaml: null, body: content };
  }

  const endIndex = content.indexOf("\n---", 3);
  if (endIndex === -1) {
    return { yaml: null, body: content };
  }

  const yaml = content.slice(3, endIndex).trim();
  const bodyStart = content.indexOf("\n", endIndex + 4);
  const body = bodyStart === -1 ? "" : content.slice(bodyStart + 1);

  return { yaml, body };
}

/**
 * Parse a meeting markdown file into its frontmatter and body.
 * Returns null if the file has no valid frontmatter or is unparseable.
 */
export function parseFrontmatter(
  content: string,
  filePath: string
): MeetingFile | null {
  const { yaml, body } = splitFrontmatter(content);
  if (!yaml) return null;

  try {
    const parsed = parseYaml(yaml);
    if (!parsed || typeof parsed !== "object") return null;

    const fm: Frontmatter = {
      title: String(parsed.title || ""),
      type: String(parsed.type || "meeting"),
      date: parsed.date instanceof Date ? parsed.date.toISOString() : String(parsed.date || ""),
      duration: String(parsed.duration || ""),
      source: parsed.source ? String(parsed.source) : undefined,
      status: parsed.status ? String(parsed.status) : undefined,
      tags: Array.isArray(parsed.tags) ? parsed.tags.map(String) : [],
      attendees: Array.isArray(parsed.attendees)
        ? parsed.attendees.map(String)
        : [],
      attendees_raw: parsed.attendees_raw ? String(parsed.attendees_raw) : undefined,
      people: Array.isArray(parsed.people) ? parsed.people.map(String) : [],
      context: parsed.context ? String(parsed.context) : undefined,
      calendar_event: parsed.calendar_event
        ? String(parsed.calendar_event)
        : undefined,
      action_items: Array.isArray(parsed.action_items)
        ? parsed.action_items.map((a: any) => ({
            assignee: String(a.assignee || ""),
            task: String(a.task || ""),
            due: a.due ? String(a.due) : undefined,
            status: String(a.status || "open"),
          }))
        : [],
      decisions: Array.isArray(parsed.decisions)
        ? parsed.decisions.map((d: any) => ({
            text: String(d.text || ""),
            topic: d.topic ? String(d.topic) : undefined,
          }))
        : [],
      intents: Array.isArray(parsed.intents)
        ? parsed.intents.map((i: any) => ({
            kind: String(i.kind || ""),
            what: String(i.what || ""),
            who: i.who ? String(i.who) : undefined,
            status: String(i.status || ""),
            by_date: i.by_date ? String(i.by_date) : undefined,
          }))
        : [],
      speaker_map: Array.isArray(parsed.speaker_map)
        ? parsed.speaker_map.map((s: any) => ({
            speaker_label: String(s.speaker_label || ""),
            name: String(s.name || ""),
            confidence: (s.confidence === "high" ||
              s.confidence === "medium" ||
              s.confidence === "low"
              ? s.confidence
              : "medium") as "high" | "medium" | "low",
            source: (s.source === "deterministic" ||
              s.source === "llm" ||
              s.source === "enrollment" ||
              s.source === "manual"
              ? s.source
              : "llm") as "deterministic" | "llm" | "enrollment" | "manual",
          }))
        : undefined,
    };

    return { frontmatter: fm, body, path: filePath };
  } catch {
    return null;
  }
}

// ── File scanning ────────────────────────────────────────────

/**
 * Recursively find all .md files in a directory.
 */
async function findMarkdownFiles(dir: string): Promise<string[]> {
  const results: string[] = [];

  try {
    const entries = await readdir(dir, { withFileTypes: true });
    for (const entry of entries) {
      const fullPath = join(dir, entry.name);
      if (entry.isDirectory()) {
        // Skip hidden directories and common non-meeting dirs
        if (!entry.name.startsWith(".")) {
          const nested = await findMarkdownFiles(fullPath);
          results.push(...nested);
        }
      } else if (
        entry.isFile() &&
        extname(entry.name).toLowerCase() === ".md"
      ) {
        results.push(fullPath);
      }
    }
  } catch {
    // Directory doesn't exist or permission denied — return empty
  }

  return results;
}

/**
 * Parse a single meeting file from disk.
 */
async function readMeetingFile(
  filePath: string
): Promise<MeetingFile | null> {
  try {
    const content = await readFile(filePath, "utf-8");
    return parseFrontmatter(content, filePath);
  } catch {
    return null;
  }
}

/**
 * Sort meetings by date descending (newest first).
 */
function sortByDateDesc(meetings: MeetingFile[]): MeetingFile[] {
  return meetings.sort((a, b) => {
    const dateA = a.frontmatter.date || "";
    const dateB = b.frontmatter.date || "";
    return dateB.localeCompare(dateA);
  });
}

// ── Public API ───────────────────────────────────────────────

/**
 * List meetings from a directory, sorted by date descending.
 */
export async function listMeetings(
  dir: string,
  limit: number = 20
): Promise<MeetingFile[]> {
  const files = await findMarkdownFiles(dir);
  const meetings: MeetingFile[] = [];

  for (const file of files) {
    const meeting = await readMeetingFile(file);
    if (meeting) meetings.push(meeting);
  }

  return sortByDateDesc(meetings).slice(0, limit);
}

/**
 * Search meetings by a text query in title and body.
 * Uses String.includes() — no regex, safe from special character crashes.
 */
export async function searchMeetings(
  dir: string,
  query: string,
  limit: number = 20
): Promise<MeetingFile[]> {
  if (!query) return [];

  const queryLower = query.toLowerCase();
  const files = await findMarkdownFiles(dir);
  const results: MeetingFile[] = [];

  for (const file of files) {
    const meeting = await readMeetingFile(file);
    if (!meeting) continue;

    const titleMatch = meeting.frontmatter.title
      .toLowerCase()
      .includes(queryLower);
    const bodyMatch = meeting.body.toLowerCase().includes(queryLower);

    if (titleMatch || bodyMatch) {
      results.push(meeting);
    }
  }

  return sortByDateDesc(results).slice(0, limit);
}

/**
 * Get a single meeting by file path.
 */
export async function getMeeting(
  filePath: string
): Promise<MeetingFile | null> {
  return readMeetingFile(filePath);
}

/**
 * Layer sidecar speaker confirmations over a meeting's `speaker_map`,
 * returning a new MeetingFile with the corrections applied. The original
 * meeting object is not mutated, and the body text is not rewritten —
 * Minutes treats raw markdown as immutable capture.
 *
 * For each confirmation:
 *   - if a `speaker_map` entry with the same `speaker_label` exists, its
 *     `name` is replaced and confidence/source are bumped to high/manual
 *   - if no entry exists, a new one is appended
 *
 * Pass an empty `confirmations` array to no-op.
 */
export function applySpeakerOverlays(
  meeting: MeetingFile,
  confirmations: SpeakerConfirmation[]
): MeetingFile {
  if (!confirmations || confirmations.length === 0) {
    return meeting;
  }

  const baseMap = meeting.frontmatter.speaker_map ?? [];
  const merged: SpeakerAttribution[] = baseMap.map((attr) => ({ ...attr }));

  for (const confirmation of confirmations) {
    if (!confirmation.speaker_label || !confirmation.name) continue;

    const existing = merged.find(
      (attr) => attr.speaker_label === confirmation.speaker_label
    );
    if (existing) {
      existing.name = confirmation.name;
      existing.confidence = "high";
      existing.source = "manual";
    } else {
      merged.push({
        speaker_label: confirmation.speaker_label,
        name: confirmation.name,
        confidence: "high",
        source: "manual",
      });
    }
  }

  return {
    ...meeting,
    frontmatter: { ...meeting.frontmatter, speaker_map: merged },
  };
}

/**
 * Rewrite `[SPEAKER_N <timestamp>] text` line prefixes in a meeting
 * transcript body to use the speaker's mapped name. Mirrors the Rust
 * `apply_confirmed_names` helper:
 *
 *   - Only attributions with `confidence: "high"` are applied — model
 *     guesses below that bar do not silently rewrite the transcript.
 *   - If a line's body itself looks like a non-lexical event marker
 *     (e.g. `[laughter]`, `[music]`), the speaker label is left alone
 *     so the rendered output keeps the event tag instead of saying
 *     "Alex Kim: [laughter]".
 *   - Non-bracketed lines (headings, prose, blank) are returned
 *     unchanged.
 *
 * The function is pure: the input string is not mutated.
 */
export function humanizeTranscript(
  body: string,
  speakerMap: SpeakerAttribution[] | undefined
): string {
  if (!speakerMap || speakerMap.length === 0) return body;

  const highMap = new Map<string, string>();
  for (const attr of speakerMap) {
    if (attr.confidence === "high" && attr.speaker_label && attr.name) {
      highMap.set(attr.speaker_label, attr.name);
    }
  }
  if (highMap.size === 0) return body;

  const out: string[] = [];
  for (const line of body.split("\n")) {
    out.push(humanizeOneLine(line, highMap));
  }
  return out.join("\n");
}

function humanizeOneLine(line: string, highMap: Map<string, string>): string {
  if (!line.startsWith("[")) return line;

  const close = line.indexOf("]");
  if (close < 0) return line;

  const inside = line.slice(1, close);
  const space = inside.indexOf(" ");
  if (space < 0) return line;

  const label = inside.slice(0, space);
  const replacement = highMap.get(label);
  if (!replacement) return line;

  const remainder = inside.slice(space + 1);
  const after = line.slice(close + 1);

  // Skip rewriting when the body is itself a bracketed event tag —
  // matches Rust's is_non_lexical_event_text guard.
  const trimmedAfter = after.trimStart();
  if (trimmedAfter.startsWith("[") && trimmedAfter.trimEnd().endsWith("]")) {
    return line;
  }

  return `[${replacement} ${remainder}]${after}`;
}

/**
 * Get a meeting with sidecar overlay confirmations layered over its
 * `speaker_map`. Best-effort convenience: shells to the local `minutes`
 * CLI (`minutes get <path> --json`) which reads `~/.minutes/overlays.db`
 * server-side and returns an overlay-applied payload. If the CLI is not
 * available or the call fails, falls back to plain `getMeeting()` so
 * consumers always get a usable result.
 *
 * For full control over which overlays apply (e.g. to layer a remote
 * overlay store, or to test against fixtures), use `applySpeakerOverlays`
 * directly with confirmations sourced however you prefer.
 */
export async function getMeetingWithOverlays(
  filePath: string,
  options: { minutesBin?: string; timeoutMs?: number } = {}
): Promise<MeetingFile | null> {
  const fallback = await getMeeting(filePath);
  if (!fallback) return null;

  // Dynamically import child_process so this module still loads in
  // environments without it (browsers, Edge runtimes). The function
  // simply degrades to non-overlay behavior in those cases.
  let execFile: typeof import("child_process").execFile;
  try {
    ({ execFile } = await import("child_process"));
  } catch {
    return fallback;
  }

  const bin = options.minutesBin ?? process.env.MINUTES_BIN ?? "minutes";
  const timeoutMs = options.timeoutMs ?? 10_000;

  const stdout = await new Promise<string | null>((resolve) => {
    execFile(
      bin,
      ["get", filePath, "--json"],
      { timeout: timeoutMs, maxBuffer: 8 * 1024 * 1024 },
      (err, out) => {
        if (err) resolve(null);
        else resolve(out.toString());
      }
    );
  });

  if (!stdout) return fallback;

  try {
    const payload = JSON.parse(stdout);
    const overlaidMap = payload?.frontmatter?.speaker_map;
    if (!Array.isArray(overlaidMap)) return fallback;

    return {
      ...fallback,
      frontmatter: {
        ...fallback.frontmatter,
        speaker_map: overlaidMap.map((attr: any) => ({
          speaker_label: String(attr.speaker_label || ""),
          name: String(attr.name || ""),
          confidence: (attr.confidence === "high" ||
            attr.confidence === "medium" ||
            attr.confidence === "low"
            ? attr.confidence
            : "medium") as "high" | "medium" | "low",
          source: (attr.source === "deterministic" ||
            attr.source === "llm" ||
            attr.source === "enrollment" ||
            attr.source === "manual"
            ? attr.source
            : "llm") as "deterministic" | "llm" | "enrollment" | "manual",
        })),
      },
    };
  } catch {
    return fallback;
  }
}

/**
 * Find open action items across all meetings.
 */
export async function findOpenActions(
  dir: string,
  assignee?: string
): Promise<Array<{ path: string; item: ActionItem }>> {
  const files = await findMarkdownFiles(dir);
  const results: Array<{ path: string; item: ActionItem }> = [];

  for (const file of files) {
    const meeting = await readMeetingFile(file);
    if (!meeting) continue;

    for (const item of meeting.frontmatter.action_items) {
      if (item.status !== "open") continue;
      if (
        assignee &&
        item.assignee.toLowerCase() !== assignee.toLowerCase()
      ) {
        continue;
      }
      results.push({ path: meeting.path, item });
    }
  }

  return results;
}

/**
 * Build a person profile from all meetings mentioning them.
 */
export async function getPersonProfile(
  dir: string,
  name: string
): Promise<{
  name: string;
  meetings: Array<{ title: string; date: string; path: string }>;
  openActions: ActionItem[];
  topics: string[];
}> {
  const nameLower = name.toLowerCase();
  const files = await findMarkdownFiles(dir);
  const meetings: Array<{ title: string; date: string; path: string }> = [];
  const openActions: ActionItem[] = [];
  const topicSet = new Set<string>();

  for (const file of files) {
    const meeting = await readMeetingFile(file);
    if (!meeting) continue;

    const attendees = [
      ...meeting.frontmatter.attendees,
      ...parseRawAttendees(meeting.frontmatter.attendees_raw),
    ];

    const inAttendees = attendees.some((a) =>
      a.toLowerCase().includes(nameLower)
    );
    const inPeople = meeting.frontmatter.people.some((p) =>
      p.toLowerCase().includes(nameLower)
    );
    const inBody = meeting.body.toLowerCase().includes(nameLower);

    if (inAttendees || inPeople || inBody) {
      meetings.push({
        title: meeting.frontmatter.title,
        date: meeting.frontmatter.date,
        path: meeting.path,
      });

      for (const tag of meeting.frontmatter.tags) {
        topicSet.add(tag);
      }

      for (const item of meeting.frontmatter.action_items) {
        if (
          item.status === "open" &&
          item.assignee.toLowerCase().includes(nameLower)
        ) {
          openActions.push(item);
        }
      }
    }
  }

  return {
    name,
    meetings: meetings.sort((a, b) => b.date.localeCompare(a.date)),
    openActions,
    topics: Array.from(topicSet),
  };
}

/**
 * Default meetings directory (~\/meetings).
 * Override with MEETINGS_DIR env var or pass a custom path to any function.
 */
export function defaultDir(): string {
  return process.env.MEETINGS_DIR || join(homedir(), "meetings");
}

/**
 * List recent voice memos (type: memo), sorted by date descending.
 * Useful for cross-device pipeline recall — "what ideas did I capture recently?"
 */
export async function listVoiceMemos(
  dir: string,
  options: { days?: number; limit?: number } = {}
): Promise<MeetingFile[]> {
  const { days = 14, limit = 20 } = options;
  const cutoff = new Date();
  cutoff.setDate(cutoff.getDate() - days);

  const meetings = await listMeetings(dir, 500);
  const memos = meetings.filter((m) => {
    if (m.frontmatter.type !== "memo") return false;
    const date = new Date(m.frontmatter.date);
    return date >= cutoff;
  });

  return memos.slice(0, limit);
}

/**
 * Find decisions across all meetings, optionally filtered by topic keyword.
 */
export async function findDecisions(
  dir: string,
  topic?: string,
  limit: number = 50
): Promise<Array<{ path: string; title: string; date: string; decision: Decision }>> {
  const files = await findMarkdownFiles(dir);
  const results: Array<{ path: string; title: string; date: string; decision: Decision }> = [];

  for (const file of files) {
    const meeting = await readMeetingFile(file);
    if (!meeting) continue;

    for (const decision of meeting.frontmatter.decisions) {
      if (topic) {
        const topicLower = topic.toLowerCase();
        const matches =
          decision.text.toLowerCase().includes(topicLower) ||
          (decision.topic && decision.topic.toLowerCase().includes(topicLower));
        if (!matches) continue;
      }
      results.push({
        path: meeting.path,
        title: meeting.frontmatter.title,
        date: meeting.frontmatter.date,
        decision,
      });
    }
  }

  return results
    .sort((a, b) => b.date.localeCompare(a.date))
    .slice(0, limit);
}
