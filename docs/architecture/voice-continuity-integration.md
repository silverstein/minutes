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

## Small HTML prototypes

The Live voice can be selected with `[voice_live] voice_name = "Charon"` (or
another supported Gemini voice). Empty preserves the provider default. Optional
`persona = "morris"` adds a dry, helpful chief-of-staff tone without changing tool,
privacy or approval rules. Both settings take effect in a new session; a resumed
socket retains the selected voice. Other users keep the existing tone by default.
Morris gives useful facts first; humor must not invent urgency or contradict the
answer. Voice selection changes the sound, independently of the persona.

`[voice_live] html_prototypes = true` opts into a bounded discuss/build/revise
loop. It is off by default and also requires voice cloud consent. Gemini Live
distills an explicit build request into a brief; the configured coding agent
(or explicitly requested Codex/Claude) returns text-only HTML and a title.
The agent uses the isolated Recall launch contract, not broad `ask_agent`
permissions or configured permission-bypass flags. Merely discussing an idea
must not start a build. This is intent routing, not speaker identification.

New prototypes without a requested visual style default to a restrained lo-fi
cyberpunk developer-tool direction: charcoal, readable monospace, crisp borders,
pixel details and selective bright accents. An explicit user style overrides
that default. Revisions preserve their existing look unless a change is requested.
This is generation guidance, not a deterministic theme applied to arbitrary HTML.

Minutes saves immutable versions under `~/.minutes/prototypes/` and opens a
host-authored local viewer. Generated code runs in an opaque-origin iframe;
inline scripts are allowed, but network, storage, popups, downloads, form
submission and access to the surrounding document are not granted. A restrictive
CSP blocks resource loads and frame navigation. This is an offline prototype,
not a deployed app or permission to modify a repository. The configured agent
service receives the brief and, on revision, the immediate prior HTML version.

For a revision, the model must use the exact returned `prototype_id`; paths,
changed records and symlinked storage are refused. Previous versions remain.
This opted-in generate/save/open operation does not require terminal approval.
Broader agent actions retain their existing host-review boundary. Cancellation
uses the existing queue semantics; an already running build is timeout-bound
and may save/open a preview even if its result is withheld after cancellation.

A short rehearsal is: discuss a small interface, explicitly ask to build it,
inspect its controls, then request one concrete revision. The assistant must
wait for the receipt before claiming it opened anything and must not claim the
generated app was tested. Agent generation, synthetic Live routing, and browser
checks are separate evidence from a successful microphone rehearsal.

## Host review is an execution boundary

Operations outside explicitly opted-in, constrained capabilities stage a local
review, including broad delegated-agent execution and MCP calls. Disabled
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

Shared-corpus reads and desktop actions stay on one serialized worker. HTML
prototypes and music each have a separate worker with four pending slots and
one running call, so a long build does not delay requested hold music or status
reads. Two builds (or two music renders) do not overlap. Host-approved actions
and selection capture always use the serialized lane. All lanes share the same
duplicate, cancellation and shutdown registry. Generation completions cannot
clear or re-display an unrelated host-review proposal.

With music explicitly enabled, requested `make_music` calls generate and play
directly without an additional terminal approval. The recording exclusion and
post-generation recording check remain. The assistant must describe a pending
call as generating, never as already drafted or playing.
The prompt requires a brief spoken acknowledgment for each new generation
request before calling its tool, including music requested while a build is
pending. This is model behavior, not a deterministic host-generated audio cue.
Unstyled hold-music requests default to dry, snarky lyrics over an upbeat,
repetitive synth/lounge loop. Explicit style, instrumental, calm or no-jokes
requests override that default; other music requests do not inherit it.

Generated songs have a separate sample queue in both audio backends. Assistant
speech takes priority; music resumes after a 250 ms speech tail. `control_music`
routes directly to this session's generated-song queue before desktop dispatch:
pause retains its position, play resumes, and stop discards the remaining audio
without deleting the saved song or cutting off speech. These local controls do
not wait behind corpus work and remain declared with music enabled even when
desktop automation is disabled. Playlist skipping is not supported for generated
songs. `/cancel` stops generated playback as well as cancelling queued calls.
The Music/Spotify fallback compiles only a fixed command for its resolved target
application, avoiding dictionary-specific verbs in a dynamic AppleScript tell.

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

`research_public` answers public background and current-topic questions through
Gemini 3.8 Flash with Google Search grounding, using the existing cloud consent
and API key. It never launches a local agent or exposes filesystem, shell or
action tools. It runs without host approval, caps the question and response,
times out after 45 seconds, and requires a completed answer with web sources.
The voice assistant sends only the public lookup, then combines returned facts
with private conversational context in the ongoing voice session. It
must not send private transcripts or calendar contents as search context.
Returned source links preserve attribution; failure does not
fall back to broad delegation. General explanations need no delegation at all.

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
- https://ai.google.dev/gemini-api/docs/generate-content/google-search

## Voice delegation context

Voice Live maintains a process-local working context as user transcription,
assistant text and delivered read-tool evidence arrive. It also remembers text
the host explicitly shares through `/work share` or selected-text sharing. It
does not read private work history, search the archive speculatively, run a model
or launch an agent in the background. Raw audio and images are not copied into
the context. Voice transcripts remain hypotheses; no identity/text rewriting
or automatic interpretation turns speech into an accepted decision.

When `ask_agent` arrives, the session freezes a versioned snapshot before queueing
the call. The existing local review includes that snapshot in `minutes_context`.
Dispatch uses those exact reviewed bytes, even if the user continues talking
while the job is queued or awaiting review. Corrections to a proposed task need
a new proposal and review. A model cannot provide `minutes_context` or authorize
dispatch. This adds no new approval step to other Voice Live tools.

An `ask_agent` proposal is an `AwaitingApproval` receipt, not an agent answer.
Voice instructions distinguish waiting for local review from dispatched work
and ask the voice model to wait for the requested agent rather than substitute
its own answer. Unrelated conversation remains available while the agent runs.
After approved execution returns, the host sends a new conversational turn with
an explicit completion caption and a separate JSON receipt. The caption asks for
the substantive returned answer with agent attribution, or the actual error,
rather than a generic follow-up. Returned text stays untrusted result data and
cannot grant permission for another action. Explicitly shared selections use
their own caption and do not acquire action or agent provenance.

Completion still waits for the configured agent process to exit successfully.
An earlier stdout message or streamed agent-turn event is not a successful
process exit. This path does not disable lifecycle hooks, bypass process cleanup
or keep an agent connection warm to reduce latency.

The snapshot retains at most 16 entries, at most 1,024 UTF-8 bytes per entry,
and at most 8,192 bytes for its JSON envelope. Older material is shed first;
truncation is explicit. Tool evidence carries its tool/call provenance and is
historical, not proof a source is still current or authorized. Drafts and
interrupted/generated assistant text are labeled separately from user input.
Only successful, delivered public-research and source-attested `get_meeting`
results are eligible. Meeting reads are compared with current authorized bytes
when cached, and their canonical path and full-content hash are checked again
immediately before agent dispatch. A changed, deleted or newly restricted source
refuses the handoff and requires a new read and review. Aggregate searches and
knowledge/graph results are omitted until complete source attestations exist;
failed, cancelled, undelivered, arbitrary MCP and action results are excluded.
Text explicitly shared by the host is separately labeled. Speaker-playback
transcription without echo cancellation invalidates the cache instead of being
accepted as user instructions. Contention never waits on the voice producer;
missed updates invalidate older context and mark the next snapshot incomplete.
Stopping clears the context, and a new session gets a distinct epoch.

Context preparation is local and requires no OpenAI API key or API credit.
Delegation uses the configured local agent and its existing authentication/billing.
The Gemini Live voice connection is unchanged. Context preparation alone does
not eliminate agent startup or establish a measured latency improvement.
`delegation_context_prepared` logs only revision,
epoch, byte count, completeness and preparation duration, never its contents.
Synthetic tests exercise correction/interrupt behavior and the actual review,
dispatch and subprocess path using an offline echo executable. Native voice
acceptance and live latency measurements remain separate release checks.

### Opt-in Codex app-server transport

The default `delegate_transport = "cli"` still waits for the existing per-call
CLI process to exit. On Unix, `[voice_live] delegate_transport = "codex_app_server"`
instead owns one `codex app-server --stdio` process for this voice session. It
starts only at an approved handoff; there is no speculative prompt or daemon.
Existing Codex authentication is used, including a ChatGPT login when configured.
This does not supply credits or change provider billing.

This route requires Codex and `delegate_writes = false`. Each approved question
starts a fresh read-only thread with approval policy `never`; the response must
confirm both before a turn is sent. Effective MCP servers and plugins are
individually disabled in thread-local overrides, along with apps, computer use,
multi-agent delegation, web search and Codex memory use/generation. Global Codex configuration and hooks are
unchanged. The regular CLI route remains available for other capabilities.
Launch arguments accept only paired `-c`/`--config`, `--enable` and `--disable`
options; unsupported configurations fail explicitly without falling back.

The worker serializes approved calls with cancellable, deadline-bounded waits,
and rechecks meeting-source authorization after startup/queueing, immediately
before `turn/start`. Only the matching thread and turn's final agent message is
eligible. A successful `turn/completed` and acknowledged `thread/archive` are
required before delivering the answer. No old thread is resumed or reused;
archived records follow Codex's own local history policy. The server may reuse
its provider connection, but prior question transcripts are never submitted as
history for another question.

Input, frame size, queued output, per-call output and answer size are bounded.
Malformed output, authorization requests, failed turns, archive errors, timeout
and cancellation discard the owned server and withhold the answer. There is no
automatic retry of a possibly executed question. Stop closes stdin for graceful
shutdown, then kills and reaps the owned process group after a bounded grace
period. Idle cleanup runs off the capture thread; in-flight workers observe
session stop. Windows retains the CLI route until equivalent descendant cleanup
is qualified. Transport qualification and paired end-to-end latency remain
separate from context-correctness and native voice-output acceptance.

## Lint scope

The portable harness uses global `-D warnings`. The integrated voice-only
all-targets configuration exposes inherited warnings in unrelated capture,
knowledge, graph and other test modules. Its dedicated workflow therefore runs
Clippy on the actual core and CLI and fails all compiler errors plus every
warning touching changed Rust lines, while retaining other warnings in logs
and an uploaded JSON artifact. Formatting checks every changed Rust file. This
is a changed-code gate, not a claim that whole-workspace Clippy is clean. The
repository-wide and native release gates remain separate.
