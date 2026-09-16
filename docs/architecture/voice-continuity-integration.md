# Voice continuity: integrated CLI experiment

Status: implementation branch; no release or installed-app change. Native UI
and live-provider behavior require verification in Minutes Dev before release.
This connects the portable foundation in PR #1004 to the real `minutes talk`
runner, rather than treating contract tests as evidence of a finished product.

## Product decision

The experiment is one workflow: share an artifact, discuss it against authorized
conversation history, correct an interpretation, park the work, and resume it
later. It is not an always-on desktop recorder or an unrestricted agent command
center. The Mac adapter can lead discovery while the checkpoint, approval and
cancellation semantics remain portable. Core recording ownership is unchanged.

The first behavior worth evaluating is whether the user repeats less context
and can resume a real piece of work. Count wrong referents, repeated explanations,
corrections that do not stick, and time to useful progress. Model acknowledgments
and added desktop verbs are not successful outcomes. Compare standard Live and
Extended Thinking with the same tasks, including denial, interruption, a changed
recipient, a stale result, and a socket reconnect. Native parity is not a release
requirement for an experimental capability; honest availability and existing
capture/CLI/MCP reliability are.

## A complete local checkpoint loop

Build the existing CLI with `voice-live`. The new offline mode does not need
voice enabled, cloud permission, an API key, a microphone, or a network session:

```sh
cargo run -p minutes-cli --no-default-features --features voice-live --bin minutes -- talk --local-work
```

Inside it:

```text
/work new Simplify the proposal
/work debrief I was interested, but did not agree to the expanded scope.
/work show
/work park
/work list
/work resume <identifier-returned-by-park>
q
```

These use the same storage as the live session. `--json` emits local results as
JSON lines. The offline host rejects sharing, approval and execution commands.

## The integrated live loop

Use the existing `[voice_live] enabled=true`, `allow_cloud=true` configuration
and `api_key_env` credential. Cloud consent includes microphone audio and
explicitly released tool context. Neither mode silently turns on optional
screen, desktop, MCP, agent or music access.

```sh
cargo run -p minutes-cli --no-default-features --features voice-live --bin minutes -- talk --ptt
```

An empty input line starts/stops push-to-talk. Ordinary typed lines are sent to
the provider. Commands beginning with `/` are routed to the host first, never
accepted from microphone transcripts or model text. Unknown commands stay local.

Start or resume a work item with `/work new GOAL` or `/work resume ID`. Both are
local. `/work show` previews it. `/work share` explicitly sends the current
snapshot, including private interpretations, to the configured provider. This
is a deliberate disclosure of historical working notes, **not** a claim that
old source documents remain authorized or accurate. Current factual claims must
be retrieved again through normal policy-safe meeting tools. `/work forget`
clears only the local active context; it cannot remove already-shared provider
context or delete saved files.

Discuss the proposal and ask the model to park a checkpoint. The new
`propose_checkpoint` tool stages a goal, model-authored summary and next step.
The exact resulting capsule is frozen at proposal time and shown locally.
Existing private capsule history is not echoed in the model's proposal receipt.
Only `/approve ID` releases that exact payload to the worker. `/reject` revokes
pending approval. Saving a model summary does not convert it into an accepted
substantive decision: it remains a suggestion. `/work debrief WORDS` separately
stores a private user interpretation without rewriting the transcript.

On macOS, with `screen_on_request=true` and Accessibility permission, use
`/share-selection [BUNDLE_ID]`, for example `/share-selection com.apple.Safari`.
This is an explicit selected-text disclosure. It is queued off the audio loop,
reads the chosen app's focused element, rejects secure fields, and verifies the
window/element/selection did not change during the read. It does not activate an
app, read the clipboard, take a screenshot, or broaden to full-window text when
selection is unavailable. It captures at execution time, not at an unimplemented
hotkey press. Explicit app identity is useful when the CLI has focus. Windows
and Linux return Unsupported rather than substituting a broader capture path.
Generic text can still contain sensitive information; secure-field rejection
is not a guarantee of comprehensive redaction.

## Host review is an execution boundary

All model-requested local mutations, desktop mutations, screen frames, music,
MCP calls and delegated-agent invocations stage a local review. Disabled
capabilities fail before staging and are checked again before execution. The
legacy `confirm` token is not in the voice tool schema and is rejected by this
host path. Recalled text, another utterance, or a model-chosen ID cannot approve.
The CLI displays the exact operation arguments, and `/approve ID` creates an
internal queue entry that cannot be represented by tool-call JSON.

Approval is checked again at dequeue. A replacement, rejection, expiry or stop
can invalidate work still queued. The stored payload is executed once; model
arguments are not re-read or amended after review. Returned completion receipts
are shown to the host and sent as context to the provider. Platform account
selection, app permission and actual executor tool permissions remain separate
concerns: the review shows configured-account semantics rather than claiming to
have independently verified the account chosen by Mail or another external app.

The local host control channel is trusted. This is not a sandbox against another
process already authorized to control the user's terminal or desktop. Delegated
agents retain their configured execution permissions; an instruction saying
read-only is not enforcement. This change does not manufacture such a sandbox.

## Queue cancellation and transport lifecycle

Each provider call is registered once. Duplicate IDs and model use of the
reserved `host:` namespace are rejected. Cancellation before execution prevents
the call from beginning. Cancellation during execution records a request and
withholds its result, without pretending the process stopped or external effects
were undone. `/cancel` also rejects pending approval and flushes playback. The
registry retains bounded tombstones so reconnect cannot replay a completed or
cancelled call. This is not yet a durable Herdr/task-service executor or a
cross-process cancellation implementation. Running work remains timeout-bound.

Provider profile selection now reaches the real setup and response builders.
Extended Thinking declarations are NON_BLOCKING, proactive audio stays enabled,
and tool scheduling is omitted. The wire decoder reports interaction status;
utterance completion and drained playback no longer imply all reasoning is
finished. Pending calls participate in readiness. The outbound channel is bounded,
and write batches yield to provider reads. Unknown models fail before opening a
connection rather than silently assuming compatible semantics.

Normal voice meeting reads now exclude restricted records, including exact-path
reads. Unattested derived-insight log access is disabled pending live-source
provenance. This does not claim a complete final-egress re-attestation of every
existing brain, prep, calendar or already-delivered provider-context path.

## Persistence and recovery

Checkpoints are bounded, versioned JSON under `~/.minutes/work-capsules`.
The content digest is the identifier.
The existing policy filesystem capabilities create private staging files and
atomically publish without replacing a saved checkpoint. Reads reject path
traversal, symlinks, changed content and oversized input, and verify the exact
retained file. Linux/macOS use owner-private modes and Windows uses the existing
protected filesystem implementation. Content addressing detects changes; it is
not proof of human approval or immunity to same-user edits. Imported text never
grants authority to act.

## Validation and release boundary

### Spoken reads and reasoning

With `[voice_live] ask_agent = true`, `read_pull_requests` exposes fixed GitHub
repository search, PR list and PR detail reads without terminal approval.
`review_pull_request` fetches current PR metadata and a bounded diff, checks that
the head SHA did not change during retrieval, and sends that evidence to Codex
or Claude using the existing isolated Recall launch contract. It cannot merge,
post or edit, does not inherit configured delegation flags, and reports when
the diff was truncated. The assessment is not a checkout-and-test review.
Broader `ask_agent` delegation still needs host approval and now accepts an
explicit agent choice; flags are not transferred between different agents.

Ordinary conversation continues to use the configured voice model. The
`think_deeply` tool can use `gemini-3.8-live-extended-thinking` for one task using
supplied evidence, without replacing the active voice session. It has no action
tools. `[voice_live] thinking_level` defaults to `medium` and accepts `low`,
`medium` or `high`; standard Live setup omits the unsupported thinking field.
The extended model uses the v1alpha endpoint. `get_status` reports the active
voice model and the availability of on-demand reasoning.

CLI-only macOS builds find the Calendar helper produced by the core build
script even when no desktop bundle is staged. Calendar permission probes must
run in the same responsible application context as the user session: an SSH
probe can have different Calendar access from Terminal.

Regression tests cover actual dispatcher staging/approval, private-history
non-disclosure, offline refusal of network actions, restricted exact-path reads,
checkpoint persistence/tampering, queue cancellation, and real wire JSON shapes.
CI additionally builds the actual CLI and runs an offline create/debrief/park/
list/resume smoke test with fresh local state and no provider credential.
Portable contracts are tested independently from the audio dependency graph.

Compilation is not proof that AX works in every application. Before a release,
the signed Minutes Dev identity must be used to exercise selected-text capture,
open-mic echo cancellation, interrupt/reconnect behavior, exact local review,
normal-to-restricted source changes, and the complete conversational loop. No
Tauri hotkey, overlay, autonomous clicking, always-on monitoring, live provider
benchmark, or production release is introduced by this branch. A future native
surface must preserve these host boundaries rather than recreate them in prompts.

Google contract references verified September 16, 2026:
- https://ai.google.dev/gemini-api/docs/models/gemini-3.8-live-extended-thinking
- https://ai.google.dev/gemini-api/docs/live-api/tools

## Lint scope

The portable harness uses global `-D warnings`. The integrated voice-only
all-targets configuration exposes inherited warnings in unrelated capture,
knowledge, graph and other test modules. Its dedicated workflow therefore runs
Clippy on the actual core and CLI and fails all compiler errors plus every
warning touching changed Rust lines, while retaining other warnings in logs
and an uploaded JSON artifact. Formatting checks every changed Rust file. This
is a changed-code gate, not a claim that whole-workspace Clippy is clean. The
repository-wide and native release gates remain separate.
