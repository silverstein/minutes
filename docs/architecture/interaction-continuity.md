# Mac-first interaction experiments, portable continuity

Status: experimental library and offline lab. This is not a shipped desktop
assistant or a completed Voice Live integration.

## Scope and ownership

`minutes_core::interaction` implements small, surface-neutral contracts for
shared attention, work capsules, debrief provenance, delegated task lifecycle,
local approval, final-egress source attestation and Live model capabilities.
The library has no filesystem, process, audio, desktop or network effects. It
uses only std, serde and serde_json. The existing capture pipeline and recording
mutual exclusion are unchanged. No new cloud opt-in or desktop permissions are
silently granted.

This is deliberately separate from the active `feat/voice-live` lane. It does
not edit that lane's AppleScript, transport, session runner or overlay. The
existing `live_sidekick::session` remains the live-assistance reducer; these
primitives do not replace it or become another owner of recording. Adapters
should use its existing evidence identities and source/user generations.

The product policy is **Mac-first discovery, cross-platform contracts**. Native
context capture, echo cancellation, AX selection, Shortcuts and OSA execution
belong at the edge. Goals, decisions, evidence provenance, approval semantics,
task state and serialization must not depend on a Mac bundle ID, Apple event,
shell command, Gemini connection or Herdr pane.

## Shared attention

A trusted hotkey or UI action starts an `AttentionSession` for one exact target
before an overlay takes focus. A capture request binds the application, window,
optional artifact, generation, request sequence and time. A provider must report
its actual observed target, not blindly echo the requested target. The host
accepts only the newest matching frame/selection, within the active grant and
freshness budget. Stop, expiry and a new request invalidate stale work. All
runtime times use one host monotonic clock; they are not persisted wall clocks.

`ContextProvider` exposes Available, PermissionRequired or Unsupported. Missing
support must not trigger broader capture or fabricated success. The initial
adapter can be Mac-specific; no platform adapter is implemented by this change.
Continuous monitoring, clipboard polling and capture-on-every-window-switch are
not enabled. A selected image reference is data, not authority to read a path.

## Approval and release

`ApprovalGate` stores one complete action for local review, including account,
recipient/target and exact payload. The model may know the proposal ID. It may
not call `approve_from_host`. Only a trusted UI/CLI gesture with the exact view
reviewed by the human can do so. Another utterance, an input transcript or a
model-provided confirmation token confers no authority.

Execution moves the stored approved payload out once; it does not execute
resubmitted model arguments. Replacement, rejection, expiry and policy changes
invalidate approval. The host still must check operation allowlists, platform
permissions, recipient resolution, sandbox boundaries and source freshness at
its final dispatch boundary. Approval is not an execution receipt; an uncertain
network send must be reconciled, not retried automatically. This library does
not yet replace the active voice branch's confirmation mechanism.

`validate_cloud_release` requires explicit cloud opt-in and matching current
attestations for every contributing source. Changed bytes, changed policy,
restricted/unknown sources, missing sources and duplicates fail closed. There
is no restricted override here. The host must obtain those attestations from
existing policy-safe readers immediately before egress, not from the model or
an old index. This helper supplements, not replaces, `AgentSafeContext` and the
existing capability-bound file access. Capsule text and debriefs also need their
own source/privacy classification; calling this helper only on meeting links
is not sufficient to authorize an entire generated capsule.

## Work capsules and debriefs

A `WorkCapsule` stores an objective, constraints, source-backed observations,
private user interpretations, suggestions, accepted decisions and a next step.
Updates require the expected revision. A model suggestion remains a suggestion
until a host acceptance creates a separate decision record linked to it. The
original observation and transcript are not rewritten by a debrief.

JSON snapshots have a version and bounds; Markdown exports embed the snapshot.
The offline lab can create, debrief and resume these artifacts. Importing one
never grants execution authority. A claim in an editable file is not proof of
approval, and every source must be revalidated before cloud use. Existing
Minutes storage should own persistence; this change does not create a competing
vault, automatic background writer or new source-of-truth database.

## Delegated tasks

`Task` represents an explicit workspace, instruction and allowed-tool set.
A real adapter must enforce the allowlist; a prompt saying 'read only' is not
an access control. `start_from_host` issues a generation-bound run identity but
does not spawn a process. Steering requests cancellation first. Only a confirmed
stop permits dispatch of revised work. Old-generation completions are rejected.

Cancellation requested is not cancellation completed. A failed cancellation or
restored in-flight task enters NeedsReconciliation. Restarting a socket or app
never replays uncertain outward work. The host must reconcile that state with
its durable worker/task service before deciding what to do next. This change
does not implement a Herdr adapter, process cancellation or a task scheduler.

## Gemini Live adapter contract

`LiveProfile` recognizes explicit standard and Extended Thinking model IDs.
Extended Thinking declarations use NON_BLOCKING; responses omit scheduling;
proactive audio remains enabled. Unknown model names need an explicit profile.
`interaction_status` parses the documented camelCase and snake_case variants;
unknown status is not idle. `LiveActivity` separates playback, input, provider
activity and pending tools. Stopping playback never cancels a task. For Extended
Thinking, turn completion never establishes idle; explicit provider status does.

These helpers are implemented and covered by contract tests. The active
`voice_live::protocol` and `voice_live::session` must call them before this becomes
a runtime fix. In particular, declarations and tool responses need to use the
same model profile after reconnect, the runner must continue receiving after
turnComplete, and UI readiness must incorporate pending tools and playback.

Provider references checked September 15, 2026:

- https://ai.google.dev/gemini-api/docs/models/gemini-3.8-live-extended-thinking
- https://ai.google.dev/gemini-api/docs/live-api/tools

## Offline lab and validation

The lightweight harness compiles the exact production module files without the
audio, native desktop and full workspace dependency graph:

```sh
cargo test --manifest-path tooling/interaction-contracts/Cargo.toml
cargo clippy --manifest-path tooling/interaction-contracts/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path tooling/interaction-contracts/Cargo.toml -- --check
```

The lab has no external effects or credentials. Shell redirection below is
explicit local artifact storage, not an automatic file writer:

```sh
cargo run --quiet --manifest-path tooling/interaction-contracts/Cargo.toml -- new offer-review 'Simplify the offer' > capsule.json
cargo run --quiet --manifest-path tooling/interaction-contracts/Cargo.toml -- debrief 'I was interested but did not agree to more scope' < capsule.json > capsule-updated.json
cargo run --quiet --manifest-path tooling/interaction-contracts/Cargo.toml -- resume < capsule-updated.json
```

The dedicated workflow runs the harness on Linux, macOS and Windows. It does not
establish that native audio, accessibility, app permissions, screen capture or
overlays work on any of those platforms. Those require native app tests, with
`~/Applications/Minutes Dev.app` on macOS. Rust tests and UI verification remain
separate gates. No release or changes to the installed desktop app are made.

The implementation environment did not have Rust, a Mac or the local-only
beads store. Compilation, linting and runtime claims must therefore come from
CI or a local checkout, not from the existence of these tests. The PR remains a
draft until those results and the integration review are available.
