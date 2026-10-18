import { parseDocument } from "yaml";
import { evidenceForSpan, evidenceHash, type SourceEvidence } from "./evidence.js";

export interface HistoricalDecision {
  id?: string;
  text: string;
  topic?: string;
  authority?: string;
  supersedes?: string;
  effective_date?: string;
}

export interface DecisionHistorySource {
  path: string;
  content: string;
  body: string;
  frontmatter: { date: string; decisions: HistoricalDecision[] };
}

export interface DecisionHistoryEntry {
  id: string;
  declared_id?: string;
  text: string;
  topic?: string;
  authority?: string;
  recorded_at: string;
  effective_date: string;
  effective_date_basis: "explicit" | "recorded-at";
  status: "current" | "superseded" | "unresolved" | "unknown";
  supersedes?: string;
  superseded_by?: string;
  unresolved_reason?: string;
  /** Exact source YAML, not a claim that a model extraction is a transcript quote. */
  evidence: SourceEvidence[];
}

const normalize = (value: string) => value.normalize("NFKC").trim().replace(/\s+/g, " ").toLowerCase();

/** ISO calendar date or RFC3339 instant; reject JS Date.parse rollovers/shorthand. */
function decisionTime(value: string): number {
  if (typeof value !== "string") return NaN;
  const parts = value.match(/^(\d{4})-(\d{2})-(\d{2})(?:T(\d{2}):(\d{2}):(\d{2})(?:\.\d{1,9})?(Z|[+-]\d{2}:\d{2}))?$/);
  if (!parts) return NaN;
  const [, year, month, day, hour, minute, second, zone] = parts;
  const calendar = new Date(0);
  calendar.setUTCFullYear(Number(year), Number(month) - 1, Number(day));
  if (calendar.getUTCFullYear() !== Number(year) || calendar.getUTCMonth() !== Number(month) - 1 || calendar.getUTCDate() !== Number(day)) return NaN;
  if (hour !== undefined && (Number(hour) > 23 || Number(minute) > 59 || Number(second) > 59)) return NaN;
  if (zone && zone !== "Z" && (Number(zone.slice(1, 3)) > 23 || Number(zone.slice(4)) > 59)) return NaN;
  return Date.parse(value);
}

/**
 * Pure projection over authorized source snapshots. A changed statement is not
 * assumed to contradict or supersede another. Only a unique explicit link can
 * close a prior decision. No filesystem reads, embeddings, or model inference.
 */
export function buildDecisionHistory(sources: DecisionHistorySource[], options: { asOf?: string; limit?: number } = {}): DecisionHistoryEntry[] {
  const limit = options.limit ?? 1000;
  if (!Number.isInteger(limit) || limit < 1 || limit > 1000) throw new RangeError("Decision history limit must be between 1 and 1000");
  const asOf = options.asOf === undefined ? Date.now() : decisionTime(options.asOf);
  if (!Number.isFinite(asOf)) throw new RangeError("Decision history asOf must be a valid date");
  const records: Array<{ entry: DecisionHistoryEntry; raw: HistoricalDecision; path: string }> = [];
  for (const source of sources) {
    const sourceRevision = evidenceHash(source.content);
    const yamlEnd = source.content.indexOf("\n---", 3);
    const document = yamlEnd >= 0 ? parseDocument(source.content.slice(3, yamlEnd)) : undefined;
    for (const [index, raw] of source.frontmatter.decisions.entries()) {
      if (!raw.text.trim()) continue;
      const node = document?.getIn(["decisions", index, "text"], true) as { range?: number[] } | undefined;
      const evidence = node?.range
        ? [evidenceForSpan(source, 3 + node.range[0], 3 + node.range[1])]
        : [];
      const id = evidenceHash(`minutes-decision-v1\0${source.path}\0${sourceRevision}\0${index}`);
      const effective_date = raw.effective_date ?? source.frontmatter.date;
      const entry: DecisionHistoryEntry = {
        id, declared_id: raw.id, text: raw.text, topic: raw.topic, authority: raw.authority,
        recorded_at: source.frontmatter.date, effective_date,
        effective_date_basis: raw.effective_date === undefined ? "recorded-at" : "explicit",
        status: "unknown", evidence,
      };
      if (!raw.topic?.trim()) entry.unresolved_reason = "missing-topic";
      else if (!Number.isFinite(decisionTime(source.frontmatter.date))) entry.unresolved_reason = "invalid-recorded-date";
      else if (!Number.isFinite(decisionTime(effective_date))) entry.unresolved_reason = "invalid-effective-date";
      else if (decisionTime(effective_date) > asOf || decisionTime(source.frontmatter.date) > asOf) entry.unresolved_reason = "not-yet-effective";
      else if (evidence.length === 0) entry.unresolved_reason = "source-span-unavailable";
      else entry.status = "unresolved";
      records.push({ entry, raw, path: source.path });
      if (records.length > 10_000) throw new RangeError("Decision history exceeds its 10000-record analysis bound");
    }
  }
  const active = records.filter(({ entry }) => entry.status !== "unknown");
  // Resolve references through a bounded index rather than scanning the entire
  // history for every edge. Keep at most two candidates: two already means
  // ambiguity, which must never be resolved by arbitrary traversal order.
  const references = new Map<string, Array<typeof active[number]>>();
  const referenceKey = (topic: string | undefined, kind: string, value: string) =>
    `${normalize(topic ?? "")}\0${kind}\0${value}`;
  for (const record of active) {
    const refs: Array<[string, string | undefined]> = [
      ["id", record.entry.id], ["id", record.raw.id],
      ["text", normalize(record.entry.text)], ["date", record.entry.recorded_at.slice(0, 10)],
    ];
    for (const [kind, value] of refs) {
      if (!value) continue;
      const key = referenceKey(record.entry.topic, kind, value);
      const candidates = references.get(key) ?? [];
      if (!candidates.includes(record) && candidates.length < 2) candidates.push(record);
      references.set(key, candidates);
    }
  }
  const links: Array<{ record: typeof active[number]; previous: DecisionHistoryEntry }> = [];
  for (const record of active) {
    const target = record.raw.supersedes?.trim();
    if (!target) continue;
    const candidates = [...new Set([
      ...(references.get(referenceKey(record.entry.topic, "id", target)) ?? []),
      ...(references.get(referenceKey(record.entry.topic, "text", normalize(target))) ?? []),
      ...(references.get(referenceKey(record.entry.topic, "date", target)) ?? []),
    ])];
    if (candidates.length !== 1 || candidates[0] === record || decisionTime(candidates[0].entry.recorded_at) >= decisionTime(record.entry.recorded_at) || decisionTime(candidates[0].entry.effective_date) > decisionTime(record.entry.effective_date)) {
      record.entry.unresolved_reason = candidates.length > 1 ? "ambiguous-supersedes" : "unresolved-supersedes";
      continue;
    }
    links.push({ record, previous: candidates[0].entry });
  }
  // Validate the whole graph before assigning edges. Competing successors must
  // never pick a winner based on filesystem traversal or array insertion order.
  const successorCounts = new Map<string, number>();
  for (const { previous } of links) successorCounts.set(previous.id, (successorCounts.get(previous.id) ?? 0) + 1);
  for (const { record, previous } of links) {
    if (successorCounts.get(previous.id)! > 1) {
      record.entry.unresolved_reason = "competing-supersession";
    }
  }
  // Earlier records are validated first, so missing/competing ancestry cannot
  // disappear merely because a later record names an unresolved predecessor.
  for (const { record, previous } of links.sort((a, b) =>
    decisionTime(a.record.entry.recorded_at) - decisionTime(b.record.entry.recorded_at)
    || a.record.entry.id.localeCompare(b.record.entry.id))) {
    if (record.entry.unresolved_reason) continue;
    if (previous.unresolved_reason) {
      record.entry.unresolved_reason = "unresolved-supersession-ancestry";
      continue;
    }
    record.entry.supersedes = previous.id;
    previous.superseded_by = record.entry.id;
    previous.status = "superseded";
  }
  const topics = new Map<string, DecisionHistoryEntry[]>();
  for (const { entry } of active) {
    if (entry.status === "superseded") continue;
    const key = normalize(entry.topic ?? "");
    const group = topics.get(key) ?? [];
    group.push(entry);
    topics.set(key, group);
  }
  for (const group of topics.values()) {
    const statements = new Set(group.map((entry) => normalize(entry.text)));
    const resolvable = statements.size === 1 && group.every((entry) => !entry.unresolved_reason);
    for (const entry of group) {
      entry.status = resolvable ? "current" : "unresolved";
      if (!resolvable) entry.unresolved_reason ??= "multiple-statements-without-explicit-resolution";
    }
  }
  return records.map(({ entry }) => entry).sort((a, b) =>
    (Date.parse(b.recorded_at) || 0) - (Date.parse(a.recorded_at) || 0) || a.id.localeCompare(b.id)
  ).slice(0, limit);
}
