---
name: minutes-tag
description: Lightweight outcome tagging for meetings — won, lost, stalled, great, or noise. Use whenever the user says "tag this meeting", "mark that as a win", "that one was a loss", "tag yesterday's call as stalled", "mark this great", "that meeting was noise", "label that meeting", or any time they describe a meeting outcome in passing. Tagging takes 5 seconds and unlocks /minutes-mirror correlation analysis — the more meetings get tagged, the smarter mirror gets at telling the user what behavior patterns lead to wins. Surface this skill any time the user mentions a meeting result, win, loss, or wasted time.
triggers:
  - tag this meeting
  - mark that as a win
  - that one was a loss
  - tag yesterday's call as stalled
  - mark this great
  - that meeting was noise
  - label that meeting
user_invocable: true
metadata:
  display_name: Minutes Tag
  short_description: Lightweight outcome tagging for meetings — won, lost, stalled, great, or noise. Use whenever the user says "tag this meeting", "mark that as a win", "that one was a loss", "tag yesterday's call as stalled", "mark this great", "that meeting was noise", "label that meeting", or any time they describe a meeting outcome in passing. Tagging takes 5 seconds and unlocks /minutes-mirror correlation analysis — the more meetings get tagged, the smarter mirror gets at telling the user what behavior patterns lead to wins. Surface this skill any time the user mentions a meeting result, win, loss, or wasted time.
  default_prompt: Use Minutes Tag for this task.
assets:
  scripts:
    - scripts/tag_apply.py
  templates: []
  references: []
output:
  claude:
    path: .claude/plugins/minutes/skills/minutes-tag/SKILL.md
  codex:
    path: .agents/skills/minutes/minutes-tag/SKILL.md
tests:
  golden: true
  lint_commands: true
---

# /minutes-tag

Lightweight outcome tagging — adds an `outcome:` field to a meeting's frontmatter so `/minutes-mirror` can correlate the user's behavior with their results over time.

The whole point of this skill is **speed**. Tagging should take 5 seconds, not 5 questions. Don't be precious about it — most users will never adopt tagging if it feels like data entry.

## How it works

### Phase 1: Identify the meeting

Three patterns the user might use. **Always filter to meetings, not voice memos** — voice memos can't be "won" or "lost".

**1. Most recent** ("tag this meeting", "mark that as a win", "tag the call I just finished"):
```bash
minutes list --content-type meeting --limit 1
```
Use the most recent. **Don't ask which one** — that defeats the speed promise. The default behavior should always be "the call you just had".

**2. By date** ("tag yesterday's call", "tag the Tuesday call"):
```bash
minutes list --content-type meeting --limit 10
```
Pick the meeting matching the date. If multiple meetings match the same day, ask once: "You had <N> meetings <date>. Which one?" with options listing titles.

**3. By name** ("tag my call with Sarah as a win"):
```bash
minutes search "<name>" --content-type meeting --limit 5
```
Pick the most recent. If ambiguous, ask once.

### Phase 2: Identify the tag

If the user already named the outcome in their message ("tag that as a win"), use it directly. Don't ask again — they already told you.

If they haven't, ask via AskUserQuestion with these standard options:

- **won** — got the outcome you wanted (deal closed, decision made, agreement reached)
- **lost** — didn't get what you wanted (deal lost, idea rejected, no decision)
- **stalled** — neither — went sideways, no clear outcome, needs another meeting
- **great** — high-quality conversation regardless of outcome (insight, real connection, energy, learned something)
- **noise** — should have been an email; no value; time wasted
- **(custom)** — let the user provide their own tag

Standard tags are the only ones `/minutes-mirror` will correlate. Custom tags are stored faithfully but won't appear in correlation analysis — warn the user gently if they pick a custom one: "Custom tags are saved, but mirror only correlates the standard five."

### Phase 3: Capture a note **only if the user gave one in their message**

**Do not ask an interactive note question.** That's a second prompt and it breaks the speed promise.

Parse the user's original message for a "why" or note. Common patterns:
- "tag as won, **note: Sarah committed to monthly billing**" → note = "Sarah committed to monthly billing"
- "tag won — **got the verbal commit on pricing**" → note = "got the verbal commit on pricing"
- "tag stalled **because Alex postponed the decision**" → note = "Alex postponed the decision"

If you find a note in the message, use it. If you don't, **leave `outcome_note` out of the frontmatter entirely**. Don't insert an empty field. Don't ask. Users who want a fuller record have `/minutes-debrief` for that.

### Phase 4: Edit the frontmatter via the bundled helper script

**Use the script — do not Edit the frontmatter manually.** YAML frontmatter is fragile, and the script handles all the edge cases (no existing frontmatter, existing outcome that needs replacement, atomic write to prevent half-edits, preservation of all other fields).

```bash
python3 "${CLAUDE_PLUGIN_ROOT}/skills/minutes-tag/scripts/tag_apply.py" \
  "<absolute-path-to-meeting-file>" \
  --outcome <won|lost|stalled|great|noise|custom> \
  [--note "the optional one-line note from Phase 3"]
```

Pass `--note` only if Phase 3 found a note in the user's message. Skip the flag entirely otherwise — the script will omit `outcome_note` from the frontmatter rather than inserting an empty field.

**What the script guarantees:**

- The new fields (`outcome`, `outcome_note` if a note was passed, `tagged_at`) are inserted just before the closing `---` of the frontmatter, after every other existing field.
- All other frontmatter fields are preserved **byte-for-byte** — no reordering, no reformatting, no whitespace changes.
- Re-tagging is fully idempotent: if `outcome:` already exists, the script removes the old outcome lines and re-inserts fresh ones at the end. Old `outcome_note:` is dropped if no new note is passed.
- The body of the meeting file is never touched — only the frontmatter block.
- Writes are atomic (temp file + rename) so an interrupted run can never leave a half-written meeting.

The script prints `{"status": "ok", ...}` to stdout on success, or `{"error": "..."}` to stderr with non-zero exit on failure. Surface any error to the user.

**Fallback if Python isn't available** (extremely rare on macOS): use the `Edit` tool with surgical precision. Find the closing `---` of the frontmatter, anchor on a small unique block ending in it, and insert your new fields right before. This is brittle on unusual frontmatter — only do it if the script fails.

### Phase 5: Confirm and nudge

Confirm in **one line**: "Tagged **<meeting title>** as **<outcome>**."

Then verify the file is still parseable by Minutes after the edit. The slug is the filename minus `.md` (e.g., `2026-03-18-product-roadmap-with-case`):

```bash
minutes get "<filename-without-.md>" 2>&1 | head -3
```

If the output contains an error or warning about malformed frontmatter, surface it gently: "Note: this meeting's frontmatter has a pre-existing schema issue. The tag was saved, but `/minutes-mirror` may skip this meeting until it's fixed." Don't try to fix the unrelated schema issue — that's not tag's job.

**One-time lifetime nudge** (idempotent — never repeats):
```bash
ls ~/.minutes/tag-nudge-shown 2>/dev/null
```

If that marker file doesn't exist, this is the first time tag has run on this machine. Show the nudge once, then create the marker:

> "First tag — nice. When you've tagged ~10 meetings, run `/minutes-mirror trends` and I'll show you what your winning meetings have in common."

```bash
mkdir -p ~/.minutes && touch ~/.minutes/tag-nudge-shown
```

The marker file is the state. No counting, no edge cases, no risk of repeated nudges from re-tagging the same meeting.

## Gotchas

- **Speed is the entire feature.** If tagging takes more than two questions (the tag, optionally the note), you've broken it. Default to "most recent". Skip the optional note unless the user clearly wants to add one.
- **Standard tags only correlate.** Mirror's correlation analysis only works on the five standard tags: `won`, `lost`, `stalled`, `great`, `noise`. Custom tags are saved but won't be analyzed. Warn the user once if they pick a custom tag — don't lecture them, just let them know.
- **Don't touch the meeting body.** Only edit the YAML frontmatter block between the first two `---` markers. Use `Edit` with surgical precision.
- **Re-tagging is intentional.** If the user tags a meeting that's already tagged, overwrite it cleanly. They're either correcting themselves or seeing it differently after the fact. Both are valid.
- **Preserve existing frontmatter exactly.** Some meetings have `action_items`, `decisions`, `intents`, `entities`, `people`, `calendar_event`, `captured_at`, `device`, `recorded_by`, etc. Don't reformat or reorder anything — only insert/update the three outcome fields.
- **Tag freshness matters.** Tags are most valuable within ~24 hours, while the outcome is fresh in the user's head. Tagging two weeks later is fine but worth less. Don't enforce this — just don't make tagging feel like a chore that the user puts off.
- **Don't try to infer the tag from the transcript.** If the user says "tag this meeting" without saying which outcome, ask. Don't guess from the transcript — your guess will be wrong in the cases that matter most (a meeting that looks like a win on paper but actually wasn't, or vice versa).
- **The note is optional for a reason.** Most users will skip it. That's fine — the tag itself is the load-bearing data. Don't make the user feel like they're underperforming if they skip the note.
