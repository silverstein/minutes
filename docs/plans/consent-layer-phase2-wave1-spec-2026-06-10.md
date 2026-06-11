# Build Spec — Consent Phase 2, Wave 1: Require Modal + Sensitive Meeting Mode

Author: Claude (handed to Codex). Parent plan: `consent-layer-phase2-2026-06-10.md`.
Beads: `minutes-3yub.1` (modal), `minutes-3yub.2` (sensitive mode).
Repo: ~/Sites/minutes (work here; cd first, NOT ~/.minutes/assistant).

Same hard constraints as v1 (`consent-layer-spec-2026-06-04.md`), restated where they
bite. Read the parent plan's "core reframe" section before coding: sensitivity is an
enforcement contract for agents; Wave 1 lays the designation + artifact groundwork
that Wave 2 enforces.

## Part A — Desktop Require modal (`minutes-3yub.1`)

### What

When `config.consent.mode == Require` and a **meeting** recording is about to start
from the desktop app, show a blocking confirmation dialog instead of the current
non-modal notification. Confirm proceeds (and records consent exactly as today);
Cancel aborts the start cleanly (no PID, no job, no sidecar left behind).

### Where

- `tauri/src-tauri/src/commands.rs`: the `TODO(phase 2)` sits in
  `maybe_save_and_show_recording_consent` (Require arm currently mirrors Remind).
  The gate must run BEFORE capture starts in `cmd_start_recording` (~line 5789),
  not after; restructure so Require returns a "needs confirmation" signal to the
  frontend rather than spawning capture and notifying.
- Recommended shape: `cmd_start_recording` returns a new structured variant (e.g.
  `Err`-free `StartOutcome::ConsentRequired { disclosure: String }`) when mode is
  Require and no confirmation token was supplied; frontend shows the modal and
  re-invokes with `consent_confirmed: true`. Mirror how existing two-step flows do
  it if one exists; otherwise this request/confirm round-trip is the pattern.
- `tauri/src/index.html`: new overlay following the existing dialog pattern
  (`detail-overlay` / `readiness-overlay`: `role="dialog"`, `aria-modal="true"`,
  same card styling, line ~4667 region). Content: the configured disclosure script
  (or compiled default), one Confirm button ("Start recording"), one Cancel.
  No new fonts/colors/radii (DESIGN.md).

### Constraints

- Only the desktop interactive path changes. CLI behavior (TTY prompt, non-TTY
  degrade-to-remind + unattested) is already correct: do not touch
  `prepare_recording_consent`.
- Palette/hotkey/automation-initiated desktop starts: treat as interactive (they
  end in the same UI) and show the same modal. Tray quick-record included.
- Recordings with an explicit basis already supplied (call-detect intent path or
  `default_basis` set) still confirm under Require: Require means a human clicks.
- Copy discipline: dialog may not claim legality. Use the disclosure script verbatim
  plus neutral buttons.

### Acceptance

- Dev-app click-test (mandatory, `~/Applications/Minutes Dev.app`): Require mode
  shows modal; Confirm records with consent sidecar identical to v1; Cancel leaves
  zero artifacts; Remind/Off behavior unchanged.
- Unit tests for the new outcome variant; Tauri tests for the gate ordering.

## Part B — Sensitive Meeting designation + no-capture mode (`minutes-3yub.2`)

### What

A meeting the user designates **sensitive** produces a normal meeting artifact with
NO audio captured: typed markers during, guided debrief after, provenance fields that
Wave 2's agent-layer enforcement will key on.

### Frontmatter contract (the load-bearing part — design first, code second)

New optional fields in meeting frontmatter (additive, back-compatible):

```yaml
capture: none            # absent => normal captured meeting
sensitivity: restricted  # absent => normal; values: normal | restricted
```

- Written only by the human-initiated flows below. Agents never write these
  (RFC #194 discipline).
- `crates/core/src/markdown.rs`: extend frontmatter parse/serialize + JSON schema
  snapshot; `crates/reader` and `crates/sdk/src/reader.ts` must parse them
  (read-only exposure now; Wave 2 adds enforcement).

### CLI surface

- `minutes note --sensitive --title "Board sync"` is NOT the shape. Instead:
  `minutes sensitive start --title "Board sync"` / `minutes sensitive stop`
  (subcommand keeps the recording verbs unambiguous). `start` creates the session
  file + PID-style lock (reuse `pid.rs` flock pattern, separate lock name so a
  sensitive session and a recording cannot run simultaneously: starting one while
  the other is active errors).
- During: `minutes note "text"` already targets the active session via the
  notes machinery; route markers to the session and ALSO append a
  `sensitive.marker` event to the event bus (`crates/core/src/events.rs`,
  append-only JSONL, same envelope as existing events; new `event_type` string,
  no payload beyond timestamp + text + session ref).
- `stop` finalizes: prompts (TTY only) for the debrief sections (summary,
  decisions, action items: reuse the structured extraction shapes), writes the
  meeting markdown with `capture: none`, `sensitivity: restricted`, consent fields
  per v1 (basis defaults to `na` for no-capture; it records nothing).
- Non-TTY `stop`: write the file with markers only and a `debrief: pending` field;
  never hang (v1 hard constraint).

### Desktop surface (minimal for Wave 1)

- One entry point: a "Sensitive meeting" action (palette + tray menu item) that
  starts/stops the same core session via new `cmd_sensitive_start/stop` wrapping the
  core functions. Markers reuse the existing Add Note window. Debrief on stop:
  open the meeting file in the assistant with a debrief prompt (reuse the
  `/minutes-debrief` skill contract) rather than building a new panel. NO new
  panels beyond a start/stop state chip; defer richer UX until the designation
  proves itself.
- Calendar pre-designation (auto-suggest sensitive for matching events) is OUT of
  Wave 1; manual designation only.

### Constraints

- The recorder must be provably off: the sensitive session path must not touch
  `capture.rs`/`streaming.rs` at all (no muted-capture tricks; no audio objects
  constructed).
- Skill mirrors: any new/changed skills sync to `.agents/skills/minutes/` and
  `.opencode/` per repo rule.
- Docs: README section + CONFIG.md for new fields; llms.txt regenerated.
- Tests: frontmatter round-trip (core + reader + reader.ts), event append, lock
  exclusivity both directions, non-TTY stop, CLI integration test. Site test-count
  sync after.

## Out of scope (resist)

- Any agent-surface filtering (Wave 2: `minutes-3yub.4`).
- Retention changes (Wave 2: `minutes-3yub.3`, see `retention-audit-2026-06-10.md`).
- Calendar auto-designation, prep panels, marker hotkeys.

## Verify (all must pass; report results + diff)

`cargo fmt --all -- --check` · `cargo clippy --all --no-default-features -- -D warnings`
· `cargo test -p minutes-core --no-default-features` · reader + sdk tests ·
`node scripts/sync_site_release_version.mjs` · dev-app click-test for Part A and the
desktop entry point of Part B.
