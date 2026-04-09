---
name: minutes-prep
description: Interactive meeting preparation — builds a relationship brief and talking points before a call. Use when the user says "prep me for my call with", "I'm meeting with X", "prepare me for", "what should I bring up with", "meeting prep", "get ready for my call", or wants to review history with someone before a meeting.
triggers:
  - prep me for my call with
  - I'm meeting with
  - prepare me for
  - what should I bring up with
  - meeting prep
  - get ready for my call
phase: lifecycle
user_invocable: true
metadata:
  display_name: Minutes Prep
  short_description: Interactive meeting prep from your conversation history.
  default_prompt: Use Minutes Prep to help me prepare for an upcoming meeting using relationship history, open commitments, and prior decisions.
assets:
  scripts: []
  templates: []
  references: []
output:
  claude:
    path: .claude/plugins/minutes/skills/minutes-prep/SKILL.md
  codex:
    path: .agents/skills/minutes/minutes-prep/SKILL.md
tests:
  golden: true
  lint_commands: true
---

# /minutes-prep

Interactive meeting preparation that searches your entire conversation history with someone, synthesizes a relationship brief, and produces talking points — before you walk into the room.

## How it works

This is a multi-phase interactive flow, not a single command. Walk the user through each phase using AskUserQuestion, pushing back on vague answers.

### Phase 0: Calendar auto-detect (optional, runs first)

Before asking who the user is meeting with, check if upcoming meetings are available from any calendar source. Try these in order — use the first that works:

**1. Google Calendar MCP** (best — most Minutes users have Claude + MCP):
If `mcp__claude_ai_Google_Calendar__gcal_list_events` is available, query today's remaining events:
```
gcal_list_events(
  timeMin: "<now ISO, e.g. 2026-03-19T14:00:00>",
  timeMax: "<end of day, e.g. 2026-03-19T23:59:59>",
  condenseEventDetails: false
)
```
Do NOT hardcode a timezone — omit the `timeZone` parameter so the MCP uses the user's calendar default. This returns attendees, event titles, and times. Parse the results to find the next upcoming meeting with other people (skip all-day events and events with no attendees).

**2. `gog` CLI** (if installed):
```bash
gog calendar list --today --json -a <account> 2>/dev/null
```

**3. Apple Calendar (osascript)** (every Mac, zero install):
```bash
osascript -e 'tell application "Calendar" to get {summary, start date} of (every event of every calendar whose start date >= (current date) and start date < ((current date) + 1 * days))'
```

**4. None available** — skip to Phase 1 and ask manually.

**If upcoming meetings are found:**
Present via AskUserQuestion: "I see you have these meetings coming up today:
- [time] — [title] with [attendees]
- [time] — [title] with [attendees]

Which one are you prepping for?"

Options: list each meeting + "None of these — I want to prep for something else"

If the user picks a meeting, auto-populate the person name and skip Phase 1. Pull attendee names from the calendar event directly.

**If no upcoming meetings or calendar unavailable:**
Silently skip to Phase 1. Don't error or apologize — just ask manually.

### Phase 1: Who are you meeting with?

If Phase 0 already identified the person, skip this phase.

Otherwise, ask via AskUserQuestion: "Who are you meeting with?"

**If the answer is specific** (a name like "Alex" or "Case"):
→ Search all past meetings:
```bash
minutes search "<name>" --limit 50
```
Also search common variations — first name, last name, nickname.

**If the answer is vague** ("the team", "everyone", "my usual meeting"):
→ Push back: "Be specific. Name one person who'll be in the room. I'll search everything you've discussed with them."

**If the answer is a topic** ("the pricing meeting", "the Q2 planning call"):
→ Adapt to topic-based prep:
```bash
minutes search "<topic>" --limit 20
```
Skip the relationship brief and go straight to a topic brief instead.

### Phase 2: Relationship brief

Read each matching meeting file with `Read`. Build a relationship brief:

**Meeting history:**
- Total meetings with this person, first and most recent dates
- Meeting frequency trend (increasing/decreasing/stable)

**Recurring topics:**
- Topics that come up across multiple meetings
- Topic trending: which topics are appearing MORE recently (↑) vs fading (↓)
- Flag any topic that appeared 3+ times in the last 2 weeks — "Something's on their mind"

**Open commitments:**
- Items YOU owe THEM (from `action_items` where assignee matches the user)
- Items THEY owe YOU (from `action_items` where assignee matches the other person)
- Flag any overdue items (due date in the past, status still `open`)

**Decision history:**
- Recent decisions involving this person (from `decisions:` frontmatter)
- Any volatile decisions (changed 2+ times) — flag as "still in flux"

Present the relationship brief to the user. Don't ask for approval — just show it and move to Phase 3.

**If there are zero past meetings:**
Say: "I don't have any recorded meetings with [name]. This will be your first meeting on record. What's the context — how do you know them?"

Then skip the relationship brief and go straight to Phase 3 with whatever context the user provides.

### Phase 3: What do you want to accomplish?

Ask via AskUserQuestion: "What's the one thing you'd regret not discussing in this meeting?"

**If the answer is specific** ("finalize the pricing at monthly billing", "get a commitment on the hire"):
→ Frame talking points around that goal. Connect it to relationship data — e.g., "Alex's mentioned pricing 3 times recently. She's ready for this conversation."

**If the answer is vague** ("just catch up", "the usual"):
→ Push back with evidence: "Based on your last 3 meetings with Alex, these topics are active: [list]. Which one matters most today?"

**If the user skips** ("I don't know yet" / "nothing specific"):
→ Accept it. Frame as an open-ended catch-up. Still produce talking points based on open items and recent topics.

### Phase 4: Save prep file

Save the prep brief to `~/.minutes/preps/` for later pickup by `/minutes-debrief`:

```bash
mkdir -p ~/.minutes/preps
```

Write the file as `~/.minutes/preps/YYYY-MM-DD-{person-first-name}.prep.md` with this structure:

```markdown
---
person: {full name}
date: {today ISO}
goal: {what they want to accomplish}
meeting_count: {total past meetings}
---

## Relationship Brief
{the brief from Phase 2}

## Talking Points
{the talking points}

## Goal
{their stated goal, quoted}
```

Set permissions to 0600:
```bash
chmod 600 ~/.minutes/preps/YYYY-MM-DD-{slug}.prep.md
```

Use the person's first name (lowercase) as the slug — e.g., `sarah`, not `sarah-chen` — because transcript attendee names are often abbreviated.

### Phase 5: Closing ritual

End with three beats:

1. **Signal reflection** — Quote a specific thing the user said during the session.
   "You said '[exact quote from their AskUserQuestion answers]' — that's your north star for this call."

2. **Assignment** — One concrete real-world action before the meeting. Not "go build it" — something specific.
   Examples: "Text Alex before your call that you want to finalize pricing."
   "Review the competitor grid Case sent you — it's still in your action items."

3. **Next skill nudge** — "After your call, run `/minutes-debrief` to capture what you decided and compare it to what you planned."

## Gotchas

- **Calendar auto-detect is best-effort** — If no calendar source is available, silently fall back to asking manually. Never error or nag the user about calendar setup. The Google Calendar MCP (`gcal.mcp.claude.com/mcp`) is the recommended source for Claude users.
- **Calendar attendee names may differ from meeting transcript names** — Calendar says "Alex Chen (sarah@company.com)" but transcripts say "Alex" or "SPEAKER_0". Match on first name.
- **Skip all-day events and solo events** — Only show events with 2+ attendees as prep candidates. All-day events and events where the user is the only attendee aren't meetings.
- **Push back on vague answers** — This is the most important pattern. Vague prep = useless prep. "Everyone" → "Name one person." "Catch up" → "What would you regret not discussing?"
- **First-name slug for prep files** — Use `sarah` not `sarah-chen`. Transcript attendees are often abbreviated. Debrief matches on first name.
- **Zero past meetings is not an error** — It's a first meeting. Adjust the flow, don't apologize.
- **Don't hallucinate meeting history** — If you searched and found nothing, say so. Never invent meetings or conversations that don't appear in the search results.
- **Prep files are sensitive** — They contain relationship intelligence. Always 0600 permissions.
- **Multiple people in the meeting** — If the user names 2-3 people, search for each and combine the briefs. For >3, suggest picking the most important person to focus on.
- **Stale prep files** — `/minutes-debrief` ignores preps older than 48 hours. The user can prep the day before and still get the debrief connection.
