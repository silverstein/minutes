# Audit — Current Audio Retention Behavior (prereq for minutes-3yub.3)

Author: Claude, 2026-06-10. Read-only audit of main at `0b7a062`.
Feeds the Wave 2 retention work in `consent-layer-phase2-2026-06-10.md`.

## Headline finding

Retention machinery **already exists and is deliberately preview-only**. Wave 2 is an
extension (sensitivity tiers + an opt-in destructive apply path), not greenfield.

## What exists today

### Config (`crates/core/src/config.rs`, `RetentionConfig`)

| Key | Default | Meaning |
|---|---|---|
| `successful_audio_days` | 30 | keep successful recording audio this long |
| `failed_audio_days` | 90 | failed/needs-review audio kept longer for recovery |
| `keep_pinned_audio` | true | honor `audio_retention: pinned` in meeting frontmatter |
| `auto_cleanup` | **false** | future destructive runners must opt in |
| `cleanup_on_startup` | **false** | startup may not trigger cleanup |
| `warn_above_gb` | 2 | storage warning threshold |

Product stance (doc comment): markdown transcripts are the durable library; raw audio
is a temporary recovery/reprocessing layer unless explicitly pinned.

### Engine (`crates/core/src/retention.rs`)

- `preview_audio_retention(config, now) -> RetentionPlan` — pure planner, no deletion.
- Classes: `Successful`, `FailedOrNeedsReview`, `RuntimeScratch`.
- Actions: `Keep` / `DeleteCandidate` with human-readable `reason`, age, linked
  markdown path, byte totals.
- `RuntimeScratch` (= `~/.minutes/jobs/` + `~/.minutes/native-captures/`) is
  inventoried but **never** marked delete-candidate ("not auto-deleted yet").
- Frontmatter `audio_retention: pinned` is parsed and honored.

### Surface (`crates/cli/src/main.rs`, `cmd_storage`)

`minutes storage [--json]` prints the plan + storage summary. **No destructive apply
path exists anywhere** (no flag, no runner, no Tauri command).

## Artifact lifecycle map (where audio lives and dies today)

| Artifact | Location | Current end-of-life |
|---|---|---|
| Recording job WAVs | `~/.minutes/jobs/job-*.wav` | moved/archived by jobs lifecycle (`jobs.rs`: archive on terminal state, `hard_link`+`remove_file` moves); orphaned files persist indefinitely |
| Native call stems + `.mov` | `~/.minutes/native-captures/`, preserved beside meeting md | persist; counted by retention scan as scratch; 0.18.1 made cleanup stem-aware |
| Failed captures | `~/meetings/failed-captures/` | persist (90-day class in preview) |
| Watcher inputs (memos) | moved to `processed/` / `failed/` | persist after move; sidecar removed (`watch.rs:281`) |
| Live transcript WAV | optional preservation per live session | **not classed by the retention scanner** (gap) |
| Dictation audio | not persisted as standalone WAVs (clipboard + daily note text) | n/a (verify on Wave 2 spec) |

## Gaps Wave 2 must close

1. **No destructive path.** Need an opt-in apply runner (`minutes storage --apply` or
   similar) gated on `auto_cleanup`, honoring pin + class rules, with a dry-run default
   and an explicit per-run log of what was deleted and why.
2. **No sensitivity dimension.** Retention classes know success/failure, not
   sensitivity. Wave 2 adds: per-sensitivity retention overrides (e.g. `restricted`
   audio deleted immediately after transcription, or never captured at all via the
   Wave 1 no-capture mode), resolved from the meeting frontmatter the same way
   `audio_retention: pinned` already is.
3. **Live transcript WAVs unclassed.** Add to the scanner.
4. **Transcript retention is out of scope of the current engine** (audio only). Wave 2
   decides whether transcript deletion is in scope; recommendation: audio-only first,
   transcript retention as a separate explicit decision (deleting the durable library
   contradicts the product stance and deserves its own consent UX).

## Reuse, don't rebuild

The Wave 2 spec should extend `RetentionConfig` + `retention.rs` classes and wire the
apply path through the existing plan/reason structure, so `minutes storage` remains the
single pane of glass: preview shows exactly what apply would do.
