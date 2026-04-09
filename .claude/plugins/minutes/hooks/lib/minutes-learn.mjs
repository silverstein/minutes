#!/usr/bin/env node

import { existsSync, mkdirSync, readFileSync, appendFileSync } from "fs";
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
  const matches = readLearnings()
    .filter((entry) => entry.type === type && entry.key === key)
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
  return appendLearning({
    ts: new Date().toISOString(),
    type,
    key,
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

export function getAliasCluster(name) {
  const normalizedTarget = normalizePersonName(name);
  if (!normalizedTarget) return [];

  const adjacency = new Map();
  const displayNames = new Map();

  for (const entry of readLearnings()) {
    if (entry.type !== "alias") continue;
    const a = normalizePersonName(entry.key || "");
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
  return appendLearning({
    ts: new Date().toISOString(),
    type,
    key,
    value: null,
    source: "explicit",
    confidence: 1.0,
    notes: "cleared",
  });
}
