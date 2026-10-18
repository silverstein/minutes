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

Audio source: `f98c85bf2b84e970b19ac4df0fbee689df08f870`.
Installed terminal binary SHA256:
`8fa3a107201ca5923db995baa649055ca5f8acb9b0da732ee2472abca946de6c`.
A second native three-cycle capture/start/stop run passed in 5.34 seconds.
The terminal wrapper still launches this audio-only build. The production and
Minutes Dev bundles, selected devices and isolated voice configuration were not
changed by this repair. Full audible conversation acceptance remains open.

### Shared-context source tranche, 2026-09-17

Historical source-qualification snapshot, before activation below.
Source: `5b5cb515e45a27a652943edc4893a8e8218e068c`, branch
`codex/voice-operate-20260917`. These additions are source-qualified, not yet
installed in the terminal binary above and not accepted on the native desktop.

| Scope | Implemented and source-tested | Still required |
| --- | --- | --- |
| SC1, SC3 | Explicit frontmost app/window grant, 1-300 seconds; bounded AX selection/roles/labels/scalar values; notification-triggered recent deltas; exact single-window image on request; no desktop-image fallback. Stop is non-blocking and revokes in-flight release. | Run the disposable native fixture; qualify a real browser's AX tree and targeted image; verify permissions under the actual responsible application. |
| SC2 | Sharing lifecycle invalidates old selection-write references. Existing field/value/range binding remains required for writes. Prompt prefers selection over guessed screen referents. | Native selected-paragraph conversation, target switch and stale-write refusal in the actual supported editor. |
| SC4 | Existing first-party artifact controls and bounded native controls remain the preferred action route. Observations are expressly not write authority; fresh control/selection references are required. | Combined native inspect/act/readback rehearsal. Generic external DOM attachment, arbitrary coordinate clicking and custom/canvas controls are not implemented by this tranche. |
| SC5 | Opt-in `voice_live.work_memory`; start/remember/show/park/list/resume in the existing private checkpoint store's `voice-shared` namespace. Revision checks; corrections, constraints, reported decisions, suggestions, source references and open questions retain provenance. Resume refuses to replace an active goal. Private host `/work` notes remain separate. | Enable only in the isolated voice profile after runtime qualification. Spoken save, restart and resume with correct goal/corrections and no action replay. |
| SC6 | Prototype redirect stores a revised brief, cancels the exact old job and allows one replacement only after settlement on the same generation lane. Global cancel discards pending redirects. Park stores host task history without claiming to stop jobs. | Real prototype redirect/cancel and spoken park/resume rehearsal, including late-result suppression and clear completion speech. Broad agent jobs and outward actions cannot use this redirect path. |
| SC7 | Existing read/local-edit opt-ins and exact outward-action authority remain intact. | Recipient resolution and trusted outward-confirmation UX are still open. No blanket spoken-yes bypass added and no external message sent as a test. |
| SC8 | Existing timestamped tool/provider/render telemetry retained; new five-second microphone-stall diagnostic distinguishes audio starvation from model latency. Scope start/end is shown from host state without blocking the audio loop. | Measure a real conversation on the new installed build. Separate transcript, queue, tool, first-audio and actual callback-render timings. No latency improvement claim from unit tests. |

Verification on the daily Mac with the pinned Rust toolchain and two build jobs:

- Voice suite: 178 passed, zero failed, 29 ignored optional/live/native tests.
- Configuration suite: 66 passed, zero failed.
- Voice library Clippy with warnings denied: passed.
- Repository formatting check, whitespace check and generated release statistics:
  passed. Three existing no-default-feature test-only dead-code warnings in
  `knowledge.rs` remain; the library Clippy gate is clean.
- Disposable Swift fixture compiled, but was not launched because the foreground
  test window had not yet been approved. No personal document was focused/read
  and no existing draft was changed by this qualification.

Native fixture and reproducible instructions:
`tooling/voice-evals/shared-context-native.md`.

Known limits: context is at most 24 accessible nodes, two recent change entries,
and a bounded selection. Unsupported events may miss intermediate changes.
Document switches are detected through frontmost window identity, title and
exposed AXDocument; same-title hidden browser navigation is not universally
detectable. An unknown/ambiguous window fails closed for targeted capture.
Saved work is explicitly requested history, not automatic conversation recording.
The checkpoint listing budget is 128 revisions; a known checkpoint can still be
resumed directly. No prior task is automatically restarted.

### Native qualification and terminal activation, 2026-09-17 20:14 UTC

The new tranche is now installed in the existing terminal dogfood worktree.
Source at build: `70b69df3` (feature code `5b5cb515`). Installed CLI SHA256:
`763a55c119416c077aba209d15ded0cd44dd52ce1ff9be5afe092ef70eae68be`.
The wrapper still uses the isolated voice profile. Only `voice_live.work_memory`
was added, set to true; parsed TOML equality verified all other settings unchanged.
The original profile was backed up, replacement was guarded by its original
SHA256, and permissions remain 0600. Zephyr, Morris and Extended Thinking low
were confirmed by actual session startup. No production or Dev app was replaced.

Native test executable SHA256:
`2167321ab06845d7dc25c85c73f036ca34fa341754f9c658b9e555f4ef5a69bc`.
This executable was built from publication commit `1beb3f04`, whose voice source
was verified identical to the source-qualified tranche.

- Shared-context test passed in 19.48 seconds from a new Apple Terminal window:
  exact fixture selection, one-window PNG capture, AX notification delta from
  Revenue 50 to 150, document-title change revocation, and stop refusal.
- An initial SSH-launched attempt correctly refused Screen Recording. AX context
  succeeded there. Launching through the actual Terminal permission identity
  passed without any TCC toggle or reset. This is terminal evidence, not signed
  Minutes Dev application acceptance.
- Disposable TextEdit test passed in 0.37 seconds: real selected-text replacement,
  exact content readback, missing/invented/stale reference refusal and caret
  insertion. Both successful receipts had `submitted: false`. No clipboard path
  was needed; no Send/Return/Submit was performed. Browser/editor diversity and
  focus/document-switch write refusal still require broader qualification.
- Two real Live sessions exercised the installed CLI with `--ptt --mute --json`.
  The first created a synthetic goal, saved a budget correction (42, not 24), an
  open maintenance question, and the next step, then parked successfully. After
  process shutdown, a fresh session used list/resume and recovered all details.
  No terminal approval was requested and no jobs were restarted. Both sessions
  exited normally. These were typed requests, not acoustic voice acceptance.
- Checkpoint operations took 12-36 ms; the restart's list/resume calls had zero
  measured queue delay. Received acknowledgement audio began 881 ms after that
  typed request. The run was muted, so this is provider-receipt timing, not audible
  latency or proof of microphone end-of-speech responsiveness.

Private local receipts: `voice-sessions/2026-09-17-16-09-52.md` and
`voice-sessions/2026-09-17-16-11-31.md` under the Minutes data directory;
`/tmp/minutes-shared-context-terminal-20260917.log` and
`/tmp/minutes-selection-terminal-20260917.log`. Synthetic checkpoint revisions
are archived outside the active voice-work namespace so they cannot be mistaken
for Mat's real work. No raw microphone audio or captured screenshot was retained.

## Remaining acceptance sequence

Beads remains the work-status authority. This sequence defines proof obligations,
not a second checklist whose rows can be declared complete from code presence.

1. `minutes-vvf1.1.8`: Mat verifies the installed audio repair in a conversation;
   qualify audible playback, interruption and device-route changes separately.
2. `minutes-vvf1.1.9`: disposable native context fixture passed and the tranche
   is enabled. Still qualify conversational reference use in a real browser and
   the separately signed desktop application; do not generalize this fixture.
3. `minutes-vvf1.1.3` and `.4`: qualify exact selection, document/focus changes,
   stale-write refusal and structured edits with dependent output readback.
4. `minutes-vvf1.1.5`: CLI installed and memory enabled; typed Live checkpoint
   restart/resume passed. Spoken rehearsal and real prototype redirection remain.
5. `minutes-vvf1.1.6`: native TextEdit no-Send fixture passed; complete the actual
   browser scratch-draft and combined voice rehearsal.
   Outward recipient resolution and confirmation remain separate acceptance.
6. `minutes-vvf1.1.7`: review timestamped conversation evidence, report actual
   slow stages and regressions, and run the combined final rehearsal above.
7. Reconcile the publication branch and exact-SHA CI before landing. This source
   tranche is not a main-branch merge, a desktop release or user acceptance.

Do not mark the epic complete while any required behavior above lacks a receipt.

### Default-branch landing qualification, 2026-09-17

The repository's verified default branch is `main`, not `master`. Config
preservation PR #1023 merged there as `cca94d9290ad90447d5000de2e1f652e1735693f`.
The publication branch incorporates that change; the only merge conflict was
the generated site test count, regenerated from the combined source (3295).
Combined Mac qualification passed 66 configuration tests, 178 voice tests and
the changed-file formatting check (42 Rust files). The 29 optional/live/native
tests remain excluded from the ordinary voice suite; explicit native receipts
above are separate.

PR #1022 is the source landing vehicle for the audio repair, bounded shared
context, editable artifacts, cancellable jobs and durable voice work. Merging
that implementation does not close the acceptance epic or retrofit either
installed desktop bundle. The existing terminal build remains available for
testing without waiting on remote CI. Browser diversity, acoustic playback and
route changes, real prototype redirect, outward confirmation, signed desktop
qualification and the combined hands-free rehearsal remain open as listed above.

### Large-artifact receipt correction

Pre-merge review reproduced raw JSON truncation for four long board cards and
a 64-option dropdown: required revision/snapshot references could disappear.
The correction budgets structured responses instead of clipping JSON. Board
reads expose card pagination and exact-card detail; prototype inspection pages
controls and options before transport without changing its full-state snapshot.
Abbreviated content is labelled, while IDs, revisions, values, mutation receipts
and undo tokens remain exact. A too-small budget returns a truthful metadata-only
receipt rather than encouraging a repeated mutation.

Mac source verification: 183 voice tests pass, 29 optional/native tests ignored;
Clippy with warnings denied passes. New cases cover large boards, complete card
pagination, Unicode/escaping, long dropdowns, small budgets and malformed receipt
shapes. The actual JavaScript bridge passes isolated inspection/paging/set/undo
tests, now invoked by the CI continuity self-test. This is source/fixture proof,
not a replacement for the pending real-browser and spoken rehearsal.

Follow-up review also reproduced a Unicode-heavy mutation receipt exceeding the
preview host's byte limit. The host now budgets the UTF-8 envelope, retaining
snapshot/undo/change metadata and paging controls instead of silently dropping
the result. A final oversized-identity fallback explicitly reports an unverified
receipt, not a failed mutation that is safe to repeat. Both actual bridge and
host scripts pass a synthetic Unicode mutation test that exceeded the previous
34,000-byte boundary, plus the inspection/paging/set/undo test. These lightweight
portable tests ran with a 64 MiB Node heap cap while the daily Mac was temporarily
unreachable. This follow-up still requires CI and installation on that Mac; no
native or acoustic acceptance is claimed by the portable test.
