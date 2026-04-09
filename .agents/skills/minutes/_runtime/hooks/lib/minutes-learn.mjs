#!/usr/bin/env node

import { existsSync, mkdirSync, readFileSync, appendFileSync, readdirSync, statSync } from "fs";
import { join } from "path";
import { homedir } from "os";

const AGENT_DIR = join(homedir(), ".minutes", "agent");
const LEARNINGS_FILE = join(AGENT_DIR, "learnings.jsonl");

const ALLOWED_TYPES = new Set([
  "alias",
  "workflow_preference",
  "nudge_feedback",
  "presentation_preference",
]);

const ALLOWED_SOURCES = new Set(["explicit", "observed", "hook", "skill"]);

function ensureDir() {
  mkdirSync(AGENT_DIR, { recursive: true });
}

function normalizeLearningKey(type, key) {
  if (type === "alias") {
    return normalizePersonName(key);
  }
  return key;
}

export function normalizePersonName(value) {
  return value
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, " ")
    .trim()
    .replace(/\s+/g, " ");
}

export function readLearnings() {
  if (!existsSync(LEARNINGS_FILE)) return [];
  const lines = readFileSync(LEARNINGS_FILE, "utf8")
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  const out = [];
  for (const line of lines) {
    try {
      out.push(JSON.parse(line));
    } catch {
      // Ignore malformed lines rather than crashing the hook.
    }
  }
  return out;
}

export function getLatestLearning(type, key) {
  const normalizedKey = normalizeLearningKey(type, key);
  const matches = readLearnings()
    .filter((entry) => entry.type === type && entry.key === normalizedKey)
    .sort((a, b) => new Date(a.ts).getTime() - new Date(b.ts).getTime());
  return matches[matches.length - 1] ?? null;
}

function appendLearning(entry) {
  ensureDir();
  appendFileSync(LEARNINGS_FILE, `${JSON.stringify(entry)}\n`);
  return entry;
}

export function rememberExplicit(type, key, value, notes = "") {
  if (!ALLOWED_TYPES.has(type)) {
    throw new Error(`Unsupported learning type: ${type}`);
  }
  const normalizedKey = normalizeLearningKey(type, key);
  return appendLearning({
    ts: new Date().toISOString(),
    type,
    key: normalizedKey,
    value,
    source: "explicit",
    confidence: 1.0,
    notes,
  });
}

export function rememberAlias(nameA, nameB, notes = "") {
  const normalizedA = normalizePersonName(nameA);
  const normalizedB = normalizePersonName(nameB);
  if (!normalizedA || !normalizedB) {
    throw new Error("Alias names must both be non-empty");
  }
  if (normalizedA === normalizedB) {
    throw new Error("Alias names normalize to the same value");
  }
  return appendLearning({
    ts: new Date().toISOString(),
    type: "alias",
    key: normalizedA,
    value: {
      name: nameB,
      normalized: normalizedB,
      anchor: nameA,
    },
    source: "explicit",
    confidence: 1.0,
    notes,
  });
}

export function rememberObserved(type, key, value, confidence = 0.7, notes = "") {
  if (!ALLOWED_TYPES.has(type)) {
    throw new Error(`Unsupported learning type: ${type}`);
  }
  if (confidence < 0 || confidence > 1) {
    throw new Error(`Observed confidence must be between 0 and 1`);
  }
  return appendLearning({
    ts: new Date().toISOString(),
    type,
    key,
    value,
    source: "observed",
    confidence,
    notes,
  });
}

export function normalizeLearnings() {
  const latest = new Map();
  for (const entry of readLearnings()) {
    if (!ALLOWED_TYPES.has(entry.type)) continue;
    if (!ALLOWED_SOURCES.has(entry.source)) continue;
    latest.set(`${entry.type}:${entry.key}`, entry);
  }
  return Object.fromEntries(latest.entries());
}

function countMarkdownFiles(dir) {
  if (!existsSync(dir)) return 0;
  return readdirSync(dir).filter((name) => name.endsWith(".md")).length;
}

function markdownFileStats(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((name) => name.endsWith(".md"))
    .map((name) => {
      const filePath = join(dir, name);
      return {
        name,
        path: filePath,
        mtimeMs: statSync(filePath).mtimeMs,
      };
    })
    .sort((a, b) => a.mtimeMs - b.mtimeMs);
}

function latestMarkdownMtime(dir) {
  if (!existsSync(dir)) return 0;
  let latest = 0;
  for (const name of readdirSync(dir)) {
    if (!name.endsWith(".md")) continue;
    const mtime = statSync(join(dir, name)).mtimeMs;
    if (mtime > latest) latest = mtime;
  }
  return latest;
}

export function inferMeetingPrepModeFromUsage(baseDir = homedir()) {
  const prepsDir = join(baseDir, ".minutes", "preps");
  const briefsDir = join(baseDir, ".minutes", "briefs");
  const lookbackMs = 30 * 24 * 60 * 60 * 1000;
  const cutoff = Date.now() - lookbackMs;
  const prepCount = markdownFileStats(prepsDir).filter((file) => file.mtimeMs >= cutoff).length;
  const briefCount = markdownFileStats(briefsDir).filter((file) => file.mtimeMs >= cutoff).length;

  if (prepCount >= 3 && prepCount >= Math.max(1, briefCount * 2)) return "prep";
  if (briefCount >= 3 && briefCount >= Math.max(1, prepCount * 2)) return "brief";
  return "auto";
}

export function recordPendingMeetingPrepNudge(mode, baseDir = homedir()) {
  const prepsDir = join(baseDir, ".minutes", "preps");
  const briefsDir = join(baseDir, ".minutes", "briefs");
  return rememberObserved(
    "nudge_feedback",
    "meeting_prep_nudge_pending",
    {
      mode,
      shown_at: new Date().toISOString(),
      baselinePrepCount: countMarkdownFiles(prepsDir),
      baselineBriefCount: countMarkdownFiles(briefsDir),
      baselinePrepMtime: latestMarkdownMtime(prepsDir),
      baselineBriefMtime: latestMarkdownMtime(briefsDir),
    },
    0.8,
    "SessionStart reminder emitted",
  );
}

export function finalizePendingMeetingPrepNudge(baseDir = homedir()) {
  const pending = getLatestLearning("nudge_feedback", "meeting_prep_nudge_pending");
  if (!pending?.value?.shown_at) return null;

  const shownAt = new Date(pending.value.shown_at).getTime();
  if (!Number.isFinite(shownAt)) return null;

  const now = Date.now();
  const ageMs = now - shownAt;
  const windowMs = 6 * 60 * 60 * 1000;
  const prepsDir = join(baseDir, ".minutes", "preps");
  const briefsDir = join(baseDir, ".minutes", "briefs");

  const prepFilesAfter = markdownFileStats(prepsDir).filter((file) => file.mtimeMs > shownAt);
  const briefFilesAfter = markdownFileStats(briefsDir).filter((file) => file.mtimeMs > shownAt);

  const prepAdvanced = prepFilesAfter.length > 0;
  const briefAdvanced = briefFilesAfter.length > 0;

  let outcome = null;
  let observedMode = pending.value.mode || "auto";

  if (prepAdvanced || briefAdvanced) {
    outcome = "engaged";
    if (prepAdvanced && !briefAdvanced) observedMode = "prep";
    if (briefAdvanced && !prepAdvanced) observedMode = "brief";
  } else if (ageMs >= windowMs) {
    outcome = "ignored";
  }

  if (!outcome) return null;

  rememberObserved(
    "nudge_feedback",
    "meeting_prep_nudge_outcome",
    {
      mode: observedMode,
      outcome,
      shown_at: pending.value.shown_at,
    },
    0.7,
    "Finalized pending SessionStart reminder",
  );
  clearLearning("nudge_feedback", "meeting_prep_nudge_pending");
  return { mode: observedMode, outcome };
}

export function shouldSuppressMeetingPrepNudge() {
  const lookbackMs = 7 * 24 * 60 * 60 * 1000;
  const cutoff = Date.now() - lookbackMs;
  const outcomes = readLearnings()
    .filter(
      (entry) =>
        entry.type === "nudge_feedback" &&
        entry.key === "meeting_prep_nudge_outcome" &&
        new Date(entry.ts).getTime() >= cutoff,
    )
    .sort((a, b) => new Date(a.ts).getTime() - new Date(b.ts).getTime())
    .slice(-4);

  if (outcomes.length < 3) return false;
  const lastThree = outcomes.slice(-3);
  return lastThree.every((entry) => entry.value?.outcome === "ignored");
}

export function getAliasCluster(name) {
  const normalizedTarget = normalizePersonName(name);
  if (!normalizedTarget) return [];

  const adjacency = new Map();
  const displayNames = new Map();

  const entries = readLearnings().sort(
    (a, b) => new Date(a.ts).getTime() - new Date(b.ts).getTime(),
  );

  for (const entry of entries) {
    if (entry.type !== "alias") continue;
    const a = normalizePersonName(entry.key || "");
    if (!a) continue;
    if (entry.value == null) {
      const neighbors = adjacency.get(a);
      if (neighbors) {
        for (const neighbor of neighbors) {
          adjacency.get(neighbor)?.delete(a);
        }
      }
      adjacency.set(a, new Set());
      continue;
    }
    const b = normalizePersonName(entry.value?.normalized || entry.value?.name || "");
    const displayA = entry.value?.anchor || entry.key;
    const displayB = entry.value?.name || entry.value?.normalized;
    if (!a || !b) continue;

    if (!adjacency.has(a)) adjacency.set(a, new Set());
    if (!adjacency.has(b)) adjacency.set(b, new Set());
    adjacency.get(a).add(b);
    adjacency.get(b).add(a);

    if (displayA) displayNames.set(a, displayA);
    if (displayB) displayNames.set(b, displayB);
  }

  const visited = new Set([normalizedTarget]);
  const queue = [normalizedTarget];
  while (queue.length > 0) {
    const current = queue.shift();
    for (const neighbor of adjacency.get(current) || []) {
      if (visited.has(neighbor)) continue;
      visited.add(neighbor);
      queue.push(neighbor);
    }
  }

  const aliases = [];
  for (const normalized of visited) {
    aliases.push({
      normalized,
      name: displayNames.get(normalized) || normalized,
    });
  }

  aliases.sort((a, b) => a.name.localeCompare(b.name));
  return aliases;
}

export function clearLearning(type, key) {
  if (!ALLOWED_TYPES.has(type)) {
    throw new Error(`Unsupported learning type: ${type}`);
  }
  const normalizedKey = normalizeLearningKey(type, key);
  return appendLearning({
    ts: new Date().toISOString(),
    type,
    key: normalizedKey,
    value: null,
    source: "explicit",
    confidence: 1.0,
    notes: "cleared",
  });
}
