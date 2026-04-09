#!/usr/bin/env node

/**
 * SessionStart hook: proactive meeting reminder + plugin update check.
 *
 * When a Claude Code session starts, check if the user has a meeting
 * in the next 60 minutes. If so, nudge them to run /minutes-brief
 * (or /minutes-prep if they want to think harder about goals first).
 *
 * Also surfaces voice memos from the last 3 days, relationship-graph
 * intelligence (losing-touch alerts, stale commitments), and a
 * once-per-day check for newer plugin versions on GitHub — so the
 * agent walks into the session already aware.
 *
 * Guards against being annoying:
 * - Only fires on startup (not resume/compact/clear)
 * - Only fires if the user has actively used a Minutes skill before
 *   (~/.minutes/preps/ OR ~/.minutes/briefs/ exists)
 * - Only fires during business hours (8am-6pm, weekdays)
 * - Can be disabled via ~/.config/minutes/config.toml:
 *     [reminders]
 *     enabled = false
 *   And the update check separately:
 *     [updates]
 *     check = false
 *
 * Hook event: SessionStart
 * Matcher: startup
 */

import { existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "fs";
import { join } from "path";
import { homedir } from "os";
import {
  finalizePendingMeetingPrepNudge,
  getLatestLearning,
  inferMeetingPrepModeFromUsage,
  recordPendingMeetingPrepNudge,
  shouldSuppressMeetingPrepNudge,
} from "./lib/minutes-learn.mjs";

// Only run on startup, not resume/compact/clear
const input = JSON.parse(process.argv[2] || "{}");
const event = input.session_event || input.event || "";

if (event !== "startup") process.exit(0);

// Guard 1: Only nudge if the user has actively used a Minutes skill before.
// They've adopted the workflow if either preps/ or briefs/ exists.
const prepsDir = join(homedir(), ".minutes", "preps");
const briefsDir = join(homedir(), ".minutes", "briefs");
if (!existsSync(prepsDir) && !existsSync(briefsDir)) process.exit(0);

// Guard 2: Only fire during business hours (8am-6pm, weekdays)
const now = new Date();
const hour = now.getHours();
const day = now.getDay(); // 0=Sun, 6=Sat
if (day === 0 || day === 6 || hour < 8 || hour >= 18) process.exit(0);

// Guard 3: Check config for opt-out. We look for `enabled = false` scoped to
// the [reminders] section specifically. The earlier `includes("enabled = false")
// && includes("[reminders]")` shortcut false-positived on configs like
//   [audio]
//   enabled = false
//   [reminders]
//   enabled = true
// where an unrelated section's `enabled = false` would silence reminders even
// though the user explicitly enabled them. The regex below scopes the check by
// requiring `enabled = false` to appear inside the `[reminders]` block
// (i.e. before any subsequent `[section]` header).
const configPath = join(homedir(), ".config", "minutes", "config.toml");
if (existsSync(configPath)) {
  try {
    const config = readFileSync(configPath, "utf-8");
    if (/\[reminders\][^\[]*\benabled\s*=\s*false\b/.test(config)) {
      process.exit(0);
    }
  } catch {
    // Config unreadable — continue
  }
}

// Scan for recent voice memos (last 3 days, max 5)
let memoContext = "";
try {
  const memosDir = join(homedir(), "meetings", "memos");
  if (existsSync(memosDir)) {
    const { readdirSync, statSync } = await import("fs");
    const cutoff = Date.now() - 3 * 24 * 60 * 60 * 1000; // 3 days
    const files = readdirSync(memosDir)
      .filter((f) => f.endsWith(".md"))
      .map((f) => {
        const full = join(memosDir, f);
        const mtime = statSync(full).mtimeMs;
        return { name: f, path: full, mtime };
      })
      .filter((f) => f.mtime >= cutoff)
      .sort((a, b) => b.mtime - a.mtime)
      .slice(0, 5);

    if (files.length > 0) {
      const memoLines = files.map((f) => {
        // Extract title from frontmatter (first line after ---)
        try {
          const content = readFileSync(f.path, "utf-8");
          const titleMatch = content.match(/^title:\s*(.+)$/m);
          const dateMatch = content.match(/^date:\s*(.+)$/m);
          const title = titleMatch ? titleMatch[1].trim() : f.name.replace(".md", "");
          const date = dateMatch
            ? new Date(dateMatch[1].trim()).toLocaleDateString("en-US", { month: "short", day: "numeric" })
            : "recent";
          return `[${date}] ${title}`;
        } catch {
          return f.name.replace(".md", "");
        }
      });
      memoContext = `\n\nRecent voice memos: ${memoLines.join(", ")}. The user may ask about these — use search_meetings or get_meeting MCP tools to retrieve details.`;
    }
  }
} catch {
  // Non-fatal — skip voice memo scan
}

// Scan relationship graph for proactive intelligence (from SQLite index)
let relationshipContext = "";
try {
  const { execFileSync } = await import("child_process");
  const minutesBin = join(homedir(), ".local", "bin", "minutes");
  if (existsSync(minutesBin)) {
    // Get people data (auto-rebuilds if needed)
    const peopleRaw = execFileSync(minutesBin, ["people", "--json", "--limit", "10"], {
      encoding: "utf-8",
      timeout: 3000,
    });
    const people = JSON.parse(peopleRaw);

    if (Array.isArray(people) && people.length > 0) {
      // Losing touch alerts
      const losingTouch = people.filter((p) => p.losing_touch);
      if (losingTouch.length > 0) {
        const alerts = losingTouch
          .slice(0, 3)
          .map((p) => `${p.name} (${p.meeting_count} meetings, last ${Math.round(p.days_since)}d ago)`)
          .join(", ");
        relationshipContext += `\n\nLosing touch: ${alerts}. Consider reaching out.`;
      }

      // Stale commitments
      try {
        const commitsRaw = execFileSync(minutesBin, ["commitments", "--json"], {
          encoding: "utf-8",
          timeout: 3000,
        });
        const commitments = JSON.parse(commitsRaw);
        const stale = Array.isArray(commitments) ? commitments.filter((c) => c.status === "stale") : [];
        if (stale.length > 0) {
          const staleList = stale
            .slice(0, 3)
            .map((c) => `"${c.text}" for ${c.person_name || "unknown"}`)
            .join("; ");
          relationshipContext += `\n\nStale commitments (overdue): ${staleList}. Mention if relevant to today's work.`;
        }
      } catch {
        // Non-fatal
      }
    }
  }
} catch {
  // Non-fatal — relationship graph not available or not yet built
}

// ─── Plugin update check ──────────────────────────────────────────────
// Adapted from garrytan/gstack's bin/gstack-update-check pattern. Fetches
// the canonical plugin.json from raw.githubusercontent.com once per cache
// window, compares to the locally-installed version, and injects an update
// notice into additionalContext when a newer version exists. Respects
// per-version escalating-backoff snooze state so users who say "not now"
// don't get spammed, and a separate opt-out config so users who never
// want the check can turn it off entirely.
//
// Cache TTLs (match gstack's original tuning):
//   UP_TO_DATE:        60 min  — detect new releases quickly
//   UPGRADE_AVAILABLE: 12 hrs  — keep the nag visible but not every session
//
// Snooze levels (same escalation ladder as gstack):
//   level 1: 24 hrs, level 2: 48 hrs, level 3+: 7 days
//   A new remote version resets the snooze so users get re-notified
//   immediately when a newer release drops.
//
// Opt-out: ~/.config/minutes/config.toml
//   [updates]
//   check = false
//
// All failures here are silent. Network errors, GitHub blips, curl missing,
// corrupt cache, malformed remote response — none of them may block the
// session-reminder hook from firing the rest of its work.
let updateContext = "";
try {
  // Respect [updates] check = false opt-out. Uses the same scoped-regex
  // pattern as the [reminders] opt-out above so `[audio] enabled = false`
  // can't false-positive into silencing this check either.
  let updateCheckDisabled = false;
  if (existsSync(configPath)) {
    try {
      const cfg = readFileSync(configPath, "utf-8");
      if (/\[updates\][^\[]*\bcheck\s*=\s*false\b/.test(cfg)) {
        updateCheckDisabled = true;
      }
    } catch {
      // Config unreadable — fall through (check remains enabled)
    }
  }

  if (!updateCheckDisabled) {
    const stateDir = join(homedir(), ".minutes");
    const cacheFile = join(stateDir, "update-check-cache");
    const snoozeFile = join(stateDir, "update-snoozed");
    const pluginRoot = process.env.CLAUDE_PLUGIN_ROOT || "";
    const localPluginJson = join(pluginRoot, ".claude-plugin", "plugin.json");

    // Read local version from the canonical metadata file
    let localVersion = "";
    try {
      const parsed = JSON.parse(readFileSync(localPluginJson, "utf-8"));
      if (typeof parsed.version === "string") {
        localVersion = parsed.version;
      }
    } catch {
      // No local plugin.json or unparseable — skip entirely. The hook may
      // be running outside the plugin environment (e.g., during testing).
    }

    // Semver validation: reject HTML error pages, reject garbage. Accepts
    // N.N.N with optional pre-release suffix (e.g. 0.8.0-rc.1).
    const looksLikeVersion = (v) => /^\d+\.\d+\.\d+(-[\w.]+)?$/.test(v);

    // Semver compare — returns true if `b` is strictly newer than `a`.
    // Pre-release suffixes are ignored for the comparison; any pre-release
    // version is treated as lower-precedence than a final at the same core
    // version, which matches semver spec closely enough for our use.
    const isNewer = (a, b) => {
      const core = (v) => v.split("-")[0].split(".").map((n) => parseInt(n, 10) || 0);
      const pa = core(a);
      const pb = core(b);
      const len = Math.max(pa.length, pb.length);
      for (let i = 0; i < len; i++) {
        const da = pa[i] || 0;
        const db = pb[i] || 0;
        if (db > da) return true;
        if (db < da) return false;
      }
      // Core versions equal — treat pre-release as lower precedence
      const aPre = a.includes("-");
      const bPre = b.includes("-");
      if (aPre && !bPre) return true;
      return false;
    };

    // Check snooze state for a given remote version. Returns true if the
    // update is currently snoozed and we should stay quiet.
    const isSnoozed = (remoteVer) => {
      try {
        if (!existsSync(snoozeFile)) return false;
        const raw = readFileSync(snoozeFile, "utf-8").trim();
        const [snoozedVer, levelStr, epochStr] = raw.split(/\s+/);
        if (!snoozedVer || !levelStr || !epochStr) return false;
        const level = parseInt(levelStr, 10);
        const epoch = parseInt(epochStr, 10);
        if (!Number.isFinite(level) || !Number.isFinite(epoch)) return false;
        // New version? Ignore the snooze — user should be re-prompted.
        if (snoozedVer !== remoteVer) return false;
        const duration = level <= 1 ? 86400 : level === 2 ? 172800 : 604800;
        return Date.now() / 1000 < epoch + duration;
      } catch {
        return false;
      }
    };

    if (localVersion && looksLikeVersion(localVersion)) {
      let remoteVersion = "";

      // Step 1: Try the cache first. Cache is only valid for the CURRENT
      // local version — if the user just manually ran /plugin update and
      // the local bumped, the cache is stale and we re-fetch.
      let useCache = false;
      if (existsSync(cacheFile)) {
        try {
          const raw = readFileSync(cacheFile, "utf-8").trim();
          const parts = raw.split(/\s+/);
          const state = parts[0];
          const ageMs = Date.now() - statSync(cacheFile).mtimeMs;
          const ttlMs =
            state === "UP_TO_DATE"
              ? 60 * 60 * 1000
              : state === "UPGRADE_AVAILABLE"
              ? 12 * 60 * 60 * 1000
              : 0;
          if (ttlMs > 0 && ageMs < ttlMs && parts[1] === localVersion) {
            useCache = true;
            if (state === "UPGRADE_AVAILABLE") {
              remoteVersion = parts[2] || "";
            }
          }
        } catch {
          // Corrupt cache — fall through to re-fetch
        }
      }

      // Step 2: Slow path — fetch the remote plugin.json. Only runs when
      // the cache is stale or missing. 3-second timeout keeps the hook
      // responsive even if GitHub is slow or the user is offline.
      if (!useCache) {
        try {
          const { execFileSync } = await import("child_process");
          const remoteUrl =
            "https://raw.githubusercontent.com/silverstein/minutes/main/.claude/plugins/minutes/.claude-plugin/plugin.json";
          const raw = execFileSync(
            "curl",
            ["-sf", "--max-time", "3", remoteUrl],
            {
              encoding: "utf-8",
              stdio: ["ignore", "pipe", "ignore"],
              timeout: 4000,
            }
          );
          const parsed = JSON.parse(raw);
          const ver = typeof parsed.version === "string" ? parsed.version : "";
          if (looksLikeVersion(ver)) {
            const state = isNewer(localVersion, ver)
              ? `UPGRADE_AVAILABLE ${localVersion} ${ver}`
              : `UP_TO_DATE ${localVersion}`;
            mkdirSync(stateDir, { recursive: true });
            writeFileSync(cacheFile, state);
            if (isNewer(localVersion, ver)) {
              remoteVersion = ver;
            }
          }
        } catch {
          // Network error, parse error, curl missing, timeout — silent fallback.
        }
      }

      // Step 3: Inject a notice if a newer version is available and not snoozed.
      // The instruction block tells Claude how to respond to user actions
      // (snooze / disable) by writing the appropriate state files, since
      // the hook itself can't run AskUserQuestion.
      //
      // The recommended upgrade sequence is TWO commands plus a restart, NOT
      // a single /plugin update. Claude Code's marketplace is backed by a
      // local git mirror at ~/.claude/plugins/marketplaces/<name>/ that only
      // refreshes when you explicitly ask for it — /plugin update alone
      // consults the stale mirror and reports "already at latest" even when
      // the upstream has moved far ahead. You must refresh the mirror first.
      // See docs/PRE-RELEASE-CHECKLIST.md for the full background on why.
      if (remoteVersion && isNewer(localVersion, remoteVersion) && !isSnoozed(remoteVersion)) {
        updateContext = `\n\nMinutes plugin update available: v${remoteVersion} (user is on v${localVersion}). Mention it ONCE in ONE line early in the response — do not harp on it. Tell them the upgrade takes THREE steps, because Claude Code's marketplace has a local git mirror that must be refreshed first:

  1. /plugin marketplace update minutes   (git-pulls the local marketplace mirror so Claude Code knows v${remoteVersion} exists)
  2. /plugin update minutes@minutes       (installs the new version into the cache — this step alone is a no-op if you skip step 1)
  3. Restart Claude Code                  (loads the new skills, hooks, and scripts into the session)

A plain \`/plugin update minutes\` by itself will report "already at latest" because the mirror is stuck — this is a real marketplace quirk, not user error. Give them the full three-step sequence every time.

If the user says "snooze", "not now", "remind me later", or similar: read ~/.minutes/update-snoozed if it exists (format: "<version> <level> <epoch>"), increment level (cap at 3), and write "${remoteVersion} <new_level> $(date +%s)" back. Levels: 1=24h, 2=48h, 3+=7d quiet.

If the user says "never ask again", "stop reminding me", or "disable updates": append "[updates]\\ncheck = false\\n" to ~/.config/minutes/config.toml (create the file if missing). Tell them they can re-enable by removing that block.

If the user ignores the update mention and stays on task, do not bring it up again this session.`;
      }
    }
  }
} catch {
  // Top-level catch — any unhandled error in the update check path must
  // not block the session-reminder hook from firing the rest of its work.
}

// Calendar context: three-way decision tree.
//   (1) Try osascript (Apple Calendar) locally — the precise path. If we can
//       verify there's a meeting in the next 60 min, inject a specific
//       recommendation. If we can verify there's NOTHING coming up, inject
//       zero extra context — this is the "zero cost when quiet" win that lets
//       us justify running this hook on every startup. Commit 0b8adea once
//       removed this hook for being too chatty; earning that back means
//       staying silent when there's nothing to say.
//   (2) If the local check fails for any reason (non-Mac, Calendar.app not
//       running, permission denied, timeout), fall back to the lightweight
//       MCP-check instruction so Claude can still help users on Google
//       Calendar via gcal_list_events MCP.
//   (3) If raw is empty AND the local check succeeded, skip injection entirely.
let calendarContext = "";
let localCheckResolved = false;
finalizePendingMeetingPrepNudge();
const learnedPrepMode =
  getLatestLearning("workflow_preference", "meeting_prep_mode")?.value || "auto";
const learnedNudgeMode =
  getLatestLearning("nudge_feedback", "meeting_prep_nudge")?.value || "active";
const observedPrepMode = inferMeetingPrepModeFromUsage();
const effectivePrepMode =
  learnedPrepMode !== "auto" ? learnedPrepMode : observedPrepMode;
const shouldSuppressNudge =
  learnedNudgeMode === "suppress" || shouldSuppressMeetingPrepNudge();
try {
  const { execFileSync } = await import("child_process");
  // AppleScript: collect events starting in the next 60 min across every
  // calendar. try/end try per calendar so one broken calendar doesn't abort.
  // execFileSync (not exec) — no shell interpolation, no injection surface.
  const appleScript = `tell application "Calendar"
    set nowDate to current date
    set soonDate to nowDate + 3600
    set upcoming to {}
    repeat with c in (every calendar)
      try
        set evs to (every event of c whose start date >= nowDate and start date < soonDate)
        repeat with e in evs
          set end of upcoming to (summary of e) & "|" & ((start date of e) as string)
        end repeat
      end try
    end repeat
    return upcoming
  end tell`;
  const raw = execFileSync("osascript", ["-e", appleScript], {
    encoding: "utf-8",
    timeout: 2000,
    stdio: ["ignore", "pipe", "ignore"], // swallow stderr
  }).trim();
  localCheckResolved = true; // osascript ran cleanly — trust its answer
  if (raw && raw.length > 0 && !raw.toLowerCase().startsWith("error")) {
    if (!shouldSuppressNudge) {
      calendarContext =
        effectivePrepMode === "prep"
          ? `\n\nUpcoming meeting in the next 60 min detected locally. Recommend /minutes-prep in ONE line — the user consistently uses the deeper prep flow.`
          : effectivePrepMode === "brief"
            ? `\n\nUpcoming meeting in the next 60 min detected locally. Recommend /minutes-brief in ONE line — the user consistently uses the fast brief flow.`
            : `\n\nUpcoming meeting in the next 60 min detected locally. Recommend /minutes-brief (fast, no questions) or /minutes-prep (interactive goal-setting) to the user in ONE line. Prefer brief if the meeting is <20 min away.`;
      recordPendingMeetingPrepNudge(effectivePrepMode);
    }
  }
  // Empty raw with localCheckResolved=true → no meetings → no injection. Zero cost.
} catch {
  // osascript unavailable (non-Mac), Calendar.app not running, permission
  // denied, or timeout. localCheckResolved stays false → fall through to MCP hint.
}

if (!localCheckResolved) {
  // We couldn't verify locally, so ask Claude to check via MCP if available.
  // This is the graceful fallback for non-Mac users and users without
  // Calendar.app running. One short sentence — minimal context cost.
  if (!shouldSuppressNudge) {
    calendarContext =
      effectivePrepMode === "prep"
        ? `\n\nIf gcal_list_events MCP is available and the user has a meeting in the next 60 min, recommend /minutes-prep in ONE line — the user consistently uses the deeper prep flow. Otherwise stay silent.`
        : effectivePrepMode === "brief"
          ? `\n\nIf gcal_list_events MCP is available and the user has a meeting in the next 60 min, recommend /minutes-brief in ONE line — the user consistently uses the fast brief flow. Otherwise stay silent.`
          : `\n\nIf gcal_list_events MCP is available and the user has a meeting in the next 60 min, recommend /minutes-brief (fast) or /minutes-prep (goal-setting) in ONE line. Otherwise stay silent.`;
    recordPendingMeetingPrepNudge(effectivePrepMode);
  }
}

const output = {
  additionalContext: `Active Minutes user.${calendarContext}${memoContext}${relationshipContext}${updateContext}`,
};

console.log(JSON.stringify(output));
