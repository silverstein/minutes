# Work continuity: portable core, native interaction experiments

Status: additive library foundation. Not wired into Voice Live, MCP, Tauri,
Shortcuts, accessibility capture, Herdr, or a persistent store by this change.
No new recording behavior, native actions, or permissions are enabled.

## Product and platform boundary

Prototype the richest interaction on macOS without defining the work itself in
macOS terms. A work record contains goals, versioned sources/selections,
attributed memory, unresolved questions, next steps, and delegated-task outcomes.
It does not contain AppleScript, an AX object, a Gemini connection, or a window
handle that another OS must emulate.

The core lives in `minutes_core::live_sidekick::work`. Existing
`LiveAssistanceSession` continues to own live-capture state. `WorkSession` owns
the longer-lived work context. Neither replaces recording or the canonical
meeting files. Voice remains an optional failure-isolated consumer.

Native adapters can launch first on macOS. Existing capture, local memory,
CLI/MCP contracts, and permission safety must not regress on Windows or Linux.
Unsupported native capabilities must be reported as unsupported, never as
successful no-ops. Do not build a speculative universal desktop-automation
framework before a repeated interaction demonstrates its value.

## Implemented behavior

`Focus` binds a selection to an immutable source snapshot. Reusing a source id
with different version metadata is rejected; a new snapshot gets a new id.
This makes referents stable without pretending the source was freshly read.

`WorkCheckpoint` encodes bounded versioned JSON inside a Markdown file and
validates on decode. `park` updates questions and the next step, revoking pending
actions. `resume` restores context, creates a fresh runtime identity, and marks
queued/running work interrupted. It cannot restart a subprocess or recover a
previous approval. The codec performs no filesystem operations.

Memory entry points distinguish model inference, user interpretation, and
human-confirmed decisions. Imported labels are untrusted historical assertions,
not proof that a person approved something. Source records require provenance.
Do not promote an interpretation into a statement attributed to a meeting.

A task has a stable id, generation, instruction, state, full result-artifact
reference, and a separate spoken summary. A private runtime lease binds worker
completion to one session instance and generation. Queued cancellation prevents
start. Running cancellation remains requested until the supervisor acknowledges
termination. Work completed before cancellation took effect is explicitly
recorded as `CompletedAfterCancelRequest`, not falsely labelled cancelled.

The approval flow is propose, preview, trusted-host approve, consume. A proposal
id is not authority. The non-serializable permit binds the exact payload,
target, runtime instance, work revision, and expiry. Corrections, rejection,
parking, policy changes, restart, and replay cannot reuse the approval.

Cloud release uses a separate host-approved destination/source/version/policy
permit. The entire contributing source set includes task result artifacts.
Unknown policy is denied. Restricted information is not implied to be releasable
by local ownership or a global cloud-session opt-in.

## Mandatory host-adapter obligations

Host-only approval and disclosure methods must never appear in model tool
registries. These Rust capabilities are an in-process boundary, not a sandbox
against code with the user's filesystem/process privileges. Keep authorization
out of transcript interpretation and model-provided JSON. Use a monotonic
host clock for permit timestamps; do not accept time values from a model.

Capture selection and source context before an overlay takes focus. Read the
actual source bytes and policy through the existing authorized-source boundary;
metadata returned by a model or imported checkpoint is not attestation. Recheck
version, target, and authority immediately before an external action.

A consumed approval is not an execution receipt. Executors must separately
prevent retries from producing duplicate external actions, record the actual
outcome, enforce an allowed verb catalogue, and apply their real sandbox/tool
permissions. Serialize dispatch against revocation; do not queue an authorized
payload indefinitely after consuming its permit. Native, MCP, Shortcuts, and
agent adapters must obey the same rule rather than route around it.

A task cancellation state is not a process-kill implementation. The supervisor
must stop the actual worker/process group, enforce deadlines, reap it, and then
acknowledge. On app restart reconcile any detached/orphan work before offering
an explicit retry. Stopping speech must not silently imply task cancellation.

Persist checkpoints only through private, capability-bound storage with source
sensitivity and transitive provenance. The entire checkpoint may contain
sensitive text; do not add it wholesale to prompts, debug logs, or network
requests. Re-attest all contributing sources at egress and bind the exact
released bytes to that attestation. This library does not traverse provenance
for the host or invent permission for a summary with missing ancestors.

## Live-model adapter contract

`live_sidekick::live_model` supplies pure helpers for known model ids. Standard
Live responses can schedule result delivery. Extended Thinking responses omit
that unsupported field, and its function declarations are non-blocking. A
`turnComplete` message alone does not make Extended Thinking idle. Top-level,
nested, and status-only interaction events are accepted; unknown or conflicting
statuses remain unknown rather than falsely ready.

Model activity, audio playback, outstanding app tasks, and pending approval are
independent states. Disconnecting a provider does not complete app-owned tasks.
The existing Voice Live wire reader/dispatcher must actually call these helpers
before this logic changes runtime behavior. Verify real provider transcripts,
reconnection, and cancellation on the active voice branch before claiming
model integration is complete.

Official model contract consulted September 16, 2026:
https://ai.google.dev/gemini-api/docs/models/gemini-3.8-live-extended-thinking
https://ai.google.dev/gemini-api/docs/live-api/thinking

## Executable example and verification

From a complete checkout with the pinned Rust toolchain:

```sh
python scripts/check_work_continuity.py
```

The supplementary harness compiles the exact new modules in a temporary minimal
crate without audio/GUI SDK dependencies. It runs 39 tests, Clippy, formatting,
and the synthetic checkpoint example through two processes. The GitHub workflow
runs the same harness on Linux, macOS, and Windows. It does not replace full
repository CI, the canonical core tests, or native UI testing.

The example is also a normal `minutes-core` example:

```sh
cargo run -p minutes-core --no-default-features --example work_session > checkpoint.md
cargo run -p minutes-core --no-default-features --example work_session -- resume < checkpoint.md
```

This contains synthetic data only. It does not capture a screen, run an agent,
open an app, or send a message. Full-core example builds still require that
platform's normal Minutes build dependencies.

## Integration scope still outside this library

The end-to-end point-and-talk experience requires native context acquisition,
visible evidence and proposed-change UI, trusted approval controls, private
checkpoint storage, and voice/CLI/MCP entry points. Persistent real agent work
requires the supervisor and its output receipts. Debrief capture needs a user
flow that records interpretation separately from the meeting. AX, approved
Shortcuts, and Herdr adapters remain follow-on integrations, not features
implemented by exporting these types.

Evaluate one continuous workflow first: select an artifact, compare it with an
authorized source, correct the interpretation, propose a bounded change, park
without sending, and resume later. Compare models on correction persistence,
repeated context, source accuracy, cancellation outcomes, and time to useful
work, not on filler-speech latency or the number of available desktop verbs.
