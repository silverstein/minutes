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
by the native slider adapter. Jev evaluation is off by default; the explicit
request-scoped opt-in below is now available for the terminal profile.

The installed terminal binary and signed desktop application are separate
deliverables. These controls require the updated terminal build; this receipt
does not assert that the desktop voice UI has been rebuilt with them.

## Follow-Up: 10:04 and 10:12 Demo Sessions

Base: 135aeeed. The truffle failure was a call ID passed as prototype_id, followed
by an invented Chrome window ID; no set-control call ran. The actual artifact's
native controls were supported. Fixes and boundaries:

- `list_prototypes` discovers registered preview identities. Unknown references
  return the actual identities and a recovery instruction, with no write or reset.
  Writes still require an exact artifact and freshly observed control/snapshot.
- A late running-status context injection was removed. Final receipts explicitly
  supersede progress; approval proposals are `AwaitingApproval`, not executed.
  `get_status` separates pending review, active jobs and completed history.
- `create_apple_note` creates a new Apple Notes document on explicit request and
  reads back its plain text. It cannot overwrite/append existing notes. Notes may
  sync through the default account. `add_note` remains a reviewed Minutes meeting
  annotation, never an Apple Notes substitute.
- Personal review requests use `read_pull_requests(review_requested=true)` across
  repositories; repository-name search rejects misplaced PR filters.
- Room-conversation guidance distinguishes demo commentary from direct requests.
  This is not speaker authentication or guaranteed background-speech filtering.
- Songs about review findings must wait for those findings, unlike independent
  music requests. Music receipts distinguish generation from host playback.

### Timing Evidence

Every log item now has local wall time and monotonic session elapsed time.
Structured events include tool queued/started/result/delivery, input transcript
chunks, provider interruptions/turn completion, first response audio received,
and first speech/music sample consumed in the output callback. No raw audio is
stored. Callback consumption is not proof of acoustic sound. Transcript arrival
is not microphone end-of-speech; do not report it as true end-to-end latency.
The selected-session report stays local:

```sh
node tooling/voice-evals/session-timing.mjs /absolute/path/to/session.md
node tooling/voice-evals/session-timing.mjs --self-test
```

Two synthetic simple text requests per model gave first-audio samples of
891/1047 ms for standard Live and 751/1127 ms for Extended Thinking at low depth.
This small sample does not establish parity on complex spoken tasks. Zephyr and
Extended Thinking low remain configured; no microphone was restarted.

### Request-Scoped Jev

`voice_live.jev_evaluation` defaults to false. When explicitly enabled, completed
search/inspection results can carry an `evaluation_id` valid for 60 seconds.
`evaluate_candidates` can rank at most eight host-observed candidates, each capped
at 600 characters, and a goal capped at 512 bytes. Only whitelisted short
titles/snippets/labels/date/type fields are sent to `typesafe-ai/jev` through
Vercel AI Gateway; local file paths and control identities remain local.
Screenshots, clipboard, full documents and conversation histories are not sent.
There is no background index/upload or scheduled evaluation. The evaluator has
an eight-second deadline, rejects unknown choices, and cannot authorize actions.
Failure leaves original search candidates available. This ranks retrieved
candidates; it does not add a semantic index that can retrieve otherwise missing
documents. The daily terminal profile was opted in after explicit user consent.

### Additional Verification

- Actual saved truffle HTML, opened in an isolated test preview: recover identity,
  inspect price 45, set 100, verify monthly output 25,000, undo to 45 and 11,250.
  Screenshot confirmed the live 100/25,000 state. Saved HTML was unchanged.
- Native Live synthetic bad-reference recovery, set-control and completed-status
  sequence passed. It recovered the ID instead of rebuilding or changing apps.
- Native Live synthetic demo-commentary test triggered no actions; explicit
  Apple Notes and cross-repository review-queue requests selected the right tools.
  The initial PR-routing probe timed out; the explicit personal-review guidance
  was then added and the repeated three-case probe passed. This is not real
  multi-speaker audio qualification.
- Apple Notes disposable creation passed text readback, including literal HTML
  characters and a newline; the test note was then removed by exact ID/title.
- Browser scratch insertion safely refused because the clipboard exceeded the
  preservation limit. No clipboard or draft content changed. Successful paste
  remains unproven for that run; no clipboard-clearing workaround was used.
- Request-scoped Jev runtime path selected the correct synthetic slider in 806 ms.
  No private corpus or conversation was used in qualification.
- Focused voice suite: 165 passed, 27 optional/live tests skipped at this point.
  Core library Clippy with voice-live and warnings denied passed.
- Configuration suite: 66 passed. Local timing-report self-test passed.

## Late-Completion Cancellation Follow-Up

Music requests now carry a session playback generation. Stop or pause invalidates
autoplay for earlier requests, including completed audio waiting for delivery to
the player. A new music request can still play normally; resuming an existing
track does not resurrect a suppressed request. The delivery boundary also checks
the exact job's cancellation token. `/cancel` invalidates queued autoplay as well
as cancelling work. This does not cancel an already-sent remote music request or
claim a refund: generation may finish and save its file without playing it.

The host logs `music_playback_withheld` and tells the conversational model when a
late result was not played. Stop/pause during generation is handled by Minutes
even if no generated track is loaded yet. Errors from direct music controls are
recorded as failed jobs, not completed successes.

Preview opening checks cancellation after acquiring the artifact lock and again
after loading HTML and starting the preview server. A cancelled preview is not
registered or opened; the unused server is dropped. Once an OS browser-open
request has actually been issued, cancellation does not claim to undo it.

Regression cases cover stop/pause before completion, stop after completion while
audio is queued, per-job cancellation at delivery, new music after stop,
unrelated board completion, and cancelled preview registration. These are
deterministic host checks, not a new microphone/AirPods or foreground-app test.

Verification: 169 voice tests passed (27 optional/live tests skipped), core
voice-live library Clippy passed with warnings denied, and the terminal CLI
build passed. The desktop app bundle and audio-device configuration were not
changed during this follow-up.
