#!/usr/bin/env node

/**
 * SessionStart hook: proactive meeting reminder.
 *
 * When a Claude Code session starts, check if the user has a meeting
 * in the next 60 minutes. If so, nudge them to run /minutes-brief
 * (or /minutes-prep if they want to think harder about goals first).
 *
 * Also surfaces voice memos from the last 3 days and relationship-graph
 * intelligence (losing-touch alerts, stale commitments) so the agent
 * walks into the session already aware.
 *
 * Guards against being annoying:
 * - Only fires on startup (not resume/compact/clear)
 * - Only fires if the user has actively used a Minutes skill before
 *   (~/.minutes/preps/ OR ~/.minutes/briefs/ exists)
 * - Only fires during business hours (8am-6pm, weekdays)
 * - Can be disabled via ~/.config/minutes/config.toml: [reminders] enabled = false
 *
 * Hook event: SessionStart
 * Matcher: startup
 */

import { existsSync, readFileSync } from "fs";
import { join } from "path";
import { homedir } from "os";

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
    calendarContext = `\n\nUpcoming meeting in the next 60 min detected locally. Recommend /minutes-brief (fast, no questions) or /minutes-prep (interactive goal-setting) to the user in ONE line. Prefer brief if the meeting is <20 min away.`;
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
  calendarContext = `\n\nIf gcal_list_events MCP is available and the user has a meeting in the next 60 min, recommend /minutes-brief (fast) or /minutes-prep (goal-setting) in ONE line. Otherwise stay silent.`;
}

const output = {
  additionalContext: `Active Minutes user.${calendarContext}${memoContext}${relationshipContext}`,
};

console.log(JSON.stringify(output));
