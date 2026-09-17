# Voice Control Acceptance, September 17

Scope: follow-up to the September 17 09:10 local voice session. Source base
0787ab1c in the voice-operate-20260917 Mac worktree. The terminal voice profile
uses Zephyr and Gemini Live Extended Thinking. No microphone session was
started or restarted during verification.

## Behavior

- Decision-board selection is a revision-bound operation visible in the board.
- Generated HTML opens in a controlled, opaque-origin local preview. Voice can
  inspect native range/number inputs, selects and checkboxes, set one exact
  control, and undo its last non-conflicting change. It does not rebuild HTML.
- Scalar writes run native setters and input/change handlers, then read back
  values and bounded output text. The DOM is evidence about the artifact, not
  independent validation of its calculations.
- Control changes are transient preview state. Saved HTML versions stay
  immutable. Older static artifacts and closed previews are not silently
  reopened/reset to simulate successful live edits.
- Other approved foreground Mac apps can expose labelled numeric AX sliders
  through CuaDriver. A single-use 30-second reference binds process, window,
  title, label, geometry and previous value. Set-value is followed by readback.
  No arbitrary scripts, clicks, keystrokes, submissions or permissions bypass.
- Final tool receipts carry structured completion state. Current status separates
  active jobs from completed history and excludes the status query itself.
- Screen evidence is a multimodal function response, not two racing conversation
  turns. This uses the FunctionResponse parts described in Google's API reference:
  https://ai.google.dev/api/generate-content#FunctionResponse
- Selection failures have bounded diagnostic categories without persisting the
  selected text. Unsupported editing does not trigger blind clipboard insertion.

## Verification

The following use synthetic data only, on the daily MacBook:

- Focused voice suite: 158 passed before final documentation/prompt adjustments.
- Controlled browser fixture: 25 to 75 changed output 50 to 150; unrelated input
  stayed 10; undo restored 25 and 50. Out-of-range, wrong-step and stale writes
  were refused. Desktop and mobile browser snapshots were nonblank.
- The controlled viewer was retested after requiring its secret token before
  serving artifact HTML. Unauthenticated requests cannot retrieve that document.
- Native disposable AppKit fixture: CuaDriver read 25, set 75 and read back 75;
  replay of the consumed reference was refused. Initial launch needed to settle
  before its window became visible; no user document was touched.
- Native Live synthetic routing: build, inspect, set slider to 75, then ask what
  is running. Model called get_status and reported nothing running.
- Native Live synthetic message-thread image: one capture, identified the
  unresolved venue, followed up without recapture or following embedded text
  instructing it to read the clipboard.
- Core Clippy with voice-live and warnings denied passed before final prompt edits.

## Limits

This is not universal computer use. Unsupported custom/canvas controls and
Google Docs selected-text replacement remain unqualified. No generic DOM
attachment to personal browser tabs or pixel-coordinate fallback is enabled.
The safe fallback is an explicit explanation, or an explicitly requested artifact
revision that preserves the previous version. Do not imply these gaps are solved
by the native slider adapter. Private Jev evaluation remains disabled.

The installed terminal binary and signed desktop application are separate
deliverables. These controls require the updated terminal build; this receipt
does not assert that the desktop voice UI has been rebuilt with them.
