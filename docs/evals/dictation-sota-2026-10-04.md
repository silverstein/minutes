# Dictation experience against the current product bar

Evaluated October 4, 2026. Candidate: `feat/dictation-experience`, starting at
`432ce97aa8d44b0a5ce60e886106b4b12fbd4eae`. This second assessment follows the
first implementation and signed native acceptance round. The dated sections below retain the evidence available at each checkpoint.
The October 6 section records the latest signed native qualification; the
October 4 score remains historical.

## Result and method

Minutes' independently reviewed UX score was **30/40**, up from **25/40**
before the initial October 4 refinement. That score predates the later capture
feedback and readiness work; those additions have not received a new independent
score.
This is an independent heuristic review of source and browser fixtures using
the same ten criteria and four-point scale. It is not a comparative speech
benchmark, a user satisfaction score, or evidence that Minutes matches Wispr's
recognition quality. Competitor capabilities below come from current official
documentation; competing apps were not installed or tested side by side.

| Criterion | Before | After |
|---|---:|---:|
| System status | 3 | 3 |
| Familiar language | 3 | 3 |
| User control | 2 | 3 |
| Consistency | 2 | 3 |
| Error prevention | 3 | 3 |
| Recognition over recall | 2 | 3 |
| Efficiency | 2 | 3 |
| Visual clarity | 3 | 3 |
| Error recovery | 3 | 3 |
| Help in context | 2 | 3 |
| **Total** | **25** | **30** |

No criterion earns four merely for improving. Setup, destination handoff and
final native acceptance remain material limits.

## Current comparison

| Product | Documented bar | Implication for Minutes |
|---|---|---|
| Wispr Flow | Own-key gesture hints, explicit completion controls, microphone selection, movable capture bar, history audio playback, rich-text snippets and selected-text Command Mode. | Make everyday capture and correction effortless. Minutes' new controls and recovery review narrow the UX gap; richer history, spoken editing and capture-bar flexibility remain opportunities. |
| Superwhisper | Searchable history with audio, source transcript and processed text; retry in another mode; configurable models and app-activated modes. | Recovery should explain what was heard and let users retry deliberately. The October5 offline candidate searches all100 retained entries and reviews retained failure audio; full-library retention and native audio acceptance remain distinct limits. |
| Typeless | Advertises self-correction, personal vocabulary, app-sensitive style and selected-text spoken editing. | Test intended meaning, names, numbers and negation before considering automatic semantic editing. Its vendor claims do not establish a safe rewrite policy for Minutes. |
| VoiceInk | Local transcription and optional cloud enhancement in an open-source Mac app. | Local-first is a competitive foundation, but does not alone establish speed, accuracy or ease of use. |

Sources: [Wispr release notes](https://wisprflow.ai/whats-new),
[Command Mode](https://docs.wisprflow.ai/articles/4816967992-how-to-use-command-mode),
[Superwhisper history](https://superwhisper.com/docs/get-started/history),
[modes](https://superwhisper.com/docs/modes/modes),
[Typeless](https://www.typeless.com/), and [VoiceInk](https://tryvoiceink.com/).

Wispr's September 17 Canto announcement reports lower word error rates on its
own real-world and difficult-audio evaluations. That changes the relevant
quality bar, but is a vendor evaluation rather than an independent comparison
with Minutes. Measure real speakers and live delivery before choosing a new
Minutes default. [Canto announcement](https://wisprflow.ai/whats-new)

## Implemented in this round

Dictation opens directly from the app header. Writing shows the saved shortcut,
hold/tap/cancel guidance and optional practice; app and recovery preferences
expand in place. App/website scope is selected separately from its name.

Recent text supports search, recovery filtering and full-text review above the
list. The selected source, app and time remain visible. Manual correction,
Copy, guarded Paste and Restore original work without a model. Closing retains
the in-memory draft; switching records protects changes. Optional local rewrite
presets fill instructions and never submit automatically. Changed editor state
invalidates late suggestions.

Forms distinguish Add, Update and Cancel, preserve failed drafts and show nearby
errors. Preference changes expose saving/error feedback and rollback. UTF-8
limits, keyboard special-key buttons, contextual action names and visible focus
states improve reliability and keyboard use. Hands-free capture exposes Finish
and Cancel; held capture explains release-to-finish. Delivery confirmation lasts
1.2 seconds.

## Remaining priorities

1. **Native acceptance:** final signed Dev build, real shortcut gestures,
   non-focusing HUD controls and verified insertion into owned destination apps.
   Browser fixtures cannot prove OS behavior. Tracking: `minutes-437j`.
2. **Quality evidence:** real speakers, accents, noise, pauses, names, numbers,
   negation and slow destination apps. Record WER, critical meaning errors,
   end-of-speech-to-insertion latency and actual delivery success. Tracking:
   `minutes-ub6x`. The existing synthetic corpus is a smoke test only.
3. **Ready-to-dictate acceptance:** the inline setup and recovery shortcut
   recorders are implemented and checked offline; signed native first-success
   acceptance remains pending. Tracking: `minutes-y73y`.
4. **Recovery acceptance:** retained-history search, retained-audio playback,
   confirmed deletion and staged retranscription are implemented offline on
   October5. Full compilation and signed native playback/readback remain
   pending. Tracking: `minutes-5do6`.

The implementation is a stronger candidate, not a completed SOTA claim. A
credible excellent score requires the remaining workflow work and native/user
evidence, not another cosmetic pass.

## Verification receipts

The artifact directory is
`/home/mat/.codex/artifacts/minutes-dictation-sota-20261004`. It contains
independent A/B assessments, browser fixture checks and captures, source hashes,
the detector report and Mac source-sync receipts. Browser fixture checks cover
search, filtering, draft protection, manual delivery routing, failed-save retry,
UTF-8 limits and stale suggestion rejection. The final Mac desktop suite passed
413 unit and four integration tests. Design-token and Apple Speech packaging
checks pass. Final native UI acceptance is pending; this round is not released.
The independent review confirmed final source and the preceding 16 browser
variants plus two HUDs. Final Writing and Words captures refreshed successfully;
the rest of the confirmation batch stalled in the browser driver. The Mac then
went offline, leaving the final signed rebuild and native click-test pending.

## Travel-day follow-up: the capture moment

The following work is a subsequent offline refinement. The 30/40 score above
belongs to the preceding independent assessment; this follow-up has not been
independently re-scored or accepted on the native Mac.

| Moment | Wispr's documented desktop behavior | Minutes candidate |
|---|---|---|
| Start | Hold the configured shortcut or use hands-free; start sounds can be disabled. | Hold or tap the saved shortcut; start cue follows listening, with a visible timer and gesture-specific controls. |
| Speaking | Moving white bars in a small floating bar; final desktop words arrive after stopping. | A measured seven-bar meter, steady blue capture dot, timer, Finish/Cancel in hands-free and release guidance in held capture. |
| Thinking pause | Documentation presents recording as a continuous session until the finishing gesture. | Fixed phrase Processing/Success events that previously showed Captured and sounded complete while still listening. |
| Finish | Stop, process and insert the formatted result into the selected field. | Spinner and Finishing; a distinct finish cue, then delivery acknowledgement from the actual outcome. |
| After delivery | Text appears in the destination; clipboard restoration and failure recovery have explicit behavior. | Compact Typed/Pasted confirmation for1.2 seconds; copy and recovery retain detail. Routine dictation leaves its final words on the clipboard. |
| Placement | Optional resting bar, docking to left/right/bottom, following the cursor's display. | A temporary non-focusing HUD, default top-center, remembered placement; monitor selection still follows Minutes' main window. |

Sources: [first dictation](https://docs.wisprflow.ai/articles/6409258247-starting-your-first-dictation),
[desktop bar](https://docs.wisprflow.ai/articles/1790396454-move-and-dock-the-flow-bar-on-desktop),
and [sound controls](https://docs.wisprflow.ai/articles/9192039587-using-wispr-flow-discreetly-microphone-guide).
These describe product behavior, not a side-by-side native timing measurement.

### Sound evidence

The current [official web demo](https://wisprflow.ai/demo) loads a public
`@tanay-wispr/webflow-package@6.5.17` demo bundle that references start and stop
WAVs. Downloaded reference files measure0.254 and0.303 seconds. Their strongest
peak-window frequency regions are approximately440 and280Hz respectively:
a higher start tone and a lower stop tone. This is asset analysis; it does not
prove the current desktop client uses identical audio or volume.

Minutes' existing0.6-second files were authored around a soft crystal-bowl
start, felt-mallet wooden stop, book-closing completion and low wooden trouble
knock. The start/stop reference files retain appreciable signal across roughly
0.45 seconds, while complete/error are brief impacts with mostly quiet tails.
The follow-up changes cue semantics, timing and overlap; it does not replace
the shared sound assets or call a generation provider.

The local browser preview in the artifact directory compares the prior432ce97
HUD with the current candidate and offers both products' reference sounds for
audition. It explicitly simulates state and input levels without microphone,
transcription, clipboard or application insertion. Light/dark and narrow captures
verify layout; settled loud-meter height changed from4px to16px, and delivered
pill height from88px to48px.

### Follow-up evidence and remaining limits

Five standalone Rust presentation tests and eleven executable HUD/settings
tests pass. Rust formatting, script parsing, design-token baseline and Apple
Speech authority checks pass. The earlier full desktop check stopped when
the compiler received SIGTERM. The later readiness follow-up compiled the current
combined desktop candidate successfully; see the current receipts below. Earlier413+4
Mac tests predate this backend presentation change and do not qualify it.

Tracking: `minutes-bv4d`. Late cancellation during final transcription needs
separate executable qualification before adding a processing Cancel control:
`minutes-iw8w`. Native placement/focus, actual sound level and timing, recording
gestures, microphone behavior and destination insertion remain Mac acceptance
work. No production release or new SOTA performance claim is made here.

### Late cancellation follow-up

Source inspection confirmed the stop branch checked cancellation only before
final decoding. Escape during a slow final decode could therefore arrive before
the output was delivered but still lose its chance to discard the result.
The candidate now checks both sides of decoding on all four live final-decode
paths, returns a cancelled decode to the shared audio/text discard path, and
checks cancellation again at the combined-session output boundary.

Four standalone tests of the actual shared cancellation gate pass, including a
channel-held decoder cancelled by another thread before it returns. Focused
Clippy and formatting pass. Additional core regression tests cover file/daily
note output and delivery callbacks in accumulated and per-phrase modes, plus
normal delivery as a positive control. Full-core build attempts received
SIGTERM; their output is not a passing qualification. The feature-enabled test
command must include `--features streaming,whisper`; otherwise the dictation
module is excluded. The correctly configured focused run also stopped with
SIGTERM while compiling the Linux `libspa` dependency, before core tests ran.

No processing Cancel button has been added. Native Escape routing, clipboard,
focused-field readback, history and recovery-audio behavior still require the
signed development app. This change cannot retract text already delivered or
interrupt an output operation once it starts. `minutes-iw8w` remains open for
that qualification. The unified readiness flow tracked in `minutes-y73y` is now
implemented offline; its native acceptance is still pending.


## Readiness and recovery shortcuts: current offline candidate

The next tranche combines actual shortcut registration, microphone selection and
permission, selected-model preflight, and destination guidance in Writing. An
explicit practice action focuses the real test field. Healthy checks can hide;
a new blocker reveals them. Actions lead to the existing microphone test,
permission help and an explicit model installation, without opening devices or
calling providers just to inspect readiness. Paste and History now use the same
key recorder as capture; cancellation or a conflict preserves the previous keys.

Current source is the owned `feat/dictation-experience` checkout at `432ce97`
plus 15 modified/new uncommitted files. The Mac was intentionally not contacted
while the user traveled. There is no new installation, pushed commit, merge or
release. Source hashes and proof paths are in the artifact `handoff.json`.

| Check | Current result | Receipt |
|---|---|---|
| Readiness and shared shortcut controller tests | 22 pass | `readiness-and-hud-tests.log` |
| HUD/settings controller tests | 11 pass; 33 combined | `readiness-and-hud-tests.log` |
| Full desktop compilation, Linux without default features | Pass, with 50 platform warnings | `readiness-desktop-check.log` |
| Initial source-based browser batch | 13 functional checks pass; six captures | `readiness-browser-receipts.json` |
| Final browser confirmation | Eight behavior/layout checks pass, no JS errors; primary button contrast 5.05:1 in light, dark and narrow variants | `readiness-browser-confirmation.json` |
| Syntax, Rust formatting, design-token baseline and Apple Speech packaging seal | Pass; design baseline remains 499 | `readiness-final-source-checks.log` |
| Strict desktop Clippy | Fails on 53 Linux/platform diagnostics; all 53 diagnostic source excerpts also occur in HEAD | `readiness-desktop-clippy.log`, `readiness-clippy-baseline-audit.json` |
| Feature-enabled integrated core dictation tests | Did not run; Whisper C++ compiler was terminated while compiling its native dependency | `readiness-core-dictation-tests.log` |
| Signed Minutes Dev build and native acceptance | Pending while the Mac is offline | `minutes-y73y`, `minutes-437j`, `minutes-iw8w` |

The Clippy audit compares source excerpts, not a separate baseline Clippy run.
The successful desktop compilation does not replace the interrupted core test
suite or native acceptance. Required native evidence includes actual shortcut
registration/restoration, microphone and lid-state selection, real installation
progress, practice capture, sound timing, clipboard and destination readback,
and cancellation/history/recovery behavior.

`dictation-readiness-preview.html` is a self-contained source-based browser
preview with embedded assets and synthetic IPC; it needs no server. Its banner
states that no microphone, native shortcut or download is used. Final captures
include expanded light/dark/narrow states and the collapsed healthy overview.
The prior independent 30/40 score remains historical. No new recognition,
latency or SOTA claim follows from these offline checks.


## October5: retained-history recovery candidate

Recent now searches every retained entry, bounded by the existing100-record
retention policy, while initially displaying25 rows. Listen loads only retained
recovery audio by record ID; successful routine audio is still retired. Preview
transcription stages a result for explicit acceptance instead of overwriting the
record. Accepted results preserve the first raw/finished transcript and original
delivery provenance. A changed source rejects acceptance; deletion requires a
separate inline confirmation. Delivery labels distinguish verified target writes
from copied fallbacks, unverified insertion and recovered-but-not-pasted text.

Artifacts are in `/home/mat/.codex/artifacts/minutes-dictation-history-20261005`.
The46 controller tests comprise13 new recovery tests and33 existing readiness/HUD
tests. Four standalone preview-store tests and14 focused production storage-module
tests pass, including accepted/stale replacement, legacy records,100-entry access,
private WAV reads, symlink/outside-path rejection and byte ceilings. Focused Clippy
passes for both Rust harnesses. The storage harness includes exact Unix privacy
helper source and a fixture-only Config adapter that rejects ambient access; it
is not whole-crate or native-app qualification.

The attempted full desktop check received SIGTERM while compiling the core crate,
before qualifying the new backend. The October4 passing desktop check predates
these recovery changes. The latest feature-enabled cancellation suite also remains
unrun after compiler termination. Neither interruption is a passing check.

Browser fixtures exercise the actual controller/settings wiring with synthetic
history, one generated tone and simulated IPC. They establish layout and interaction
behavior, not real audio output, local model quality, persistence through the native
app, target insertion or privacy permission behavior. The final browser receipts and
captures belong to this candidate; the prior independent30/40 heuristic score has
not been updated. There is no new SOTA or comparative recognition claim.

The Mac remains intentionally uncontacted while offline. The signed Minutes Dev
build/click-test gate still precedes commit/push. New audio/acceptance commands are
registered in main; a complete byte comparison against its prior sealed source
shows only two registrations were added. The packaging check's main hash reflects
that narrowly reviewed change; Apple Speech authority logic is unchanged.


## October 5: native and Intel qualification update

The MacBook returned online. The canonical installer built the candidate into
`~/Applications/Minutes Dev.app` (`com.useminutes.desktop.dev`) using the existing
Apple Development identity, Team `63TMLKT8HN`. Strict bundle verification and the
installed Input Monitoring diagnostic passed. A new installed process, PID 42606,
started after executable replacement. Tests used an owned QA configuration,
synthetic history and audio, without editing user history or replacing the
production app. Receipts are in
`/home/mat/.codex/artifacts/minutes-dictation-stonebook-20261005`.

Native checks verified retained-history search beyond the initial 25 rows (one
match among 68 records), audio review, actual local Whisper retranscription,
saved/proposed comparison and explicit acceptance. Acceptance preserved the first
original transcript and its delivery provenance, retained audio, marked the new
text recovered/unverified, and left clipboard change count unchanged. An invalid
speech fixture first produced an error without losing saved text or audio. The
valid fixture was 4.13 seconds of local Samantha synthetic speech; its recognition
result is a functional recovery check, not a human accuracy measurement.

The readiness UI installed the selected base model into the owned QA model
folder. The native microphone check opened an input and finished with nothing
saved. The shortcut recorder saved and enabled a QA combination; native readiness
reported it active, including after recording keys was canceled with Escape.
Remote synthetic key injection did not establish an actual dictation session.
Physical shortcut capture, live HUD/cancel behavior, audible cue timing and
actual destination readback remain unverified for this candidate.

| Gate | Current result |
|---|---|
| Full core suite, Apple Silicon, no default features | 1,917 passed; one ignored |
| Feature-enabled dictation core suite, Intel | 88 passed, streaming/Whisper |
| Full desktop unit suite, Apple Silicon | 422 passed |
| Full desktop unit suite, Intel | 422 passed |
| Full Intel desktop compilation, parakeet/metal | Passed |
| Strict workspace Clippy and both Rust formatting checks | Passed on Apple Silicon |
| Recovery/readiness/HUD JavaScript controller tests | 46 passed |
| Apple Speech packaging guard, design-token baseline and diff checks | Passed |

The old Rust textual HUD test expected a `success` switch case that the new
continuous-listening controller deliberately avoids. Its assertion was updated
to require the early return before delivery, cues or dismissal; all 11 executable
HUD tests also pass. The installer preceded only this test-only correction and
the matching packaging hash update. Complete source comparison confirms no
production Apple Speech authority change beyond the two reviewed command
registrations. The installed runtime and UI match the current candidate.

Stonebook uses a user-owned Swift 6.3.3 toolchain and macOS 26.5 SDK data; its
system developer tools remain unchanged. Both unchanged Swift bridges type-check
for Intel. The Intel results qualify source and tests, not a signed Intel app:
Stonebook still has no usable signing identity. Its existing Dev app is preserved.

Mat requested pausing desktop input if another agent used Cua. The local session
inventory confirmed active `pdf-input-repair-20261005`; Minutes ended its own Cua
session and paused native input. Remaining native checks precede commit/push under
the repository rule. No release or merge has occurred. The previous 30/40 UX
assessment remains historical; recognition, real-speaker latency and comparative
SOTA performance have not been rescored.


## October 6: waveform capture, warmer surface and native interactions

Desktop access resumed after the PDF Tools session ended. The canonical signed
installer rebuilt Minutes Dev twice as Mat refined the capture design. The latest
app uses a 192 by 40 point pill with a real-level seven-bar waveform, discreet
Finish and Cancel icons, no listening label, timer or status dot during ordinary
capture. It uses warm solid tones and a faint static dither texture rather than
metallic gray shading. Important copy-only warnings and terminal recovery actions
remain visible. Finish guidance and accessible labels remain available.

Dedicated dictation sounds replace the capture and delivery cues with brief,
quiet, enveloped tones. Processing and phrase checkpoints play no completion
cue. Existing meeting-recording sounds are byte-identical to the source base.
Code and cue scheduling are tested; perceived loudness and sound quality still
need Mat's listening test.

The final installed executable SHA-256 is
`a5c8578e796d6c285e3b6663a2ca2725079c6115925cddccd41ad0e1c1273c48`.
Its bundle is `com.useminutes.desktop.dev`, signed by the existing Apple
Development identity for Team `63TMLKT8HN`. Strict bundle verification passes.
PID 11484 started after the latest replacement at October 5 22:45:04 EDT.
A fresh Input Monitoring diagnostic passed. QA used a separate configuration,
synthetic history and an owned TextEdit document.

Native Cmd-Option-K injection now starts an actual microphone stream. Both the
compact waveform-only version and final warm pill were inspected during capture.
Clicking Cancel stops the stream; the compact test preserved the complete history
hash and 70-record count, and the final warm check left no active capture PID.
This is genuine native capture/cancel evidence, but not a physical Fn test or a
measurement of real-speaker latency.

Native dictionary creation persisted its spelling rule. Editing opened the
prepopulated form and persisted an updated value, although remote Select All did
not clear the old field, so exact text replacement is not established by that
automation. Save snippet from Recent Text prefilled the recovered transcript;
naming and saving it persisted the exact transcript in the QA configuration.
Explicit Delete saved audio displayed its inline confirmation, removed the owned
WAV and audio reference, and retained text, the first original transcript, delivery
provenance and all 70 history records.

The owned TextEdit destination changed before the Paste action. The app showed
“app changed, text is on the clipboard” and preserved the recovered record's
unverified status. This verifies safe fallback, not successful insertion. Actual
TextEdit insertion was qualified on the earlier source base; insertion code is
unchanged, but this round did not establish fresh successful target readback.
The clipboard now holds synthetic recovery text from the fallback check; its
previous contents were not captured and cannot be restored.

Final checks pass: all 422 desktop unit tests after updating the stale textual
HUD fallback assertion, all 46 JavaScript controller tests, both Rust formatting
checks, Apple Speech packaging guard and the 496-entry design-token baseline.
The earlier complete core, feature-enabled Intel and strict workspace Clippy
results still apply to unchanged Rust production source. Complete main source
comparison confirms only the two reviewed command registrations affect runtime;
the other main changes are two existing HUD contract tests.

This candidate is ready for Mat's development-app test, not a release acceptance
or a new SOTA score. Physical Fn, real speakers, noisy rooms, end-to-end latency,
screen-reader behavior and slow-destination trials remain tracked in
`minutes-ub6x`; fresh successful destination readback remains in `minutes-5do6`.
Stonebook source qualification does not qualify a signed Intel installation.


### October 6 texture follow-up

Mat approved softening the perforation-like surface. Dot opacity changed from
10 percent to 3.5 percent, with half the dot density in an irregular 16-point
tile. Warm colors, pill dimensions, waveform and controls are unchanged. The
canonical signed installer rebuilt the app; the real microphone capture surface
was inspected and Cancel preserved all 70 QA history records and the full history
hash. All 46 controller tests and the design-token baseline pass.
The new installed executable SHA-256 is
`3c91c93f192db58aeb3902e98a176979148eed94a3093bc78695b80cb5d2792b`.
This replaces the earlier warmer-surface build for Mat's development test; human
speech, sound and latency acceptance remain separate.


### October 6 lifecycle follow-up

Mat requested the warm pill throughout dictation, without cycling through startup,
finishing, typing and pasted labels. The initial HTML frame now uses the same
192px warm shell. Startup and final decode/insertion share three quiet dots;
actual microphone levels continue to drive the recording waveform. Verified
typed/pasted outcomes dismiss with the existing 150ms fade. Clipboard-only
delivery shows a clipboard icon; hover/focus pauses dismissal and clicking the
icon opens the text. Failed insertion, setup and recovery keep useful explanations
and actions. Accessible announcements retain the outcome distinction.

The canonical signed Minutes Dev installer completed with strict bundle verification
and the existing Apple Development identity, Team63TMLKT8HN. Its executable SHA-256
is `ce65b47f6c627da1a08a48d49d40d83d8bb6bcbada965cc60f4301e043562b38`.
In the isolated QA profile, real microphone capture, Finish, a compact clipboard
icon and clicking it to keep the expanded text visible were inspected natively.
Three synthetic spoken utterances produced clipboard-only outcomes; persisted
history and the actual clipboard matched the latest text. All 73 QA records
remain; no active capture PID remained. Output mute/volume and the QA configuration
were restored, then the normal Fn/Whisper Small app was relaunched.

All 49 controller tests and 422 desktop tests on each architecture pass. The
shrink-only design baseline, formatting and Apple Speech packaging guard pass.
Two obsolete startup assertions were updated after the signed installer; only
test code and the corresponding source seal changed afterward. Production main
code remains identical to the previous committed candidate. The first Intel retry
used the system SDK and failed to link Speech symbols; using the existing owned
Swift 6.3.3/macOS26.5 SDK environment passed without changing source or system tools.

Light browser fixtures verify startup, busy activity, clipboard and expanded text;
real native dark capture/clipboard views verify the installed surface. Browser
fixture audio/transport failures were not treated as native feature evidence.
Processing can be too brief to appear in native screenshots; its controller and
rendered fixture were checked separately. Fresh successful external insertion,
physical Fn, perceived sound, recognition quality and comparative latency remain
the previously documented acceptance gates.
