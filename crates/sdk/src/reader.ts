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

import { realpath, stat } from "fs/promises";
import { lexicalMatch, compareSearchMatches, contentQuery, queryTokens, relaxedQueries, type LexicalMatch } from "./retrieval.js";
import { buildSourceEvidence, type EvidenceBundle } from "./evidence.js";
import { buildDecisionHistory, type DecisionHistoryEntry, type HistoricalDecision } from "./decision-history.js";
import { resolvePersonIdentity, meetingHasPersonParticipant, personOwnsAction } from "./identity.js";
import { extname, isAbsolute, join, relative, resolve } from "path";
import { homedir } from "os";
import { parse as parseYaml } from "yaml";
import {
  withStableCorpusLease,
  type StableCorpusSnapshot,
} from "./corpus-lease.js";
import {
  decodePolicyUtf8,
  readTextFileFromBoundParent,
} from "./secure-read.js";

// ── Types ────────────────────────────────────────────────────

export interface ActionItem {
  assignee: string;
  task: string;
  due?: string;
  status: string;
}

export interface Decision extends HistoricalDecision {}

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
  source: AttributionSource;
}

/**
 * How an attribution was derived.
 *
 * The named values are what Minutes emits. The trailing `(string & {})` keeps
 * an unrecognized label readable instead of a type error: hand-edited files and
 * `minutes import text` archives are supported inputs, so a provenance label
 * must never make a meeting unparseable (#595). Mirrors the `Unknown(String)`
 * variant on the Rust side, which round-trips the raw value verbatim.
 */
export type AttributionSource =
  | "deterministic"
  | "llm"
  | "enrollment"
  | "manual"
  | "ml-bleed-degraded"
  | "stem-recovery"
  // eslint-disable-next-line @typescript-eslint/ban-types
  | (string & {});

export type DiagnosticConfidence = "high" | "inferred";
export type CaptureSource = "voice" | "system" | "both" | "backend";
export type DiarizationPath = "stem-energy" | "ml" | "ml-bleed-degraded" | "none";
export type FailureKind =
  | "silent"
  | "sparse"
  | "missing"
  | "backend-unavailable"
  | "stream-error"
  | "source-starved"
  | "unsupported-format"
  | "misconfigured-route"
  | "permission-denied"
  | "route-unavailable"
  | { other: { code: string } };

export interface CaptureWarning {
  kind: FailureKind;
  source: CaptureSource;
  message: string;
  diagnostic_confidence: DiagnosticConfidence;
}

export interface RecordingHealth {
  voice_stem_active_ratio?: number;
  system_stem_active_ratio?: number;
  system_dominant_ratio?: number;
  capture_warnings: CaptureWarning[];
  diarization_path?: DiarizationPath;
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
  capture?: "none";
  sensitivity?: "normal" | "restricted";
  debrief?: "pending" | "complete" | "not-applicable";
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
  entities?: { people?: Array<{ slug?: string; label: string; aliases?: string[] }> };
  /** Applied post-pass name corrections (raw token preserved), if any. */
  name_corrections?: { raw: string; corrected: string }[];
  recording_health?: RecordingHealth;
}

export interface MeetingFile {
  frontmatter: Frontmatter;
  body: string;
  path: string;
  /** Discriminant retained for ergonomic narrowing of exact-read results. */
  restricted_stub?: false;
  evidence?: EvidenceBundle;
  retrieval?: LexicalMatch;
}

/**
 * Path-free placeholder for an exact read of a restricted meeting.
 *
 * This is deliberately not a `MeetingFile`: source paths, capture metadata,
 * transcript-shaped arrays, and every other field that could carry restricted
 * identifiers are absent from the type as well as the serialized value.
 */
export interface RestrictedMeetingStub {
  restricted_stub: true;
  body: string;
  frontmatter: {
    title: string;
    type: string;
    date: string;
    sensitivity: "restricted";
  };
}

export type ExactMeetingResult = MeetingFile | RestrictedMeetingStub;

/** Normalize only Rust/Node's equivalent Windows canonical wire prefixes. */
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

export function parseAttributionSource(raw: string): AttributionSource {
  if (
    raw === "deterministic" ||
    raw === "llm" ||
    raw === "enrollment" ||
    raw === "manual" ||
    raw === "ml-bleed-degraded" ||
    raw === "stem-recovery"
  ) {
    return raw;
  }

  return "llm";
}

function parseDiagnosticConfidence(raw: unknown): DiagnosticConfidence {
  if (raw === "high" || raw === "inferred") return raw;
  throw new Error(`unknown diagnostic confidence: ${String(raw)}`);
}

function parseCaptureSource(raw: unknown): CaptureSource {
  if (raw === "voice" || raw === "system" || raw === "both" || raw === "backend") {
    return raw;
  }
  throw new Error(`unknown capture source: ${String(raw)}`);
}

function parseDiarizationPath(raw: unknown): DiarizationPath {
  if (raw === "stem-energy" || raw === "ml" || raw === "ml-bleed-degraded" || raw === "none") {
    return raw;
  }
  throw new Error(`unknown diarization path: ${String(raw)}`);
}

function parseFailureKind(raw: unknown): FailureKind {
  if (
    raw === "silent" ||
    raw === "sparse" ||
    raw === "missing" ||
    raw === "backend-unavailable" ||
    raw === "stream-error" ||
    raw === "source-starved" ||
    raw === "unsupported-format" ||
    raw === "misconfigured-route" ||
    raw === "permission-denied" ||
    raw === "route-unavailable"
  ) {
    return raw;
  }

  if (
    raw &&
    typeof raw === "object" &&
    "other" in raw &&
    (raw as any).other &&
    typeof (raw as any).other === "object"
  ) {
    return { other: { code: String((raw as any).other.code || "") } };
  }

  throw new Error(`unknown capture failure kind: ${String(raw)}`);
}

function optionalNumber(raw: unknown): number | undefined {
  return typeof raw === "number" && Number.isFinite(raw) ? raw : undefined;
}

function parseRecordingHealth(raw: any): RecordingHealth | undefined {
  if (!raw || typeof raw !== "object") return undefined;

  return {
    voice_stem_active_ratio: optionalNumber(raw.voice_stem_active_ratio),
    system_stem_active_ratio: optionalNumber(raw.system_stem_active_ratio),
    system_dominant_ratio: optionalNumber(raw.system_dominant_ratio),
    capture_warnings: Array.isArray(raw.capture_warnings)
      ? raw.capture_warnings.map((warning: any) => ({
          kind: parseFailureKind(warning?.kind),
          source: parseCaptureSource(warning?.source),
          message: String(warning?.message || ""),
          diagnostic_confidence: parseDiagnosticConfidence(warning?.diagnostic_confidence),
        }))
      : [],
    diarization_path: raw.diarization_path
      ? parseDiarizationPath(raw.diarization_path)
      : undefined,
  };
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

    if (typeof parsed.title !== "string" || parsed.title.trim() === "") {
      return null;
    }
    if (
      typeof parsed.type !== "string" ||
      !["meeting", "memo", "dictation", "note"].includes(parsed.type)
    ) {
      return null;
    }
    const parsedDate =
      parsed.date instanceof Date ? parsed.date : new Date(String(parsed.date ?? ""));
    if (Number.isNaN(parsedDate.getTime())) {
      return null;
    }

    // Sensitivity is an agent-enforcement field. A document that explicitly
    // declares an unknown value must not be silently reclassified as a normal
    // meeting: doing so would turn a typo or future policy value into an
    // exfiltration bypass. Legacy documents with no sensitivity key remain
    // normal and readable.
    if (
      Object.prototype.hasOwnProperty.call(parsed, "sensitivity") &&
      parsed.sensitivity !== "normal" &&
      parsed.sensitivity !== "restricted"
    ) {
      return null;
    }

    const fm: Frontmatter = {
      title: parsed.title,
      type: parsed.type,
      date: parsed.date instanceof Date ? parsed.date.toISOString() : String(parsed.date),
      duration: String(parsed.duration || ""),
      source: parsed.source ? String(parsed.source) : undefined,
      status: parsed.status ? String(parsed.status) : undefined,
      capture: parsed.capture === "none" ? "none" : undefined,
      sensitivity: parsed.sensitivity === "normal" || parsed.sensitivity === "restricted"
        ? parsed.sensitivity
        : undefined,
      debrief: parsed.debrief === "pending" ||
        parsed.debrief === "complete" ||
        parsed.debrief === "not-applicable"
        ? parsed.debrief
        : undefined,
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
            id: d.id ? String(d.id) : undefined,
            authority: d.authority ? String(d.authority) : undefined,
            supersedes: d.supersedes ? String(d.supersedes) : undefined,
            effective_date: d.effective_date ? String(d.effective_date) : undefined,
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
            source: parseAttributionSource(String(s.source || "")),
          }))
        : undefined,
      entities: Array.isArray(parsed.entities?.people) ? {
        people: parsed.entities.people.filter((p: any) => p && typeof p.label === "string").map((p: any) => ({
          label: p.label,
          slug: typeof p.slug === "string" ? p.slug : undefined,
          aliases: Array.isArray(p.aliases) ? p.aliases.filter((a: unknown) => typeof a === "string") : [],
        })),
      } : undefined,
      recording_health: parseRecordingHealth(parsed.recording_health),
    };

    return { frontmatter: fm, body, path: filePath };
  } catch {
    return null;
  }
}

// ── File scanning ────────────────────────────────────────────

const INACTIVE_CORPUS_DIRS = new Set([
  "archive",
  "processed",
  "failed",
  "failed-captures",
]);

function isInactiveCorpusDirectory(name: string): boolean {
  return INACTIVE_CORPUS_DIRS.has(name.toLowerCase());
}

function isActiveCorpusPath(filePath: string, root: string): boolean {
  const scoped = relative(root, filePath);
  if (
    scoped === "" ||
    isAbsolute(scoped) ||
    scoped.split(/[\\/]+/).some((component) => component === "..")
  ) {
    return false;
  }
  return !scoped
    .split(/[\\/]+/)
    .some(
      (component) => component.startsWith(".") || isInactiveCorpusDirectory(component)
    );
}

async function canonicalCorpusRoot(root: string): Promise<string | null> {
  try {
    const canonicalRoot = await realpath(root);
    return (await stat(canonicalRoot)).isDirectory() ? canonicalRoot : null;
  } catch {
    return null;
  }
}

async function readMeetingFileAtCanonicalRoot(
  filePath: string,
  canonicalRoot: string
): Promise<MeetingFile | null> {
  try {
    const canonicalPath = await realpath(filePath);
    if (
      extname(canonicalPath).toLowerCase() !== ".md" ||
      !isActiveCorpusPath(canonicalPath, canonicalRoot)
    ) {
      return null;
    }
    const content = decodePolicyUtf8(
      await readTextFileFromBoundParent(canonicalPath)
    );
    const meeting = parseFrontmatter(content, canonicalPath);
    if (meeting) meetingSnapshotContent.set(meeting, content);
    return meeting;
  } catch {
    return null;
  }
}

/**
 * Sort meetings by date descending (newest first).
 */
function sortByDateDesc(meetings: MeetingFile[]): MeetingFile[] {
  return meetings.sort((a, b) =>
    compareDatePathNewestFirst(
      a.frontmatter.date,
      a.path,
      b.frontmatter.date,
      b.path
    )
  );
}

function compareDatePathNewestFirst(
  dateAValue: string,
  pathA: string,
  dateBValue: string,
  pathB: string
): number {
  const dateA = Date.parse(dateAValue);
  const dateB = Date.parse(dateBValue);
  const validA = Number.isFinite(dateA);
  const validB = Number.isFinite(dateB);
  if (validA && validB && dateA !== dateB) return dateA > dateB ? -1 : 1;
  if (validA !== validB) return validA ? -1 : 1;
  return pathA.localeCompare(pathB);
}

/** Maximum meetings returned by list/search APIs in one call. */
export const SDK_MEETING_RESULT_MAX = 10_000;
/** Maximum voice memos returned by `listVoiceMemos` in one call. */
export const SDK_VOICE_MEMO_RESULT_MAX = 1_000;
/** Maximum open actions returned by `findOpenActions` in one call. */
export const SDK_OPEN_ACTION_RESULT_MAX = 1_000;
/** Maximum decisions returned by `findDecisions` in one call. */
export const SDK_DECISION_RESULT_MAX = 1_000;
/** Per-collection caps for `getPersonProfile`. */
export const SDK_PERSON_PROFILE_MEETING_MAX = 1_000;
export const SDK_PERSON_PROFILE_OPEN_ACTION_MAX = 1_000;
export const SDK_PERSON_PROFILE_TOPIC_MAX = 1_000;
/** Maximum accepted voice-memo lookback window (100 years). */
export const SDK_VOICE_MEMO_LOOKBACK_MAX_DAYS = 36_500;

function normalizeResultLimit(
  limit: number,
  max: number,
  surface: string
): number {
  if (
    !Number.isSafeInteger(limit) ||
    limit < 1 ||
    limit > max
  ) {
    throw new RangeError(
      `${surface} limit must be an integer between 1 and ${max}`
    );
  }
  return limit;
}

function normalizeMeetingResultLimit(limit: number): number {
  return normalizeResultLimit(limit, SDK_MEETING_RESULT_MAX, "meeting result");
}

// ── Sensitivity enforcement ──────────────────────────────────

/**
 * Options shared by the agent-facing read functions.
 *
 * Sensitivity is an enforcement contract, not just a label: meetings marked
 * `sensitivity: restricted` are excluded by default from every agent surface
 * that reads through this module (MCP search/tools, person profiles, open
 * actions, decisions). Set `includeRestricted` to override that default; doing
 * so is explicit and logged (a warning naming the count is written to stderr,
 * which is the MCP server's log channel).
 */
export interface ReadOptions {
  /** Include `sensitivity: restricted` meetings. Default false. */
  includeRestricted?: boolean;
  /**
   * Authoritative active-corpus root for exact-path reads. Defaults to
   * `defaultDir()` (and therefore honors `MEETINGS_DIR`). A requested path
   * outside this root, or beneath an inactive/hidden directory, is rejected.
   */
  rootDir?: string;
}

/** Options for `findOpenActions`; results never exceed the exported cap. */
export interface OpenActionOptions extends ReadOptions {
  /** Maximum actions to return (1-`SDK_OPEN_ACTION_RESULT_MAX`). */
  limit?: number;
}

/** Independent collection bounds for `getPersonProfile`. */
export interface PersonProfileOptions extends ReadOptions {
  /** Maximum matching meetings to return. */
  meetingLimit?: number;
  /** Maximum matching open actions to return. */
  openActionLimit?: number;
  /** Maximum distinct topics to return. */
  topicLimit?: number;
}

/** Options for `listVoiceMemos`; both window and result size are bounded. */
export interface VoiceMemoOptions extends ReadOptions {
  /** Non-negative lookback window, capped at 100 years. */
  days?: number;
  /** Maximum memos to return (1-`SDK_VOICE_MEMO_RESULT_MAX`). */
  limit?: number;
}

const meetingSnapshotContent = new WeakMap<object, string>();

function evidenceFromMeeting(meeting: MeetingFile, query?: string): EvidenceBundle | undefined {
  const content = meetingSnapshotContent.get(meeting);
  return content === undefined ? undefined : buildSourceEvidence({
    path: meeting.path, content, body: meeting.body, speakerMap: meeting.frontmatter.speaker_map,
  }, { query });
}

/** Temporal decisions from the same policy-authorized corpus lease as search. */
export async function getDecisionHistory(
  dir: string,
  opts: ReadOptions & { asOf?: string; limit?: number } = {}
): Promise<DecisionHistoryEntry[]> {
  buildDecisionHistory([], opts); // Validate public options before opening a corpus lease.
  return stableMeetingOperation(dir, opts, "decision_history", (meetings) =>
    buildDecisionHistory(meetings.flatMap((meeting) => {
      const content = meetingSnapshotContent.get(meeting);
      return content === undefined ? [] : [{ ...meeting, content }];
    }), opts)
  );
}

/** True when a meeting is marked `sensitivity: restricted`. */
export function isRestricted(meeting: MeetingFile): boolean {
  return meeting.frontmatter.sensitivity === "restricted";
}

/**
 * Apply the default restricted-meeting exclusion to a parsed collection.
 *
 * Without `includeRestricted`, restricted meetings are dropped. With it, they
 * are kept and the override is logged to stderr (naming the count and the
 * surface) so the bypass is never silent.
 */
function enforceSensitivity<T extends MeetingFile>(
  meetings: T[],
  opts: ReadOptions
): T[] {
  if (opts.includeRestricted) return meetings;
  let retained = 0;
  for (const meeting of meetings) {
    if (isRestricted(meeting)) continue;
    meetings[retained] = meeting;
    retained += 1;
  }
  meetings.length = retained;
  return meetings;
}

function meetingsFromStableSnapshot(snapshot: StableCorpusSnapshot): MeetingFile[] {
  const meetings: MeetingFile[] = [];
  for (const file of snapshot.files) {
    const meeting = parseFrontmatter(file.content, file.path);
    if (!meeting) continue;
    meetingSnapshotContent.set(meeting, file.content);
    meetings.push(meeting);
  }
  return meetings;
}

async function stableMeetingOperation<T>(
  dir: string,
  opts: ReadOptions,
  surface: string,
  operation: (meetings: MeetingFile[]) => T
): Promise<T> {
  const canonicalRoot = await canonicalCorpusRoot(dir);
  // A missing/unreadable corpus has no source bytes to authorize. Preserve the
  // SDK's empty-corpus contract without weakening reads from an existing root.
  if (!canonicalRoot) return operation([]);
  let restrictedOverrideCount = 0;
  // Every caller below supplies a synchronous, local-only projection. Its
  // result remains inside the lease until the final journal fence succeeds;
  // no callback performs I/O, publishes output, or invokes user code.
  const result = await withStableCorpusLease(
    canonicalRoot,
    (snapshot) => {
      const meetings = meetingsFromStableSnapshot(snapshot);
      restrictedOverrideCount = 0;
      if (opts.includeRestricted) {
        for (const meeting of meetings) {
          if (isRestricted(meeting)) restrictedOverrideCount += 1;
        }
      }
      return operation(enforceSensitivity(meetings, opts));
    }
  );
  if (restrictedOverrideCount > 0) {
    console.warn(
      `[minutes] includeRestricted override: surfacing ${restrictedOverrideCount} restricted meeting(s) via ${surface}`
    );
  }
  return result;
}

// ── Public API ───────────────────────────────────────────────

/**
 * List meetings from a directory, sorted by date descending.
 *
 * Restricted meetings are excluded by default; pass `{ includeRestricted: true }`
 * for an explicit, logged override.
 */
export async function listMeetings(
  dir: string,
  limit: number = 20,
  opts: ReadOptions = {}
): Promise<MeetingFile[]> {
  const boundedLimit = normalizeMeetingResultLimit(limit);
  return stableMeetingOperation(dir, opts, "list_meetings", (meetings) =>
    sortByDateDesc(meetings).slice(0, boundedLimit)
  );
}

/**
 * Search meetings by a text query in title and body.
 * Uses the shared literal AND/final-token-prefix lexical retrieval contract.
 *
 * Restricted meetings are excluded by default; pass `{ includeRestricted: true }`
 * for an explicit, logged override.
 */
export async function searchMeetings(
  dir: string,
  query: string,
  limit: number = 20,
  opts: ReadOptions = {}
): Promise<MeetingFile[]> {
  const boundedLimit = normalizeMeetingResultLimit(limit);
  if (queryTokens(query).length === 0) return [];

  const meant = contentQuery(query);
  return stableMeetingOperation(dir, opts, "search_meetings", (meetings) => {
    const matchAll = (wanted: string) => meetings.flatMap((meeting) => {
      const retrieval = lexicalMatch(meeting.frontmatter.title, meeting.body, wanted);
      return retrieval ? [{ meeting, retrieval }] : [];
    });
    // Stopwords are optional. Below 10 full matches, meetings that match all
    // but one term fill in after them (each by its best such query); a title
    // match still outranks words scattered through a transcript (as Rust).
    const matches = matchAll(meant).map((match) => ({ ...match, complete: true }));
    if (matches.length < 10) {
      const seen = new Set(matches.map((match) => match.meeting.path));
      const best = new Map<string, (typeof matches)[number]>();
      for (const relaxed of relaxedQueries(meant)) {
        for (const match of matchAll(relaxed)) {
          if (seen.has(match.meeting.path)) continue;
          const held = best.get(match.meeting.path);
          if (!held || compareSearchMatches(
            { tier: match.retrieval.tier, score: match.retrieval.score, date: "", path: "" },
            { tier: held.retrieval.tier, score: held.retrieval.score, date: "", path: "" }
          ) < 0) best.set(match.meeting.path, { ...match, complete: false });
        }
      }
      matches.push(...best.values());
    }
    const rank = (match: (typeof matches)[number]) =>
      (match.retrieval.tier >= 2 ? 2 : 0) + (match.complete ? 1 : 0);
    matches.sort((a, b) => rank(b) - rank(a) || compareSearchMatches(
      { tier: a.retrieval.tier, score: a.retrieval.score, date: a.meeting.frontmatter.date, path: a.meeting.path },
      { tier: b.retrieval.tier, score: b.retrieval.score, date: b.meeting.frontmatter.date, path: b.meeting.path }
    ));
    return matches.slice(0, boundedLimit).map(({ meeting, retrieval }) => ({
      ...meeting, retrieval, evidence: evidenceFromMeeting(meeting, query),
    }));
  });
}

/**
 * Body text carried by a restricted-meeting stub instead of the transcript.
 */
const RESTRICTED_STUB_NOTE =
  "Content excluded by default: this meeting is designated `sensitivity: restricted`. " +
  "Pass `{ includeRestricted: true }` for an explicit, logged override.";

/**
 * Build the minimal placeholder returned for a restricted meeting fetched by
 * exact path without the override: title, date, type, and the sensitivity
 * designation only. The transcript body, action items, decisions, and
 * attendees are never included.
 */
function restrictedStub(meeting: MeetingFile): RestrictedMeetingStub {
  return {
    restricted_stub: true,
    body: RESTRICTED_STUB_NOTE,
    frontmatter: {
      title: meeting.frontmatter.title,
      type: meeting.frontmatter.type,
      date: meeting.frontmatter.date,
      sensitivity: "restricted",
    },
  };
}

/**
 * Get a single meeting by file path.
 *
 * A restricted meeting is reduced to a minimal stub by default — title, date,
 * `sensitivity: restricted`, and a note that content is excluded until the
 * `includeRestricted` override is passed — even when the caller already knows
 * the path, so a stored path cannot be used to bypass the policy. Pass
 * `{ includeRestricted: true }` for an explicit, logged override; check
 * `restricted_stub` on the result to tell the two apart.
 */
export async function getMeeting(
  filePath: string,
  opts: ReadOptions = {}
): Promise<ExactMeetingResult | null> {
  const canonicalRoot = await canonicalCorpusRoot(opts.rootDir ?? defaultDir());
  if (!canonicalRoot) return null;
  return getMeetingAtCanonicalRoot(filePath, opts, canonicalRoot);
}

async function getMeetingAtCanonicalRoot(
  filePath: string,
  opts: ReadOptions,
  canonicalRoot: string
): Promise<ExactMeetingResult | null> {
  const meeting = await readMeetingFileAtCanonicalRoot(filePath, canonicalRoot);
  if (!meeting) return null;
  if (isRestricted(meeting)) {
    if (!opts.includeRestricted) {
      console.warn(
        "[minutes] get_meeting: restricted source; returning stub (content excluded by default)"
      );
      return restrictedStub(meeting);
    }
    console.warn("[minutes] includeRestricted override: returning one restricted meeting");
  }
  meeting.evidence = evidenceFromMeeting(meeting);
  return meeting;
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
 * available or the call fails, re-reads the source through plain
 * `getMeeting()` authorization before returning its current state.
 *
 * For full control over which overlays apply (e.g. to layer a remote
 * overlay store, or to test against fixtures), use `applySpeakerOverlays`
 * directly with confirmations sourced however you prefer.
 */
export async function getMeetingWithOverlays(
  filePath: string,
  options: ReadOptions & { minutesBin?: string; timeoutMs?: number } = {}
): Promise<ExactMeetingResult | null> {
  // Resolve the authority once. The overlay subprocess is untrusted to change
  // the root used by either the initial read or the post-overlay recheck.
  const canonicalRoot = await canonicalCorpusRoot(options.rootDir ?? defaultDir());
  if (!canonicalRoot) return null;
  const fallback = await getMeetingAtCanonicalRoot(
    filePath,
    options,
    canonicalRoot
  );
  if (!fallback) return null;
  // A restricted stub never goes through the CLI overlay path: overlays would
  // add speaker names to a meeting whose content is excluded by default.
  if (fallback.restricted_stub) return fallback;

  const expected = meetingSnapshotContent.get(fallback);
  const expectedPath = fallback.path;
  const reauthorizeSource = async () => {
    const current = await getMeetingAtCanonicalRoot(
      expectedPath,
      options,
      canonicalRoot
    );
    return {
      current,
      unchanged:
        expected !== undefined &&
        current !== null &&
        !current.restricted_stub &&
        current.path === expectedPath &&
        meetingSnapshotContent.get(current) === expected,
    };
  };

  // Dynamically import child_process so this module still loads in
  // environments without it (browsers, Edge runtimes). The function
  // simply degrades to non-overlay behavior in those cases.
  let execFile: typeof import("child_process").execFile;
  let sha256: (content: string) => string;
  try {
    ({ execFile } = await import("child_process"));
    const { createHash } = await import("crypto");
    sha256 = (content) => createHash("sha256").update(content).digest("hex");
  } catch {
    return (await reauthorizeSource()).current;
  }

  const bin = options.minutesBin ?? process.env.MINUTES_BIN ?? "minutes";
  const timeoutMs = options.timeoutMs ?? 10_000;

  const stdout = await new Promise<string | null>((resolve) => {
    try {
      execFile(
        bin,
        ["get", expectedPath, "--json", "--compact-json"],
        { timeout: timeoutMs, maxBuffer: 8 * 1024 * 1024 },
        (err, out) => {
          if (err) resolve(null);
          else resolve(out.toString());
        }
      );
    } catch {
      resolve(null);
    }
  });

  // This fence is deliberately unconditional after the awaited overlay
  // attempt. Even a timeout, non-zero exit, or empty stdout may have raced a
  // normal source into a restricted, unreadable, or malformed state.
  const authorization = await reauthorizeSource();
  if (!authorization.current || authorization.current.restricted_stub) {
    return authorization.current;
  }
  if (expected === undefined || !authorization.unchanged) {
    return authorization.current;
  }

  if (!stdout) return authorization.current;

  try {
    const payload = JSON.parse(stdout);
    const overlaidMap = payload?.frontmatter?.speaker_map;

    if (
      !Array.isArray(overlaidMap) ||
      payload?.overlay_applied !== true ||
      typeof payload?.path !== "string" ||
      !canonicalPathWireEquals(payload.path, expectedPath) ||
      !canonicalPathWireEquals(authorization.current.path, expectedPath) ||
      payload?.overlay_source_sha256 !== sha256(expected)
    ) {
      return authorization.current;
    }

    return {
      ...authorization.current,
      frontmatter: {
        ...authorization.current.frontmatter,
        speaker_map: overlaidMap.map((attr: any) => ({
          speaker_label: String(attr.speaker_label || ""),
          name: String(attr.name || ""),
          confidence: (attr.confidence === "high" ||
            attr.confidence === "medium" ||
            attr.confidence === "low"
            ? attr.confidence
            : "medium") as "high" | "medium" | "low",
          source: parseAttributionSource(String(attr.source || "")),
        })),
      },
    };
  } catch {
    return authorization.current;
  }
}

/**
 * Find open action items across policy-authorized meetings within the
 * supported corpus bounds.
 */
export async function findOpenActions(
  dir: string,
  assignee?: string,
  opts: OpenActionOptions = {}
): Promise<Array<{ path: string; item: ActionItem }>> {
  const boundedLimit = normalizeResultLimit(
    opts.limit ?? SDK_OPEN_ACTION_RESULT_MAX,
    SDK_OPEN_ACTION_RESULT_MAX,
    "findOpenActions"
  );
  return stableMeetingOperation(dir, opts, "find_open_actions", (meetings) => {
    const results: Array<{ path: string; item: ActionItem }> = [];
    for (const meeting of sortByDateDesc(meetings)) {
      for (const item of meeting.frontmatter.action_items) {
        if (item.status !== "open") continue;
        if (
          assignee &&
          item.assignee.toLowerCase() !== assignee.toLowerCase()
        ) {
          continue;
        }
        results.push({ path: meeting.path, item });
        if (results.length >= boundedLimit) return results;
      }
    }
    return results;
  });
}

/**
 * Build a person profile from policy-authorized meetings within the supported
 * corpus bounds that mention them.
 */
export async function getPersonProfile(
  dir: string,
  name: string,
  opts: PersonProfileOptions = {}
): Promise<{
  name: string;
  meetings: Array<{ title: string; date: string; path: string }>;
  openActions: ActionItem[];
  topics: string[];
}> {
  const meetingLimit = normalizeResultLimit(
    opts.meetingLimit ?? SDK_PERSON_PROFILE_MEETING_MAX,
    SDK_PERSON_PROFILE_MEETING_MAX,
    "getPersonProfile meeting"
  );
  const openActionLimit = normalizeResultLimit(
    opts.openActionLimit ?? SDK_PERSON_PROFILE_OPEN_ACTION_MAX,
    SDK_PERSON_PROFILE_OPEN_ACTION_MAX,
    "getPersonProfile open-action"
  );
  const topicLimit = normalizeResultLimit(
    opts.topicLimit ?? SDK_PERSON_PROFILE_TOPIC_MAX,
    SDK_PERSON_PROFILE_TOPIC_MAX,
    "getPersonProfile topic"
  );
  if (!name.trim()) throw new Error("person selector is empty");
  return stableMeetingOperation(dir, opts, "person_profile", (sourceMeetings) => {
    // Resolve against the complete authorized corpus before applying result caps.
    const identity = resolvePersonIdentity(sourceMeetings, name);
    const meetings: Array<{ title: string; date: string; path: string }> = [];
    const openActions: ActionItem[] = [];
    const topicSet = new Set<string>();
    for (const meeting of sortByDateDesc(sourceMeetings)) {
      if (!identity) break;
      if (meetingHasPersonParticipant(meeting, identity)) {
        if (meetings.length < meetingLimit) meetings.push({
          title: meeting.frontmatter.title, date: meeting.frontmatter.date, path: meeting.path,
        });
        for (const tag of meeting.frontmatter.tags) {
          if (topicSet.size >= topicLimit) break;
          topicSet.add(tag);
        }
      }
      for (const item of meeting.frontmatter.action_items) {
        if (openActions.length >= openActionLimit) break;
        if (item.status === "open" && personOwnsAction(meeting, item.assignee, identity)) openActions.push(item);
      }
      if (meetings.length >= meetingLimit && openActions.length >= openActionLimit && topicSet.size >= topicLimit) break;
    }
    return {
      name,
      meetings,
      openActions,
      topics: Array.from(topicSet),
    };
  });
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
  options: VoiceMemoOptions = {}
): Promise<MeetingFile[]> {
  const { days = 14, limit = 20 } = options;
  if (
    !Number.isSafeInteger(days) ||
    days < 0 ||
    days > SDK_VOICE_MEMO_LOOKBACK_MAX_DAYS
  ) {
    throw new RangeError(
      `listVoiceMemos days must be an integer between 0 and ${SDK_VOICE_MEMO_LOOKBACK_MAX_DAYS}`
    );
  }
  const boundedLimit = normalizeResultLimit(
    limit,
    SDK_VOICE_MEMO_RESULT_MAX,
    "listVoiceMemos"
  );
  const cutoff = new Date();
  cutoff.setDate(cutoff.getDate() - days);
  return stableMeetingOperation(dir, options, "list_voice_memos", (meetings) => {
    const results: MeetingFile[] = [];
    for (const meeting of sortByDateDesc(meetings)) {
      if (meeting.frontmatter.type !== "memo") continue;
      if (new Date(meeting.frontmatter.date) < cutoff) continue;
      results.push(meeting);
      if (results.length >= boundedLimit) break;
    }
    return results;
  });
}

/**
 * Find decisions across policy-authorized meetings within the supported corpus
 * bounds, optionally filtered by topic keyword.
 */
export async function findDecisions(
  dir: string,
  topic?: string,
  limit: number = 50,
  opts: ReadOptions = {}
): Promise<Array<{ path: string; title: string; date: string; decision: Decision }>> {
  const boundedLimit = normalizeResultLimit(
    limit,
    SDK_DECISION_RESULT_MAX,
    "findDecisions"
  );
  const topicLower = topic?.toLowerCase();
  return stableMeetingOperation(dir, opts, "find_decisions", (meetings) => {
    const results: Array<{ path: string; title: string; date: string; decision: Decision }> = [];
    for (const meeting of sortByDateDesc(meetings)) {
      for (const decision of meeting.frontmatter.decisions) {
        if (topicLower) {
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
        if (results.length >= boundedLimit) return results;
      }
    }
    return results;
  });
}
