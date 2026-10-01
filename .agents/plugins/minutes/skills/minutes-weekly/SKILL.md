---
name: minutes-weekly
description: Weekly meeting synthesis — themes, decision arcs, stale commitments, and what deserves your attention next week. Use when the user says "weekly review", "what happened this week", "weekly summary", "recap my week", "any outstanding items", "week in review", or at the end of a work week.
---

## Local Minutes Host

This plugin runs in ChatGPT Work or Codex with access to the local computer.
Use the connected Minutes MCP tools when they cover the requested operation;
resolve their exact registered names from the host's tool list. CLI commands
and bundled helpers require a local shell on the same computer as Minutes.
If that runtime is unavailable, report the missing capability instead of
fabricating results or treating a command as executed. Keep capture, audio
processing, dictation insertion, and OS permissions in the local Minutes engine.
The plugin does not capture audio in ChatGPT's browser or upload a library.
Tool results used as model context are shared with the AI host. Preserve the
canonical skill's meeting-access, confirmation, and external-send rules.


## Skill Path

Resolve this skill's installed SKILL.md path from the host's skill metadata.
Set MINUTES_SKILL_ROOT to that file's absolute parent directory and
MINUTES_SKILLS_ROOT to the absolute parent of MINUTES_SKILL_ROOT. Do this before
running a helper. Do not infer either path from the current working directory
or a Git checkout. The bundled runtime is under MINUTES_SKILLS_ROOT/_runtime.

# /minutes-weekly

Synthesize a week of policy-authorized normal meetings and voice memos into a
forward-looking brief — themes, decision arcs, stale commitments, and what
deserves attention Monday. Restricted history is excluded by default.

## How it works

This is a synthesis skill, not a command wrapper. It reads across the normal
meetings authorized by Minutes, cross-references decisions and action items,
and produces an intelligence brief. Restricted meetings are excluded from this
agent workflow by default and must never be treated as absent facts.

### Phase 1: Gather this week's recordings

```bash
minutes list --limit 50
```

Filter to recordings from the last 7 days by checking the `date` field in each result's frontmatter. Do NOT use `minutes search` with a date string as the query — that searches file content, not dates.

For each selected recording, run `minutes get "<exact path>" --json`, require
exit status 0, and use only the returned content. List/search paths are hints,
not retained capabilities; never reopen them with the host `Read` tool.

**If zero recordings this week:**
Say: "No recordings found for the past 7 days. Nothing to synthesize."
Offer: "Want me to look at the past 2 weeks instead?"
Do NOT hallucinate a weekly summary.

**If only 1-2 recordings:**
Still produce the brief — it's shorter but still valuable. Note: "Light week — only [N] recordings."

### Phase 2: Theme extraction

Identify the 3-5 dominant themes across the authorized normal meetings this week. A theme is a topic that appeared in 2+ meetings.

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

For every decision found in the authorized normal sources this week, search for prior normal-source decisions on the same topic in the last 30 days:

```bash
minutes search "<topic>" --since <30-days-ago> --limit 20
```

Require search exit status 0. Reauthorize any result used in the decision arc
with `minutes get "<exact path>" --json`; never read a result path directly.

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

Scan authorized normal meetings from the past 30 days for action items:

```bash
minutes actions
```

Require exit status 0. If the native action surface fails, report the source
unavailable; never grep meeting files as a fallback.

Categorize:
- **Completed this week** — items that were marked done
- **Still open, on track** — items with future due dates
- **Overdue** — items with past due dates, still open
- **Assigned to others** — items others owe the user

Flag overdue items prominently:
"⚠️ 3 action items are overdue. The oldest is from Mar 10 (pricing doc for Alex)."

### Phase 4b: Relationship intelligence

Run `minutes people --json` and `minutes commitments --json` for bounded relationship intelligence. Require exit status 0 from each; a projection failure means unavailable, never “no relationships” or “no commitments.” Continue using source-backed actions and consistency results from the surrounding phases.

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
node "$MINUTES_SKILLS_ROOT/_runtime/hooks/lib/minutes-learn-cli.mjs" get-presentation-focus weekly
```

If the result is:
- `decisions-first` → lead with Decision Arcs and unresolved conflicts before commitments
- `commitments-first` → lead with Action Item Audit / stale commitments before decision arcs
- `memo-heavy` → surface voice memos and idea capture much more prominently in the synthesis

Use this concrete ordering:

- `decisions-first`
  1. Decision Arcs
  2. This Week's Themes
  3. Action Item Audit / stale commitments
  4. Attention Monday

- `commitments-first`
  1. Action Item Audit / stale commitments
  2. Decision Arcs
  3. This Week's Themes
  4. Attention Monday

- `memo-heavy`
  1. This Week's Themes, but ensure voice memos and idea capture are surfaced explicitly inside the theme summary
  2. Action Item Audit
  3. Decision Arcs
  4. Attention Monday

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
  node "$MINUTES_SKILLS_ROOT/_runtime/hooks/lib/minutes-learn-cli.mjs" set-presentation-focus weekly commitments-first "User explicitly prefers commitments first in weekly synthesis"
  node "$MINUTES_SKILLS_ROOT/_runtime/hooks/lib/minutes-learn-cli.mjs" set-presentation-focus weekly decisions-first "User explicitly prefers decisions first in weekly synthesis"
  node "$MINUTES_SKILLS_ROOT/_runtime/hooks/lib/minutes-learn-cli.mjs" set-presentation-focus weekly memo-heavy "User explicitly wants stronger voice-memo emphasis"
  ```
- **Zero recordings is not an error** — Say "nothing this week" clearly. Don't hallucinate a summary. Offer to extend the range.
- **Light weeks (1-2 recordings) are still worth summarizing** — A single meeting can have important decisions. Don't dismiss it.
- **Don't be a nag about overdue items** — Surface them factually. "This is overdue since Friday" not "You really need to get on this."
- **Decision evolution spans beyond this week** — Search the last 30 days for related decisions, not just this week. A decision made today might conflict with one from 3 weeks ago.
- **Preps without recordings might be intentional** — Maybe the meeting was cancelled. Ask, don't assume.
- **Voice memos count** — Include `~/meetings/memos/` in the weekly scan. Ideas captured on the go are easy to forget.
- **End-of-week timing** — The user might run this Thursday or Monday morning. "This week" should be the most recent 7 days, regardless of day-of-week.
