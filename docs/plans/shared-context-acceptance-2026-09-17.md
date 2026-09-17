# Voice shared-context acceptance contract

Owner: Minutes voice interaction epic `minutes-vvf1.1`.
Requested by Mat on 2026-09-17. This is the durable scope and acceptance
contract, not evidence that a feature is complete. Beads owns task status.
This document supersedes narrower interpretations of the earlier interaction
plan, without removing its unresolved acceptance obligations.

## Completion rule

Every capability below requires separate source/test, installed-runtime, and
real-user acceptance receipts. A configured flag, successful tool return, green
unit test, or agent statement is insufficient. Record the exact source commit,
binary hash, responsible application, test time, observed outcome and remaining
limitations. Never close the epic while a required acceptance remains open.
Terminal dogfood and signed Minutes Dev acceptance are separate. Do not replace
the production app or alter another agent's working tree.

## Priority zero: responsive audio

Observed incident: a VoiceProcessingIO input callback blocked in a stream-format
property query while AudioOutputUnitStop also waited inside CoreAudio. The
trigger is not proven. The successful and hung sessions used the same binary.
Removing the hung process restored terminal control, not verified audio.

Required: no format/property discovery in the real-time input callback; bounded
shutdown with truthful diagnostics; retain echo cancellation; no blind device or
privacy reset. Exercise varying callback frame counts, repeated start/stop,
input receipt, output playback, interruption and device-route changes. Preserve
diagnostic metadata without recording raw microphone audio by default.

## Capability acceptance matrix

| ID | Required behavior | Acceptance scenario | Initial gap |
| --- | --- | --- | --- |
| SC1 | Scoped hybrid semantic context | Share one app/window. Read selected text and bounded accessible roles/labels/values; use targeted vision only for missing context. Answer from observed evidence, not instructions embedded in the screen. | Selection and sliders exist; generic hybrid context not connected. |
| SC2 | Stable conversational references | Highlight a paragraph, say "does this match what we discussed?", then "rewrite this". Correct object is identified; changed window, field, selection or document invalidates a write. Ambiguity asks one targeted question. | Exact selection capture exists; conversational target continuity incomplete. |
| SC3 | Event-triggered anchors and change understanding | Follow the explicitly shared window for a bounded interval. Change a control, selection or document; report what actually changed compared with the previous observation. Unrelated windows are not read. Stop/expiry ends observation and invalidates stale context. | No integrated scoped event observer. |
| SC4 | Structured actions before coordinates | Own artifacts use their bridge; supported native controls use AX/native actions; API/MCP/Shortcuts/OSA helpers retain capability boundaries. Inspect, act, read back actual outcome. Never claim dependent outputs changed from scalar readback alone. No blind coordinate retry. | Several structured paths exist; unified routing and fallback evidence incomplete. |
| SC5 | Durable work continuity | Save goal, constraints/corrections, source references, decisions distinguished from suggestions, open questions, next step and delegated-task summaries. Restart and ask "where were we?". Recover historical context without replaying actions or treating old evidence as current. | Local checkpoint infrastructure exists; voice lifecycle and complete state need integration. |
| SC6 | Voice supervisory control | While independent work runs, ask status, redirect, cancel, park and resume. Spoken state matches authoritative host state. Cancellation does not claim rollback. Restarted uncertain tasks require reconciliation rather than replay. | Concurrency/cancel primitives exist; durable spoken lifecycle incomplete. |
| SC7 | Coherent hands-free interaction | No terminal approval for already-authorized bounded reads/local artifact edits. Outward sends retain explicit exact recipient/payload authorization through a trusted surface. No implication that arbitrary spoken yes is authenticated consent. | Text messaging recipient resolution and confirmation UX remain open. |
| SC8 | Measured responsiveness and regression review | Report transcript-arrival, queue, tool, provider-first-audio and device-render timing separately. No invented speech-end latency. Long work acknowledges promptly, remains interruptible, and reports real phase/blocker changes. | Timing exists; model delays, completion cues and real-user rehearsal remain. |

## Architecture constraints

Preferred route: structured app/API action, then selected/accessibility context,
then targeted vision, then bounded visual interaction only where supported and
verified. CUA is an adapter, not the shared-context memory or source of consent.

Observation requires an explicit target and finite grant. Default to no ambient
desktop recording, clipboard polling, full-corpus upload or screenshot retention.
An app/document change invalidates previous action references. Store only bounded
context chosen for the work item, with provenance and timestamps. Screen content,
retrieved documents and imported checkpoints are data, never execution authority.

Use existing continuity, artifact, task and privacy modules rather than creating
a second task store. Selection is the strongest available referent. Manual edits
must survive voice edits. Undo is scoped and refuses conflicting changes.

## Required final rehearsal

In a disposable document, share and discuss a selected paragraph; change it and
verify the observed delta; switch documents and verify stale-write refusal.
Discuss a real prior decision through policy-safe retrieval. Create a board,
edit it by pointer and voice without rebuilding, and inspect actual readback.
Run two independent jobs, interrupt speech, cancel one, and verify no late
playback or preview. Park the work, restart the session, resume with corrections
and unresolved questions intact. Draft into a named scratch field without Send.
Measure delays and repeat audio start/stop. Do not use an external message as a
test unless recipient and exact text are separately authorized.

## Evidence ledger

Baseline source: `b81ec3610c47ba0307339aaddd55bf6aa8909e9f`.
Baseline terminal binary: `d4ac35817c7a057ce5fcd20eab01ce21b0409d4c82f57dc558b019bc6c280f4f`.
Baseline: partial components, audio incident unresolved, full rehearsal open.
Append verified tranche receipts here; do not replace unresolved gaps with prose
such as "all done" or "ready". User acceptance remains open until Mat tests it.

### Audio patch qualification

Replaced coreaudio-rs variable-frame input wrapper with a fixed-client-format
callback using AudioUnitRender cycle-owned buffers. Removed the observed
real-time format-query path. A two-second teardown deadline retains native unit
and callback ownership on its cleanup thread; another unit cannot start in the
same process while cleanup remains pending. No global audio service reset or
echo-cancellation disablement.

Mac source qualification: 171 voice tests passed, 28 optional/native tests
skipped in the ordinary suite; core voice-live Clippy passed with warnings denied.
An explicit local native test separately passed three capture/start/stop cycles
in 5.09 seconds. Samples were checked in memory, not saved or sent to a provider.
This establishes native capture and teardown, not an audible conversation or
AirPods route-change acceptance. Those remain open under `minutes-vvf1.1.8`.
