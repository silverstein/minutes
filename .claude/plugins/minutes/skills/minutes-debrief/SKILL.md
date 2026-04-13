---
name: minutes-debrief
description: Post-meeting debrief — analyzes what happened, compares outcomes to your prep intentions, tracks decision evolution. Use when the user says "debrief", "what just happened in that meeting", "what did we decide", "debrief that call", "post-meeting", "what changed", or right after stopping a recording.
user_invocable: true
---

# /minutes-debrief

Post-meeting analysis that reads your latest recording, compares what happened to what you planned, and surfaces decision evolution — so nothing falls through the cracks.

## How it works

This is a multi-phase interactive flow. It connects to `/minutes-prep` when a prep file exists, creating a before→after loop.

### Phase 1: Find the most recent recording

```bash
minutes list --limit 5
```

Pick the most recent recording. If there are multiple from today, ask via AskUserQuestion: "You have [N] recordings today. Which one are you debriefing?" with options listing the titles.

**If no recent recording exists:**
Say: "I don't see any recent recordings. Did you run `minutes record` and `minutes stop`? If the recording is from a specific meeting, tell me the title or date and I'll find it."

Don't proceed without a recording to debrief.

### Phase 2: Read the transcript

Use `Read` on the meeting file path. Extract from the transcript and frontmatter:

- **Decisions made** (from `decisions:` frontmatter or `## Decisions` section)
- **Action items created** (from `action_items:` frontmatter or `## Action Items` section)
- **Key discussion points** (from `## Summary` or the transcript itself)
- **Attendees** (from `attendees:` frontmatter)

### Phase 2b: Check speaker attributions

If the meeting has a `speaker_map:` field in frontmatter, check the confidence levels:

- **All High confidence**: Speakers are confirmed — use real names throughout the debrief.
- **Any Medium confidence**: Note this — "Speakers were auto-identified (medium confidence). If the names look wrong, run: `minutes confirm --meeting <path>`"
- **No speaker_map but has SPEAKER_X labels**: The meeting has diarization but no attribution — suggest: "I see anonymous speaker labels. If you know who was in this meeting, run `minutes confirm --meeting <path>` to tag them."

This nudge is brief (one line) — don't make it a blocker.

### Phase 3: Check for matching prep

Look for a prep file that matches this meeting:

```bash
ls ~/.minutes/preps/ 2>/dev/null
```

Match logic:
1. Find `.prep.md` files from today or yesterday (within 48 hours)
2. Read each file's `person:` frontmatter field
3. Compare against the recording's `attendees:` list — match on first name, but check learned aliases before deciding there is no match:
   ```bash
   node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" aliases "<attendee-or-person>" 2>/dev/null
   ```
   Treat all returned variants as equivalent during prep-file matching.
4. If multiple preps match → AskUserQuestion to pick which one
5. If no prep matches → standalone debrief (skip to Phase 4b)

**Phase 4a: Prep-connected debrief** (when a matching prep exists)

Read the prep file. Pull out the `goal:` field. Ask via AskUserQuestion:

"You went into this meeting wanting to: **[goal from prep]**

Did you accomplish it?"

Options:
- **A) Yes — fully resolved** → Mark as complete. Summarize what was decided.
- **B) Partially — some progress** → Ask: "What's still open?" Capture the remaining items.
- **C) No — it didn't come up or it changed** → Ask: "What happened instead?" Capture the pivot.
- **D) The goal changed during the meeting** → Ask: "What's the new direction?"

Then produce the debrief summary with the prep comparison:

Before writing the output, check for a learned debrief presentation preference:

```bash
node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" get-presentation-focus debrief
```

If the result is:
- `decisions-first` → put Decisions before Action Items and Relationship Update
- `actions-first` → put Action Items first, then Decisions, then Relationship Update
- `relationship-first` → put Relationship Update first, then Decisions, then Action Items

If there is no preference, keep the default order below.

```
## Debrief: [Meeting Title]

### Prep vs Reality
- **Goal:** [from prep]
- **Outcome:** [resolved / partially / pivoted]
- **What changed:** [if anything]

### Decisions
- [list each decision]

### Action Items
- [list with assignee and due date]

### Relationship Update
- [any notable changes in tone, new topics, shifted priorities]
```

**Phase 4b: Standalone debrief** (no matching prep)

Produce a straightforward debrief:

Before deciding the section order, check:

```bash
node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" get-presentation-focus debrief
```

Apply the same ordering rules if a preference exists; otherwise keep the default order below.

```
## Debrief: [Meeting Title]

### Key Decisions
- [list each decision]

### Action Items
- [list with assignee and due date]

### Notable Discussion Points
- [2-3 most significant things discussed]
```

### Phase 5: Decision evolution check

Search for prior decisions on the same topics discussed in this meeting:

```bash
minutes search "<topic>" --limit 10 --since <30-days-ago>
```

For each topic that has a decision in this meeting AND a decision in a prior meeting:
- Compare the decisions
- If they differ → surface the evolution:

"**Decision evolution — pricing:**
- Mar 3 (with Case): $599
- Mar 10 (with Alex): annual billing
- Today: monthly billing
- Status: **VOLATILE** (3 changes in 14 days)

Is this settled now, or still in flux?"

Classification:
- **STABLE** — Same decision held across 2+ meetings
- **VOLATILE** — Decision changed 2+ times in 14 days
- **CONFLICTING** — Two different active decisions exist on the same topic
- **NEW** — First decision on this topic

### Phase 6: Closing ritual

End with three beats:

1. **Signal reflection** — Quote something specific from the meeting or the debrief conversation.
   "You said '[quote]' — that sounds like the decision is locked."

2. **Assignment** — One concrete follow-up action.
   "Send Alex the pricing doc tonight while the conversation is fresh."
   "Update the roadmap doc with today's Q2 timeline change."

3. **Next skill nudge** — "At the end of the week, run `/minutes-weekly` to see how all your meetings connect and what still needs attention."

## Gotchas

- **Record explicit presentation preferences when the user states them.** If the user says "show action items first", "lead with decisions", or "start with the relationship read", persist it:
  ```bash
  node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" set-presentation-focus debrief actions-first "User explicitly prefers action items first"
  node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" set-presentation-focus debrief decisions-first "User explicitly prefers decisions first"
  node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" set-presentation-focus debrief relationship-first "User explicitly prefers relationship updates first"
  ```
- **Don't hallucinate if there's no recording** — If `minutes list` returns nothing, say so. Don't invent a debrief.
- **Stale preps (>48h) are ignored** — If the prep file is more than 48 hours old, treat it as no-prep mode. The prep was for a different context.
- **First-name matching for prep files** — The prep file slug uses first name only (`sarah.prep.md`). Match against attendee first names in the recording frontmatter. "Alex C." matches "sarah".
- **Teach Minutes aliases when the user corrects matching.** If the user says "That prep was for Sarah Chen, not just Sarah" or clarifies that two names refer to the same person, persist it:
  ```bash
  node "${CLAUDE_PLUGIN_ROOT}/hooks/lib/minutes-learn-cli.mjs" set-alias "Sarah Chen" "Sarah" "User corrected prep/debrief matching"
  ```
- **Multiple recordings today** — Ask which one. Don't assume the most recent is the right one.
- **Recordings without frontmatter** — Some recordings only have raw transcripts (no summary, no decisions section). Work with what you have — extract decisions and action items from the transcript text yourself.
- **Decision evolution can span weeks** — Search the last 30 days for related decisions, not just this week.
- **Don't be preachy about decision changes** — Decisions change for good reasons. Surface the evolution factually. "Here's what shifted" not "You keep changing your mind."

