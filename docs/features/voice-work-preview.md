# Work with Minutes — integrated development preview

The experimental `voice-live` feature now connects a local desktop Work window,
selected context, live voice, host-reviewed actions, checkpoint persistence, and
agent-result review. This is not enabled in the normal release build. Existing
recording, capture and ordinary Recall remain independent.

## Build and open

Use the repository's pinned Rust toolchain. On macOS use the established dev app
identity, not an ad-hoc replacement of `/Applications/Minutes.app`:

```sh
MINUTES_BUILD_FEATURES=parakeet,metal,voice-live ./scripts/install-dev-app.sh
```

Use `MINUTES_DEV_SIGNING_IDENTITY` when configured for the dev app. Launch
`~/Applications/Minutes Dev.app` and choose **Work preview** beside Recall.
Only feature-enabled builds show the button. Configure the existing
`voice_live.api_key_env` (default `GEMINI_API_KEY`) in the app's environment.
The Work window's cloud consent applies only to the current session and does not
silently save an enabled cloud setting globally.

## One connected workflow

Enter a goal, then paste the exact passage you are discussing. On macOS the
optional Cmd+Alt+Shift+W shortcut reads selected text in the other application
before Minutes takes focus. Accessibility permission must already exist; missing
permission, unreadable/secure fields or changed selection return an error rather
than falling back to clipboard collection or full-screen monitoring. Linux and
Windows support explicit pasted selection, not fake native capture.

Review the context and cloud notice. Start either supported Live model, hold Talk
to transmit microphone audio, or type a turn. The assistant can use authorized
normal-sensitivity meeting tools to relate the selection to conversation history.
The local work record distinguishes model suggestions from your interpretations
and decisions. Review a spoken turn or a proposal in the correction field and
save it yourself to establish your attribution. A spoken "yes" is not execution
authority and never clicks the local approval control.

External tools display their complete arguments in a separate local review.
The request is bound to this runtime instance, work revision and expiry. Editing
work, rejecting, cancelling or stopping revokes queued approvals. Execution
rechecks the stored capability; model-supplied tokens cannot approve actions.
Already-executed effects cannot be undone by cancellation.

Park stops the voice session, requests cancellation, and writes a new immutable,
owner-private checkpoint through Minutes' existing capability-bound storage.
Resuming reconstructs local work and attribution, not workers, approval grants or
a cloud session. Unverified voice transcripts remain explicitly model-attributed.
Corrections are persisted when submitted. Starting/replacing a meaningful work
session also saves the previous work before discarding its UI state.

## Agent work and privacy boundaries

Agent delegation remains off by default. When explicitly enabled, each request
shows the configured executable, arguments, working directory and requested task
for review. The configured agent's actual sandbox/tool policy determines its
access: a read-only prompt is not an OS sandbox. Unix cancellation terminates the
agent process group; the Windows voice adapter refuses delegation until a real
process-tree supervisor is connected. Meeting tools can remain responsive while
the separately queued agent task runs.

The full bounded result is saved locally as a separate artifact, not compressed
into a spoken reply or automatically disclosed to the voice model. The user opens
it, inspects it, and may separately share its exact reviewed version. Oversized
output is rejected visibly rather than silently clipped. Imported records are
untrusted history, never proof of execution permission. Restricted/unknown meeting
sources are excluded from voice reads, and the old derived-insight log is not used
as an unverified bypass. Optional knowledge/prep/MCP reads still require local
review and retain their configured source controls; this preview does not claim
that arbitrary third-party MCP or agent internals are sandboxed by Minutes.

## CLI and other agents

```sh
minutes talk --goal 'Review the proposal'
minutes talk --resume work-<exact-name-returned-by-list>.md
minutes work list
minutes work show work-<exact-name-returned-by-list>.md --markdown
```

Inside `minutes talk`: `/note`, `/decision`, `/approve ID`, `/reject ID`, `/cancel`,
and `/park [next step]` are local host commands. Approval and human-attributed notes
require an interactive terminal, not piped model text. Work reads use the same
private store; agent-facing CLI output refuses records requiring unknown or
restricted source release. There is no new always-on screen observer, unrestricted
Shortcuts executor, autonomous click engine or Herdr-specific control service.

## Verification boundary

`Voice Work integration` compiles and tests the actual feature-enabled Rust paths
on macOS, Windows and Linux, in addition to the existing full repository gates.
The browser smoke test uses the real HTML/CSS/JavaScript with a synthetic host and
no device or sending effects. Its offline mode is DOM-only and does not validate
native IPC/CSP. Those checks do not establish signed macOS TCC behavior, actual
Live provider quality, acoustic performance or native selection reliability.
Native acceptance still requires the signed Minutes Dev build and an explicitly
consented provider session. No release or production installation is performed by
this source change.
