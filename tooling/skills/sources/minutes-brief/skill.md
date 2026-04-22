---
name: minutes-brief
description: Fast non-interactive briefing before any meeting — auto-detects your next calendar event, pulls relationship history, surfaces open commitments, and produces a one-page brief in under 30 seconds. Use this whenever the user says "brief me", "give me a quick brief", "what's coming up", "background on my next call", "who am I meeting next", "brief me on Sarah", "I have a call in 10 min", "quick rundown", or right before walking into a meeting. Different from /minutes-prep — brief is the fast hook-fireable version that doesn't ask questions and doesn't set goals. Use brief when speed matters; use prep when the user wants to think hard about goals first.
triggers:
  - brief me
  - give me a quick brief
  - what's coming up
  - background on my next call
  - who am I meeting next
  - brief me on Sarah
  - I have a call in 10 min
  - quick rundown
phase: lifecycle
user_invocable: true
metadata:
  display_name: Minutes Brief
  short_description: Fast meeting briefing from your conversation memory.
  default_prompt: Use Minutes Brief to prepare me quickly for an upcoming meeting using my stored meeting history.
  site_category: Lifecycle
  site_example: /minutes-brief
  site_best_for: Get a fast one-page brief before an upcoming meeting.
assets:
  scripts: []
  templates: []
  references: []
output:
  claude:
    path: .claude/plugins/minutes/skills/minutes-brief/SKILL.md
  codex:
    path: .agents/skills/minutes/minutes-brief/SKILL.md
tests:
  golden: true
  lint_commands: true
---

# /minutes-brief

Fast, non-interactive briefing that synthesizes your relationship history with someone into a one-page brief — designed so you can read it in 60 seconds before walking into a call.

This is the **proactive layer**. It's built to be invoked silently by a hook (e.g., 15 min before a calendar event) and to also work as a manual `/minutes-brief` command. Unlike `/minutes-prep`, brief asks no questions and sets no goals — it just hands you the facts.

## How it works

### Phase 0: Determine the target

Three ways the user can invoke this:

**1. With a name** (`/minutes-brief sarah`, "brief me on Alex")
→ Use that name directly. Before searching, check for learned aliases:

```bash
node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" aliases "<name>" 2>/dev/null
```

If aliases exist, search across all returned variants and treat them as the same person for the rest of the flow. If the user explicitly says two names are the same person, persist it:

```bash
node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" set-alias "Sarah Chen" "Sarah" "User confirmed these refer to the same person"
```

Then skip to Phase 1.

**2. With "auto" or no argument** (`/minutes-brief`, "brief me on my next call")
→ Auto-detect the next upcoming calendar event. Try sources in order — use the first that works:

- **Google Calendar MCP** (best — Claude users):
  ```
  gcal_list_events(timeMin: "<now ISO>", timeMax: "<+2hr ISO>", condenseEventDetails: false)
  ```
  Filter to events with 2+ attendees, skip all-day events, pick the soonest. Pull attendee names from the event.

- **`gog` CLI** (if installed):
  ```bash
  gog calendar list --today --json 2>/dev/null
  ```

- **Apple Calendar via osascript** (every Mac, zero install):
  ```bash
  osascript -e 'tell application "Calendar" to get {summary, start date} of (every event of every calendar whose start date >= (current date) and start date < ((current date) + 2 * hours))'
  ```

If none return anything, ask once: "I can't find an upcoming meeting. Who do you want a brief on?" Then take whatever they say and move on.

**3. Hook-fireable mode** (`/minutes-brief --auto`, or invoked silently from a hook)
→ Same as auto, but **never ask questions**. If no upcoming meeting and no name, exit silently with no output. Hooks should never spam the user.

### Phase 1: Gather data in parallel

The Minutes CLI already does the hard work. Fire all four of these as separate Bash tool calls in the same message — Claude Code will run them in parallel because they're independent. **The CLI's stream contract is messy; each command below has a specific shape documented below the block — read them before running.**

```bash
# 1. Person profile. stdout is CONTAMINATED: WARN tracing lines + JSON, mixed.
#    Use sed to extract from the first "{" to EOF before parsing as JSON.
minutes person "<name>" 2>/dev/null | sed -n '/^{/,$p'

# 2. Open commitments — what they owe you and what you owe them.
#    Clean JSON on stdout when --json is passed.
minutes commitments -p "<name>" --json 2>/dev/null

# 3. Recent decisions involving them, last 30 days.
#    Clean JSON on stdout by default (insights does NOT accept --json — output is already JSON).
minutes insights --participant "<name>" --kind decision --since <30-days-ago> 2>/dev/null

# 4. Recent meetings with them, last 60 days. Newline-delimited JSON (one object per line).
minutes search "<name>" --limit 10 --since <60-days-ago> --format json 2>/dev/null
```

**CLI stream-handling notes** — the Minutes CLI is actively developed and its stream contract is not fully settled. Today (0.8.0):

- `minutes person` writes tracing WARN lines **and** the JSON profile to **stdout** (not stderr, as you'd expect). The WARN lines appear first. The `sed -n '/^{/,$p'` pipe above strips them. If you pass `minutes person`'s raw stdout to a JSON parser, it will blow up on the first WARN line.
- `minutes person` also writes a human-readable summary ("Profile for Mat: …") to **stderr**. Harmless but weird. We redirect stderr to `/dev/null` to keep it out of the pipeline.
- `minutes commitments --json`, `minutes insights`, `minutes search --format json`, and `minutes people --json` all produce clean JSON on stdout when stderr is redirected. No extraction needed.
- **Do not invent new flags** on top of what's shown above — e.g. `minutes insights --json` is not a real flag, `minutes export --since` is not a real flag. The CLI will reject unknown flags with a usage error.

If a future CLI release changes any of these contracts, update this skill in the same PR that ships the CLI change.

**The `minutes person` empty-but-not-empty trap.** Some real meetings have malformed frontmatter that the CLI's strict schema rejects — you can see it in the WARN lines. When that happens, `minutes person "<name>"` returns an empty profile (`{"recent_meetings": [], ...}`) **even though meetings with that person actually exist on disk**. Before declaring "first meeting on record", always cross-check with `minutes search`:

```bash
minutes search "<name>" --limit 1 --format json 2>/dev/null
```

If `search` returns hits but `person` is empty, surface that as: "I have meetings with this person on record but the person profile is broken (likely a frontmatter schema issue). Here's what search found instead." Then read those meeting files directly and synthesize from the raw transcripts. Never lie to the user about a "first meeting" when it isn't one.

For multi-attendee meetings, focus on the **single most-mentioned attendee** (the one with the highest meeting count via `minutes person`). Mention the others briefly at the end. Don't try to brief 5 people at once — it dilutes the signal to nothing.

### Phase 2: Read what you actually need

Extract the file paths of the most recent 1–2 meetings from `minutes search`'s JSON output. Each line of the search output is a JSON object with a `path` field — the absolute path to the meeting file. Parse those, take the two most recent, and `Read` them in full.

Don't read more than two — past three meetings is enough context for a brief, and the brief is supposed to fit on one screen.

If `minutes person` returned a non-empty profile, you can also use its `recent_meetings` field for paths. Either source works; pick whichever is non-empty.

### Phase 3: Synthesize the brief

Produce a brief in this exact shape — every section is one tight chunk, total fits on one screen:

```markdown
# Brief: <Person Name> · <today's date>

**Last conversation** (<date of most recent meeting>): <2–3 sentences in prose, including the emotional tone if discernible from the transcript>

**They've been thinking about**: <comma-separated list of 3–5 hot topics from the last 30 days, ordered by recency × frequency>

**You owe them** (<count> open):
- <commitment 1> — <due date if known, mark ⚠ OVERDUE if past due>
- <commitment 2>

**They owe you** (<count> open):
- <commitment 1> — <due date if known>

**Where things stand**: <one-line read of the relationship vibe — warming, cooling, stable, urgent, drifting>

**Open with**: "<a concrete first sentence the user could literally say at the top of the call — references something specific from the last conversation>"
```

**Concrete example of a good "Open with" line:**

> Bad: "Hey Sarah, how's everything going with Q2?" — generic, could be said to anyone.
> Bad: "Hi Sarah, hope you're doing well!" — pure filler.
> **Good**: "Hey Sarah — last time you mentioned the Q2 hiring freeze was making your roadmap impossible. Did legal come back on the contractor question?"
>
> The good version is specific (cites Q2 freeze, the contractor angle), references something only THIS conversation history would surface, and gives Sarah an immediate hook to pick up the thread.

**Rules for the brief:**
- **No filler.** Cut anything that doesn't change what the user does in the next 60 minutes.
- **Be specific.** "They want to talk about Q2" is useless. "Sarah has raised the Q2 hiring freeze in 3 of her last 4 meetings" is useful.
- **Honest about gaps.** If `minutes person` AND `minutes search` both return nothing, this is genuinely a first meeting — say so explicitly. If `minutes search` returns hits but `minutes person` is empty, surface that mismatch (likely a frontmatter schema issue) and synthesize from the raw meeting files anyway. **Never claim "first meeting on record" without verifying with `minutes search`.**
- **The opening line matters most.** This is the punchline. Make it usable verbatim. The user should be able to literally say it.

### Phase 4: Save and display

Save the brief to `~/.minutes/briefs/` so `/minutes-debrief` can compare against it later:

```bash
mkdir -p ~/.minutes/briefs
chmod 700 ~/.minutes/briefs
```

Write to `~/.minutes/briefs/YYYY-MM-DD-{person-first-name-lowercase}.brief.md` with frontmatter:

```yaml
---
person: <Full Name>
date: <today ISO>
brief_type: auto | manual
meeting_count: <count from minutes person>
trigger: "calendar:<event title>" | "manual"
---
```

…followed by the brief body. Then `chmod 600` the file — briefs contain relationship intelligence and should be private.

Display the brief inline in the response. Don't paraphrase it or summarize it after — let the brief speak for itself.

### Phase 5: One-line nudge

End with **exactly one line** — this skill is about speed:

> "Want to think harder about goals? Run `/minutes-prep <name>`. After the call, run `/minutes-debrief`."

That's it. No follow-up questions, no "anything else I can help with?". Brief is fast on purpose.

## Gotchas

- **Record explicit workflow preferences when the user states them.** If the user says something like "default to prep", "always brief me first", or "stop reminding me about meeting prep", persist it:

```bash
node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" set-explicit workflow_preference meeting_prep_mode prep "User explicitly prefers prep"
node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" set-explicit workflow_preference meeting_prep_mode brief "User explicitly prefers brief"
node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" set-explicit nudge_feedback meeting_prep_nudge suppress "User explicitly asked to suppress meeting prep nudges"
```

- **Record explicit person aliases when the user confirms them.** If the user says "Sarah and Sarah Chen are the same person" or "Dan means Dan Benamoz here", persist it:

```bash
node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" set-alias "Sarah Chen" "Sarah" "User confirmed alias"
```

- **Hook-fireable mode is silent on failure.** When invoked with `--auto` and nothing matches, exit cleanly with zero output. Hooks should never spam the user. Errors only matter when the user is actively asking.
- **First-name slugs match the rest of the plugin.** Save files as `sarah.brief.md`, not `sarah-chen.brief.md`. Slug rules: lowercase, ASCII-only (strip diacritics via Unicode NFKD), replace spaces and punctuation with hyphens, take only the first name token. "María José Pérez" → `maria`. "Jean-Claude" → `jean-claude`. Single-name people (`Madonna`) → `madonna`. This matches `/minutes-prep` and `/minutes-debrief` so they can find each other.
- **If the user types two names, ask once.** "brief me on Sarah and Alex" — pick one as the focus and tell the user. "I'll focus the brief on Sarah since you have more history with her, and mention Alex briefly." Don't refuse, don't try to brief both fully.
- **Don't duplicate prep.** Brief sets no goals, asks no questions, and never produces talking points. If the user wants any of that, the closing nudge points them at `/minutes-prep`. Two skills, two jobs — keep the line clean.
- **One screen of output.** If the brief body is longer than ~25 lines, you've added filler. Cut it.
- **Multi-attendee meetings focus on one person.** Pick the single most-mentioned attendee (highest meeting count) and brief on them. Mention others in one sentence at the end: "Also in the room: Logan, Kim." Don't try to brief everyone — the signal dilutes to noise.
- **Zero history is not an error — but verify it before claiming it.** If both `minutes person` AND `minutes search` return empty, this is genuinely a first meeting. Output a brief that says exactly that, plus whatever the calendar event tells us. If `search` returns results but `person` is empty, this is the schema-error trap (Phase 1) — never call it a first meeting. Never invent history.
- **Calendar attendee names are messy.** The event might say "Sarah Chen <sarah@acme.com>" but transcripts say "Sarah" or "SPEAKER_1". Match on first name when looking up history; the CLI's `--participant` flag does fuzzy matching for you.
- **Briefs are sensitive.** Always `chmod 600`. They contain relationship intelligence the user wouldn't want leaked.
- **Be honest about staleness.** If the most recent meeting with this person is 3+ months old, lead with that fact: "**Last conversation** (4 months ago): …" — the user's mental model of the relationship may be more recent than reality.
- **Run the four CLI calls in parallel.** They're independent. Don't chain them sequentially — fire all four in one tool-call batch and synthesize from the combined results. Brief is supposed to be fast.
- **Don't apologize for missing data.** If `minutes commitments` returns empty, just say "No open commitments on either side." Move on. Apologizing for the absence of data wastes the user's most precious resource: the 60 seconds before the call.
