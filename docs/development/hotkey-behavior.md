# Minutes Hotkey Behavior Spec

This document defines the intended behavior for any future global hotkey
capture mode in Minutes.

The purpose is to preserve **trust, portability, and parity** before any
shortcut implementation is attempted.

Minutes is not a dictation toy. Hotkeys exist to reduce friction for:

- quick thoughts
- memo capture
- fast meeting start

They must not create a second product with different semantics from the tray,
CLI, or MCP surfaces.

## Principles

- Hotkeys are **opt-in**.
- No hidden recording starts.
- No global shortcut should be enabled before permission implications are
  explained.
- The underlying artifact model stays the same:
  - meeting or memo
  - markdown output
  - normal searchability
  - normal recovery behavior

## Supported Modes

We intentionally separate behavior from implementation.

### 1. Hold-to-record

Use case:
- quick thought capture
- one-sentence memo
- transient notes where the user expects press-to-talk semantics

Behavior:
- recording starts when the hotkey is depressed
- recording stops when the hotkey is released
- if the recording is shorter than the minimum duration threshold, it is
  silently discarded and no markdown artifact is created

### 2. Tap-to-lock

Use case:
- user wants hands-free recording without touching the tray

Behavior:
- first valid tap starts recording
- second valid tap stops recording
- there is no hidden intermediate session restart

## Timing Rules

These values are intentionally conservative defaults and should remain
configurable later.

- `minimumKeyTime`: `300ms`
- `minDuration`: `400ms`

Interpretation:

- Release before `minimumKeyTime`:
  - candidate for tap-to-lock behavior
- Release after `minimumKeyTime`:
  - treated as hold-to-record
- Total captured audio shorter than `minDuration`:
  - no artifact
  - no processing
  - no stop/completion cue

## Sound Semantics

If capture cues are enabled:

- Start cue:
  - plays when recording actually begins
  - never plays for a discarded non-start
- Stop cue:
  - plays when recording transitions from active capture to processing
  - never plays for silent discards
- Completion cue:
  - plays when a saved artifact is ready
- Error cue:
  - plays when a recoverable failure happens

Rules:

- No sound should imply a saved artifact when no artifact will exist.
- Hotkey flows must reuse the same cue semantics as tray/UI flows.

## Output Semantics

Hotkeys must not invent a new output format.

- Quick-thought hotkeys produce standard memo artifacts.
- Meeting-start hotkeys produce standard meeting artifacts.
- Titles, search, recovery, and notifications all follow normal Minutes rules.

## Cross-Surface Rules

Hotkeys must respect the same shared state model as all other entry points.

If another surface already owns the session:

- hotkey start should fail with the equivalent of "already recording"
- hotkey stop should act on the same session, not create a new one

If Minutes is processing:

- hotkey start is blocked
- user receives a clear signal rather than silent no-op

If a recovery item exists:

- hotkeys do not auto-consume it
- recovery remains a deliberate user action

## Permission Rules

Hotkeys must not bypass the permission center story.

- If a global shortcut requires Accessibility on a given implementation path,
  that requirement must be shown explicitly before enabling it.
- If a platform-specific approach can avoid Accessibility while still being
  reliable, prefer that.
- Never ask for Screen Recording just to implement hotkeys.

## User-Facing Copy Rules

The app should explain hotkeys in outcome terms, not implementation terms.

Good:
- "Hold to capture a quick thought"
- "Tap again to stop"

Bad:
- "Modifier event promotion"
- "Buffered state transition"
- "Low-level keyboard monitor enabled"

## Non-Goals

Hotkeys are not for:

- dictating into every app by default
- replacing the tray or CLI
- background surveillance capture
- creating a separate interaction language unrelated to the rest of Minutes

## Definition Of Done

Hotkey implementation is not ready until:

- behavior matches this spec
- cue semantics match tray/UI semantics
- artifacts remain normal markdown outputs
- readiness/permission messaging is honest
- conflicts with tray/CLI/MCP sessions are handled cleanly
- recovery behavior remains intact
