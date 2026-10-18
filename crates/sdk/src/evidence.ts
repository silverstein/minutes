import { createHash } from "node:crypto";
import { foldSearchText, matchesSearchToken, queryTokens } from "./retrieval.js";

export interface EvidenceSource {
  path: string;
  /** Exact, already-authorized UTF-8 decoded Markdown snapshot. */
  content: string;
  body: string;
  speakerMap?: Array<{ speaker_label: string; name: string; confidence: string; source: string }>;
}

export interface SourceEvidence {
  id: string;
  source_id: string;
  source_revision: string;
  path: string;
  quote: string;
  /** Zero-based offsets into the original Markdown, measured in UTF-16 code units. */
  start: number;
  end: number;
  offset_unit: "utf16";
  line_start: number;
  line_end: number;
  quote_truncated?: true;
  timestamp_seconds?: number;
  speaker?: { label: string; name?: string; confidence?: string; provenance?: string };
}

export interface EvidenceBundle {
  source_id: string;
  source_revision: string;
  segments: SourceEvidence[];
  truncated: boolean;
}

export function evidenceHash(value: string): string {
  return createHash("sha256").update(value, "utf8").digest("hex");
}

// Weak metadata cache avoids hashing an entire source for every decision span.
// Compare both immutable strings so a caller mutating its input object cannot
// reuse a stale revision. The line cursor is constant-space and restarts on
// out-of-order spans; no unbounded array of per-line metadata is retained.
const sourceMetadata = new WeakMap<EvidenceSource, {
  path: string; content: string; sourceId: string; revision: string; lineOffset: number; line: number;
}>();
function metadata(source: EvidenceSource) {
  let value = sourceMetadata.get(source);
  if (!value || value.path !== source.path || value.content !== source.content) {
    value = { path: source.path, content: source.content,
      sourceId: evidenceHash(`minutes-source-v1\0${source.path}`), revision: evidenceHash(source.content),
      lineOffset: 0, line: 1 };
    sourceMetadata.set(source, value);
  }
  return value;
}
function lineNumber(source: EvidenceSource, offset: number): number {
  const value = metadata(source);
  if (offset < value.lineOffset) { value.lineOffset = 0; value.line = 1; }
  let newline = source.content.indexOf("\n", value.lineOffset);
  while (newline >= 0 && newline < offset) {
    value.line += 1; value.lineOffset = newline + 1;
    newline = source.content.indexOf("\n", value.lineOffset);
  }
  value.lineOffset = offset;
  return value.line;
}

/** Pure derivation only. A returned path is a citation, never an audio/file access grant. */
export function evidenceForSpan(source: EvidenceSource, start: number, end: number): SourceEvidence {
  const value = metadata(source);
  return spanEvidence(source, start, end, value.sourceId, value.revision);
}

function spanEvidence(source: EvidenceSource, start: number, end: number, source_id: string, source_revision: string): SourceEvidence {
  if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end <= start || end > source.content.length) {
    throw new RangeError("Evidence span is outside its source snapshot");
  }
  const cutsSurrogatePair = (offset: number) => offset > 0 && offset < source.content.length &&
    /[\uD800-\uDBFF]/.test(source.content[offset - 1]) && /[\uDC00-\uDFFF]/.test(source.content[offset]);
  if (cutsSurrogatePair(start) || cutsSurrogatePair(end)) throw new RangeError("Evidence span splits a Unicode character");
  const requestedEnd = end;
  end = Math.min(end, start + 1200);
  // Never split a surrogate pair while bounding the response.
  if (end < requestedEnd && /[\uD800-\uDBFF]/.test(source.content[end - 1])) end -= 1;
  const quote = source.content.slice(start, end);
  const line_start = lineNumber(source, start);
  const line_end = line_start + (quote.match(/\n/g)?.length ?? 0);
  const evidence: SourceEvidence = {
    id: evidenceHash(`minutes-segment-v1\0${source_id}\0${source_revision}\0${start}\0${end}`),
    source_id, source_revision, path: source.path, quote, start, end, offset_unit: "utf16", line_start, line_end,
  };
  if (end < requestedEnd) evidence.quote_truncated = true;
  const timestamp = quote.match(/^\s*(?:[-*]\s*)?\[(?:(.+?)\s+)?(\d+(?::\d{2}){1,2}(?:\.\d+)?)\]/);
  if (timestamp) {
    const parts = timestamp[2].split(":").map(Number);
    if (parts.every(Number.isFinite) && parts.slice(1).every((part) => part >= 0 && part < 60) && Number.isFinite(parts.reduce((seconds, part) => seconds * 60 + part, 0))) {
      evidence.timestamp_seconds = parts.reduce((seconds, part) => seconds * 60 + part, 0);
    }
    if (timestamp[1]) {
      const label = timestamp[1];
      const attribution = source.speakerMap?.find((entry) => entry.speaker_label === label);
      evidence.speaker = { label };
      if (attribution) {
        evidence.speaker.confidence = attribution.confidence;
        evidence.speaker.provenance = attribution.source;
        // An uncertain attribution is evidence about a label, not a named identity.
        if (attribution.confidence === "high") evidence.speaker.name = attribution.name;
      }
    }
  }
  return evidence;
}

export function buildSourceEvidence(source: EvidenceSource, options: { query?: string; limit?: number } = {}): EvidenceBundle {
  const limit = options.limit ?? 32;
  if (!Number.isInteger(limit) || limit < 1 || limit > 128) throw new RangeError("Evidence limit must be between 1 and 128");
  if (!source.content.endsWith(source.body)) throw new Error("Evidence body does not match its source snapshot");
  const tokens = queryTokens(options.query ?? "");
  let offset = source.content.length - source.body.length;
  const candidates: Array<{ start: number; end: number; score: number; clipped: boolean }> = [];
  for (const line of source.body.split("\n")) {
    if (line.trim()) {
      const lineTokens = Array.from(line.matchAll(/[\p{L}\p{N}\p{M}]+/gu), (match) => ({ token: foldSearchText(match[0]), offset: match.index }));
      const score = tokens.reduce((count, wanted, index) => count + Number(lineTokens.some(({ token }) => matchesSearchToken(token, wanted, index === tokens.length - 1))), 0);
      if (tokens.length === 0 || score > 0) {
        const hit = lineTokens.find(({ token }) => tokens.some((wanted, index) => matchesSearchToken(token, wanted, index === tokens.length - 1)));
        let start = offset + Math.max(0, (hit?.offset ?? 0) - 120);
        if (start > offset && /[\uD800-\uDBFF]/.test(source.content[start - 1]) && /[\uDC00-\uDFFF]/.test(source.content[start])) start += 1;
        candidates.push({ start, end: offset + line.length, score, clipped: start > offset });
      }
    }
    offset += line.length + 1;
  }
  candidates.sort((a, b) => b.score - a.score || a.start - b.start);
  const { sourceId: source_id, revision: source_revision } = metadata(source);
  const segments = candidates.slice(0, limit).map((span) => {
    const evidence = spanEvidence(source, span.start, span.end, source_id, source_revision);
    if (span.clipped) evidence.quote_truncated = true;
    return evidence;
  });
  return { source_id, source_revision, segments, truncated: candidates.length > limit || segments.some((segment) => segment.quote_truncated) };
}
