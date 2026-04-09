---
name: minutes-weekly
description: Weekly meeting synthesis — themes, decision arcs, stale commitments, and what deserves your attention next week. Use when the user says "weekly review", "what happened this week", "weekly summary", "recap my week", "any outstanding items", "week in review", or at the end of a work week.
triggers:
  - weekly review
  - what happened this week
  - weekly summary
  - recap my week
  - any outstanding items
  - week in review
user_invocable: true
metadata:
  display_name: Minutes Weekly
  short_description: Weekly meeting synthesis — themes, decision arcs, stale commitments, and what deserves your attention next week.
  default_prompt: Use Minutes Weekly for this task.
assets:
  scripts: []
  templates: []
  references: []
output:
  claude:
    path: .claude/plugins/minutes/skills/minutes-weekly/SKILL.md
  codex:
    path: .agents/skills/minutes/minutes-weekly/SKILL.md
tests:
  golden: true
  lint_commands: true
---

# /minutes-weekly

Synthesize an entire week of meetings and voice memos into a forward-looking brief — themes, decision arcs, stale commitments, and what deserves attention Monday.

## How it works

This is a synthesis skill, not a command wrapper. It reads across all meetings, cross-references decisions and action items, and produces an intelligence brief.

### Phase 1: Gather this week's recordings

```bash
minutes list --limit 50
```

Filter to recordings from the last 7 days by checking the `date` field in each result's frontmatter. Do NOT use `minutes search` with a date string as the query — that searches file content, not dates.

For each recording from the past 7 days, read the full file with `Read` to get content.

**If zero recordings this week:**
Say: "No recordings found for the past 7 days. Nothing to synthesize."
Offer: "Want me to look at the past 2 weeks instead?"
Do NOT hallucinate a weekly summary.

**If only 1-2 recordings:**
Still produce the brief — it's shorter but still valuable. Note: "Light week — only [N] recordings."

### Phase 2: Theme extraction

Identify the 3-5 dominant themes across all meetings this week. A theme is a topic that appeared in 2+ meetings.

For each theme:
- Which meetings discussed it
- How the conversation evolved across meetings
- Whether a resolution was reached

Present as:

```
## This Week's Themes

### 1. Pricing (3 meetings)
- Mon: Case proposed $599 baseline
- Wed: Alex pushed back to annual billing
- Fri: Agreed on monthly billing experiment
- Status: RESOLVED (after 3 discussions)

### 2. Q2 Roadmap (2 meetings)
- Tue: Committed to April ship date
- Thu: Discussed pushing to May
- Status: CONFLICTING — not reconciled
```

### Phase 3: Decision evolution arcs

For every decision made this week, search for prior decisions on the same topic in the last 30 days:

```bash
minutes search "<topic>" --since <30-days-ago> --limit 20
```

Classify each decision:

- **STABLE** — Decision held across 2+ mentions → "Locked in"
- **VOLATILE** — Changed 2+ times in 14 days → "Still in flux"
- **CONFLICTING** — Two active contradictory decisions → "Needs reconciliation"
- **NEW** — First time this topic was decided → "Fresh"

Present as a table:

```
## Decision Arcs

| Decision | Status | Arc | Last Meeting |
|----------|--------|-----|-------------|
| Pricing: monthly billing experiment | VOLATILE | $599→annual billing→monthly billing | Fri w/ Alex |
| Hire senior eng by Q2 | STABLE | Set Mar 10, held | Tue w/ Case |
| Q2 ship date | CONFLICTING | April vs May | Thu w/ team |
```

If there are CONFLICTING decisions, flag them prominently:
"⚠️ You have conflicting decisions on Q2 ship date. Monday is a good time to reconcile."

### Phase 4: Action item audit

Scan all meetings from the past 30 days for action items:

```bash
minutes actions
```

Or grep across meeting frontmatter for `action_items` with `status: open`.

Categorize:
- **Completed this week** — items that were marked done
- **Still open, on track** — items with future due dates
- **Overdue** — items with past due dates, still open
- **Assigned to others** — items others owe the user

Flag overdue items prominently:
"⚠️ 3 action items are overdue. The oldest is from Mar 10 (pricing doc for Alex)."

### Phase 4b: Relationship intelligence (from conversation graph)

If the `minutes` CLI is available, pull relationship data:

```bash
minutes people --json --limit 20
minutes commitments --json
```

From the people data, produce:

**Relationship changes this week:**
- Who you met with most this week (compare to their usual frequency)
- Anyone new you met for the first time
- Anyone on your "losing touch" list

**Stale commitments by person:**
- Group overdue/stale commitments by person name
- Include the meeting they came from and the original due date
- Prioritize by age (oldest first)

Present as:

```
## Relationship Pulse

**Most active:** Sarah Chen (3 meetings this week, usually 1/week)
**New contact:** Jordan Mills (first meeting Thursday)
**Losing touch:** Alex Kumar (5 meetings total, last seen 3 weeks ago)

### Stale Commitments
- **Alex Kumar:** Send tech spec (due Mar 20, from Q2 Planning)
- **mat:** Pull March revenue numbers (due Mar 22, from Investor Update Prep)
```

If no graph data is available (minutes people returns empty), skip this phase silently. It's additive, not required.

### Phase 5: Unresolved preps

Scan `~/.minutes/preps/` for prep files from this week that were never followed by a debrief:

```bash
ls ~/.minutes/preps/ 2>/dev/null
```

For each prep file from the last 7 days: check if a meeting with that person occurred after the prep date. If the user prepped but never debriefed:

"You prepped for a call with Alex on Tuesday but I don't see a debrief. Did the call happen?"

This catches meetings that happened but weren't recorded, or recordings that weren't processed.

### Phase 6: Forward brief

Produce a "what deserves your attention Monday" section:

Before deciding how to order the weekly output, check:

```bash
node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" get-presentation-focus weekly
```

If the result is:
- `decisions-first` → lead with Decision Arcs and unresolved conflicts before commitments
- `commitments-first` → lead with Action Item Audit / stale commitments before decision arcs
- `memo-heavy` → surface voice memos and idea capture much more prominently in the synthesis

If there is no preference, keep the default order in this skill.

```
## Attention Monday

1. **Reconcile Q2 ship date** — April vs May is unresolved.
   Last discussed Thu with the team.

2. **Send pricing doc to Alex** — Overdue since Friday.
   She's expecting it. This is blocking.

3. **Follow up with Case on competitor grid** — He committed Mar 17.
   No update yet. Worth a ping.
```

Prioritize by: CONFLICTING decisions > overdue action items > open commitments > unresolved preps.

### Phase 7: Closing ritual

End with three beats:

1. **Signal reflection** — Reference a pattern or insight from the week.
   "Pricing dominated this week — 3 meetings, 3 changes, one resolution. That's behind you now."

2. **Assignment** — The single most important thing to do Monday.
   "First thing Monday: send the pricing doc to Alex. It's overdue and she's waiting."

3. **Next skill nudge** — "For your most important meeting Monday, run `/minutes-prep` to go in prepared."

## Gotchas

- **Record explicit weekly presentation preferences when the user states them.** If the user says "always show commitments first", "start with decisions", or "surface voice memos more", persist it:
  ```bash
  node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" set-presentation-focus weekly commitments-first "User explicitly prefers commitments first in weekly synthesis"
  node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" set-presentation-focus weekly decisions-first "User explicitly prefers decisions first in weekly synthesis"
  node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" set-presentation-focus weekly memo-heavy "User explicitly wants stronger voice-memo emphasis"
  ```
- **Zero recordings is not an error** — Say "nothing this week" clearly. Don't hallucinate a summary. Offer to extend the range.
- **Light weeks (1-2 recordings) are still worth summarizing** — A single meeting can have important decisions. Don't dismiss it.
- **Don't be a nag about overdue items** — Surface them factually. "This is overdue since Friday" not "You really need to get on this."
- **Decision evolution spans beyond this week** — Search the last 30 days for related decisions, not just this week. A decision made today might conflict with one from 3 weeks ago.
- **Preps without recordings might be intentional** — Maybe the meeting was cancelled. Ask, don't assume.
- **Voice memos count** — Include `~/meetings/memos/` in the weekly scan. Ideas captured on the go are easy to forget.
- **End-of-week timing** — The user might run this Thursday or Monday morning. "This week" should be the most recent 7 days, regardless of day-of-week.
