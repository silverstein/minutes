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
