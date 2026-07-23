# Re-Summarize: rerun the AI pass on a meeting transcript

**Date:** 2026-07-21 (revised 2026-07-23)
**Status:** Approved by maintainer (issue #523, 2026-07-23) — implementation go. Codex review (gpt-5.6-terra, xhigh) incorporated 2026-07-23 — all 16 findings applied; maintainer notes M1–M5 folded in below
**Motivating session:** manual resummarization of an edited meeting file, 2026-07-20/21

## Maintainer response (issue #523, 2026-07-23)

Direction confirmed, PR invited. Five notes, each folded into the phase steps
below with an (M#) tag:

- **M1 — one engine-resolution choke point.** Engine resolution must go through
  the exact same path the pipeline uses — no parallel resolution logic.
  In-flight privacy work will gate what leaves the machine per meeting, and a
  single choke point means resummarize inherits that policy for free instead of
  becoming a bypass. (Phase 1 step 4)
- **M2 — merge doctrine confirmed.** The status-preserving merge is the hardest
  part; ambiguous identity matches surface in the preview rather than being
  decided silently. Same doctrine as speaker mapping. (Phase 1 step 10)
- **M3 — hard no-write confirmed + cost transparency.** No write on
  `engine = "none"`, provider error, or empty result; the preview must clearly
  state that it invoked the model. (Phase 1 step 6, Phase 2)
- **M4 — run record shape decided.** Follow the shape of the existing
  `speaker_mapping` health block for consistency — this resolves step 11's
  open choice in favor of a `summarization:` health block, not the
  warnings-replacement variant. (Phase 1 step 11)
- **M5 — scope confirmed.** v1 text-only, CLI + core only, desktop button and
  MCP tool as separate follow-ups.

Timing: 0.23.0 is about to cut; this lands in the release after. No deadline
pressure — priority is getting the merge semantics right.

## Problem

The desktop app's "Open in Editor" button actively encourages editing the meeting
file, and the transcript section is a legitimate edit target:

- correcting transcription errors significant enough to drift the summary
- deleting worthless sections (phone trees, hold music narration)
- striking hot-mic moments or sensitive content from the record
- fixing recording errors not worth re-editing audio for

But nothing re-runs the summarization pass afterward. The `## Summary` section and
the derived frontmatter go stale, which defeats the purpose of the edit. Full audio
reprocessing does not safely refresh an edited artifact: it re-transcribes from
scratch, typically creates a duplicate output (filename-collision resolution), and
in rewrite/retry paths can replace the edits. The only other path is a fully manual
rewrite of the AI-owned sections.

Conceptually the feature is: **rerun the AI pass on the meeting transcript** —
whatever the final button/command label ends up being.

## Grounded architecture facts (verified 2026-07-21 @ f05aa87; re-verified 2026-07-23 @ 3b13375e)

- The pipeline's summarization is **one self-contained stage**: a single call into
  `summarize::summarize_with_template` (`crates/core/src/summarize.rs:92`). The
  returned `Summary` struct carries text, key points, **decisions, action items,
  open questions, commitments**, and participants. NOTE (Codex F1): this is one
  *stage*, not necessarily one model request — `build_prompt`
  (`summarize.rs:539`) chunks long transcripts and the engines do map-reduce
  (per-chunk summarize → synthesis), so actual invocation count depends on
  engine and transcript length. Don't claim "exactly one LLM call."
- Everything else is derived **locally, no further LLM calls**:
  - `action_items:` / `decisions:` / `intents:` frontmatter — string parsing of
    the `Summary` (`extract_action_items` etc., `crates/core/src/pipeline.rs:4357`)
  - `entities:` + `people:` — `build_entity_links` canonicalization; inputs are
    broader than the transcript alone: attendees, title, context, tags, and
    vocabulary/identity config all feed it (Codex F13).
- Title refinement is a **separate optional** LLM call (`refine_title`).
- Speaker mapping is a **separate** LLM call with its own recovery command
  (`minutes redo-speaker-mapping`), which merges without ever downgrading
  High-confidence attributions and records a `speaker_mapping` health block.
  Its merge is **exact-label/confidence-based, not fuzzy** (Codex F5).
- The hidden `.<slug>.embeddings` sidecar is **voice embeddings from audio**
  (`voice::save_meeting_embeddings`), keyed by the *original* diarization labels
  (`SPEAKER_n`). Transcript edits never invalidate the vectors; but manual
  *renames* of transcript/speaker_map labels break the label linkage that
  `minutes confirm --save-voice` uses (`embeddings.get(speaker_label)` misses).
- User notes (`## Notes`) are weighted heavily by the summarizer (the
  `USER_NOTES_HEADER` "weight them heavily" framing); passed as a separate
  parameter, though some engines compose them into a single prompt.
- `markdown::update_frontmatter` (`markdown.rs:~1040`) is the in-tree precedent
  for surgical rewrites: fail-closed (refuses unparseable frontmatter),
  validates the result parses before swapping, atomic tmp-sibling write,
  **preserves the existing file mode** (#384). File modes are not uniformly
  0600: `Visibility::Team` writes **0640** (`markdown.rs:677`).
- `resolve_single_meeting` (`crates/cli/src/main.rs:4766`) is **hard-filtered to
  meetings** — search resolution explicitly excludes memos/dictation; only the
  direct-path branch can reach files under the memos subdir.
- The jobs queue (`jobs.rs`) has a `Summarizing` state, but job payloads and the
  worker are **audio/transcription-specific** — it is not a generic background
  queue; a text-only job needs a new job kind (Codex F4).

New since f05aa87 (re-verification 2026-07-23):

- **`minutes import text` (#516, landed 2026-07-21)** converts existing text
  archives into meeting files (`source: text-import`) with **no LLM pass**:
  no generated summary and no summary-derived frontmatter (the source text may
  itself contain arbitrary headings, so phrase it as "no generated summary",
  not "no Summary section" — Codex F16). A resummarize command is exactly the
  missing first-AI-pass for them; motivation bullet for the issue.
- **Speaker mapping now supports the Apple engine (#498)** and
  `cmd_redo_speaker_mapping` already does a per-run engine override via
  `cfg.summarization.engine` + `speaker_mapping_model_hint` — the `--engine`
  pattern Phase 2 needs is proven in-tree.
- **`cmd_redo_speaker_mapping` refuses non-Meeting content types** (memos /
  dictation bail early). Do **not** copy that gate blindly: memos also get
  summaries (pipeline has a second `summarize_with_template` call site,
  ~`pipeline.rs:2516`, besides the meeting path at ~`pipeline.rs:1904`), so
  `resummarize` should accept memos too — which requires resolver work, see
  Phase 1 step 1.
- Drifted line anchors: `cmd_redo_speaker_mapping` is now `main.rs:4903`
  (`transcript_section` 4811, `merge_speaker_map` 4861); the summarize stage
  anchors are ~1904 / ~2516, not ~1860–1990. `summarize_with_template`
  (summarize.rs:92), `extract_action_items` (pipeline.rs:4357),
  `extract_decisions` (4390), `extract_intents` (4418), `build_entity_links`
  (4540), `refine_title` (312), `map_speakers` (2673, still hardcoded
  `Confidence::Medium`), `save_meeting_embeddings` (voice.rs:817) all
  re-verified.

Consequence: re-running the summarize stage plus the local derivations
regenerates essentially everything a transcript edit invalidates. The feature
fits the architecture cleanly — but the safety contract around the write is the
real work (below).

## Phase 1 — `minutes-core::resummarize_meeting()` (the real work)

`pipeline::resummarize_meeting(path, config, opts) -> ResummarizeReport`, shared
by CLI and Tauri (both statically link core).

1. **Resolve + validate the artifact (Codex F7).** Accept meetings, memos, and
   `source: text-import` files. `resolve_single_meeting` search resolution is
   meeting-only today — build a resolver that also searches memos, or extend the
   existing one with an opt-in content-type set. Explicitly **reject** in v1:
   dictation, `NoSpeech`-diagnosed artifacts, and `capture: none` / sensitive
   artifacts (their "transcript" may be a placeholder plus human-authored
   debrief — resummarizing would destroy meaning). Require parseable
   frontmatter + a canonical `## Transcript` section.
2. **Section parsing is a first-class deliverable (Codex F6, F14).** The
   CLI-local `transcript_section` extractor is not a general splicer. Build a
   core-owned, fence-aware **section-range parser** in `minutes-core::markdown`
   (move/extend `transcript_section`): handles fenced code blocks, duplicate
   headings, CRLF, missing/malformed sections, and custom user text inside
   AI-owned sections. **Fail closed:** ambiguous documents (e.g. duplicate
   `## Transcript` or `## Notes` headings) are rejected without writing —
   surfaced in the preview. `## Notes` extraction uses the same parser and the
   same canonical-section policy.
3. **Reassemble inputs — text-only in v1 (Codex F8).** Transcript from body,
   user notes from the canonical `## Notes` section. **No screenshot re-feed:**
   screens dirs are keyed by original audio stem with no durable provenance in
   frontmatter, retention varies by path, and the Apple engine ignores images
   anyway. Image-aware resummarize is future work gated on persisted
   screenshot provenance.
4. **Engine/template contract (Codex F10, M1).** Default to the template
   recorded in the artifact's frontmatter; if it is unavailable, **fail
   visibly** (no silent default-template swap — that changes output shape).
   `--template` overrides explicitly; record the resulting template back to
   frontmatter. Engine: config engine by default, `--engine` override per run
   (pattern proven in `cmd_redo_speaker_mapping`, including Apple). Desktop
   uses the configured engine unless its UI later grows an override.
   **Maintainer constraint (M1): resolve the engine through the exact same
   code path the pipeline's summarize stage uses — no parallel resolution
   logic.** In-flight privacy work will gate per-meeting egress at that choke
   point, and resummarize must inherit the policy rather than bypass it. If
   the current resolution logic is inline in the pipeline, extract it into a
   shared fn and call that from both sites (and note this in the PR so the
   privacy work knows the second caller exists).
5. **Run the summarize stage.** Call `summarize_with_template`, then the
   existing local derivations: actions, decisions, intents,
   `build_entity_links`.
6. **Never destroy on failure (Codex F2, M3 confirmed).** `engine = "none"`,
   provider errors, an empty `Summary`, or malformed structured output are
   **hard no-write failures** — no backup, no mutation, non-zero exit,
   explicit error in `--json`. A splice may only proceed from a validated,
   non-empty candidate.
7. **Concurrent-edit guard (Codex F3).** The feature's own premise is that the
   file is being edited externally. Hash the full file before inference;
   re-read and compare immediately before applying; on mismatch, abort with a
   retryable conflict error ("file changed while summarizing — save and
   re-run"). Atomic tmp-sibling rename alone does not cover this.
8. **Splice, don't rewrite.** Replace only AI-owned body sections
   (`## Summary`, `## Decisions`, `## Action Items`, `## Open Questions`,
   `## Commitments`) and derived frontmatter fields, via the fence-aware parser
   ranges. Never touch `## Notes`, `## Transcript`, `speaker_map`,
   date/duration/consent/recording metadata. Atomic write; **preserve the
   existing file mode** (not "0600" — team-visibility files are 0640; follow
   `update_frontmatter`'s #384 behavior). Timestamped backup of the prior file
   before rewrite, with a defined location + retention story and
   backup-before-write failure ordering.
9. **Frontmatter field contract (Codex F13).** Document every field as one of:
   **preserved** (speaker_map, attendees, date, duration, consent, recording
   metadata, tags, title), **recomputed** (entities, people, intents),
   **merge-derived** (action_items, decisions — see step 10), or **recorded**
   (template, engine/model, run health). This table is part of the core fn's
   doc comment and the PR description.
10. **Status-preserving merge — exact identity, not fuzzy (Codex F5, M2
    confirmed).** Action
    items and decisions accumulate user state (`status: done`, assignees, due
    dates, decision authority/supersession). Blind regeneration resets it, and
    fuzzy matching can silently attach user state to the wrong item —
    `merge_speaker_map` is exact-label-based and is precedent for
    *never-downgrade*, not for fuzziness. Rule: match by **exact normalized
    task identity** (case/whitespace/punctuation-normalized text) for automatic
    carry-forward of all user-owned fields; anything ambiguous, unmatched, or
    removed is **surfaced in the preview for explicit resolution**, never
    silently resolved. Render body sections from the merged structured state so
    frontmatter and body checkboxes cannot diverge. This is "a wrong rewrite is
    worse than none" applied to summaries — deserves the densest test coverage.
11. **Run record + warnings hygiene (Codex F9, M4 decides).** Record the run
    (model, duration_ms, last_run) in a new **`summarization:` health block
    whose shape follows the existing `speaker_mapping` health block** —
    maintainer note M4 settles the previously open choice; the simpler
    "replace `step=summarize` warnings" variant is dropped. Warnings hygiene
    still applies: keep unrelated capture/diarization warnings, and do not
    blindly preserve or blindly overwrite overall status. **Drop the
    speaker-label consistency report from v1** — after
    high-confidence mapping, rendered names replace raw `SPEAKER_n` labels in
    the transcript, so "speaker_map label absent from transcript" false-flags
    healthy files; redesign later around raw-vs-rendered labels if wanted.

## Phase 2 — CLI: `minutes resummarize <meeting>`

Thin wrapper following `redo-speaker-mapping`'s CLI conventions — resolve by
path or search term, `--apply`, `--engine`, `--template`, `--json` — but *not*
its meeting-only type gate (see Phase 1 step 1). Default mode is
**preview/no-write** (Codex F11): call it that, not "dry-run", and document
that the preview **does invoke the model** (cost + privacy implications —
transcript and notes leave the machine on cloud engines), exactly like
`redo-speaker-mapping`'s dry-run. Per M3, the preview output itself must
clearly state that the model was invoked (cost transparency), not just the
docs. Preview shows the new summary, which
sections change, and every merge decision needing resolution (step 10). Ships
standalone value before any UI work. Also serves `source: text-import` files
(#516) as their first AI pass.

## Phase 3 — Desktop app button (not a thin reuse — Codex F4)

- The jobs queue's payloads and worker are audio-pipeline-specific; a Markdown
  artifact cannot be represented as a current job. Scope honestly: add an
  explicit `Resummarize` job kind with an artifact-path payload, a worker
  branch calling the Phase-1 core fn, serialization backward-compatibility for
  existing queued jobs, retry/progress/notification semantics, and per-file
  locking (one resummarize per artifact at a time; compose with the Phase-1
  concurrent-edit guard). The existing `Summarizing` progress state/label is
  reusable; the rest is new.
- UI: button beside "Open in Editor" on the meeting view; disabled while that
  file has an active job; view refreshes on job completion.
- Pre-commit checklist applies: dev-app build + click-test in
  `~/Applications/Minutes Dev.app` (UI render verification is not covered by
  type checks or unit tests).
- If this scope is too heavy for the first PR, defer Phase 3 entirely rather
  than wedging a text job into audio-shaped plumbing.

## Phase 4 — follow-ups

- **Mandatory, not optional (Codex F11):** define index/graph invalidation for
  rewritten summary-derived frontmatter — anything that caches derived views
  (graph.db, knowledge exports) needs a documented rebuild/invalidation story,
  separate from opt-in ingestion. QMD and MCP read files directly and need no
  help.
- MCP tool `resummarize_meeting` — a follow-up milestone, not "nearly free"
  (Codex F15): needs schema, path/scope validation, async semantics (long
  runs), permission posture, and an error contract on top of shelling out to
  the CLI.
- `--retitle` flag for the separate title-refinement call, **off by default**
  (filename slug derives from title; silent renames break links).
- `--ingest` to chain knowledge/graph re-ingestion; vault re-sync if
  configured.

## Out of scope

Re-transcription, re-diarization, voice embeddings (audio-derived, unaffected),
speaker mapping (has its own command), screenshot re-feed (v1 — see Phase 1
step 3), dictation / `NoSpeech` / `capture: none` artifacts (v1 — see Phase 1
step 1). Note for redaction use-cases: transcript edits don't touch the
retained WAV — true audio redaction is `minutes cleanup`/retention territory
and the docs should say so.

## Effort shape

Phase 1 is the bulk of the work — and larger than the original draft assumed:
the fence-aware section parser, the exact-identity merge with preview-surfaced
conflicts, and the failure/concurrency guards each deserve dense tests. Phase 2
is thin. Phase 3 is moderate (new job kind, not a thin queue addition) and can
be deferred. Phase 4 items are independent follow-ups.

## Workflow

This repo is the fork (`origin` = rymalia/minutes; `upstream` =
silverstein/minutes). Sequence: file the feature-request issue on upstream →
maintainer buy-in → implement on a fork branch → PR to upstream (same flow as
PR #421). Status: issue filed and **buy-in received 2026-07-23** (see
Maintainer response section) — implementation is unblocked; target the
post-0.23.0 release window.
