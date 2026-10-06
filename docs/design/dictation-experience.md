# Dictation experience

Minutes dictation extends the desktop's existing paper, ink, Geist and Instrument Serif system. This is an Operate surface: speaking, placing words, and recovering mistakes should take less attention than the words themselves.

## Direction contract

The first view puts the dictation shortcut, destination and writing style together. Four keyboard-navigable tabs separate Writing, Words & Snippets, Microphone and Recent Text. Existing recording settings retain their own scope. Rows and native controls carry the information, with one border between sections and visible focus states.

The main interaction is the microphone level check: start it, see the selected input and live level, then stop or leave the panel. It stops after fifteen seconds and saves no audio or transcript. Primary capture cancels the optional test. Motion is limited to short hover transitions and respects reduced motion.

The correction interaction is explicit: open a recent dictation, edit one spelling, request a candidate, then review the spoken and written phrases in the dictionary before pressing Remember spelling. No background monitoring or silent learning occurs.

## Behavior

- Cursor context is opt-in and Mac-only. It uses supported Accessibility fields, with at most 256 characters on either side and a short selection exposed to the formatter. The native snapshot temporarily holds the field value, bounded to 32 KB, in memory for field verification and undo. Secure fields, terminals, recognized financial sites, large documents and unsupported controls are excluded.
- Website rules require cursor context and exact hostnames. Exact website rules take priority over exact app names or bundle identifiers. Terminal prose is an explicit app preference; shell/code mode remains literal.
- Personal spelling rules are visible and removable. They supply bounded local decoder hints where supported. Literal writing skips decoder hints and cleanup. Uncertain correction candidates remain unsaved.
- Preferred microphones have an explicit order. Disconnected inputs fall through to other permitted physical inputs. A closed laptop lid excludes the built-in input from these fallbacks. System mode follows the OS default; shared mode retains the recording microphone behavior.
- Optional Paste Last and Recent Dictations shortcuts are disabled by default. A shortcut must include a modifier and cannot reuse another registered Minutes shortcut. Failed registration or persistence rolls back the bindings.
- Recent Text opened from its shortcut remembers the destination app. Pasting verifies that target; when a captured field changed, text is copied instead. Opening ordinary settings clears that destination.
- Undo requires an exact field, value and caret match after insertion. It restores the original selection only when all checks pass. Otherwise it copies the original words.
- Recovery retranscribes saved audio into history. It does not paste or change the clipboard. The private audio remains until explicitly deleted.
- Routine dictation retains its existing contract: final words stay on the clipboard. Pasting recent text uses rich clipboard restoration, which preserves readable item formats within a 16 MB bound and yields to any newer clipboard copy. Users can allow half a second, one second, or five seconds for the destination to read a paste. Longer waits delay the next dictation.
- Local editing is a preview using the configured Ollama model on this computer. The original remains available. Copy, paste and discard are separate choices. Suggestions still require checking names, numbers and meaning.

## Evaluation

`scripts/build_dictation_synthetic_corpus.py OUTPUT` creates 120 clean synthetic WAV files using two installed macOS voices at three rates. It writes reference texts, audio hashes and explicit provenance. It makes no network requests and captures no microphone audio. Run the existing file benchmark with `scripts/run_dictation_benchmark.sh OUTPUT/corpus.json REPORT_ROOT`.

The corpus covers names, numbers, negation, messages, email, agent prose, corrections and long dictation. It is a reproducible smoke test, not evidence about accents, room noise, human pauses, hotkey latency or Wispr performance. Engine comparisons refuse a production fallback under a requested engine label. A default-engine change requires real-speaker and live-delivery evidence.

Implicit “actually” editing remains disabled. Changes to optional consumers must preserve capture and recoverable audio. Signed native acceptance uses `~/Applications/Minutes Dev.app`; browser fixtures cannot establish Accessibility, hotkey, device, clipboard or insertion behavior.

Native dogfood can set `MINUTES_CONFIG_PATH` to a test config and `MINUTES_DATA_DIR` to an absolute test-state directory. The latter isolates history, recovery audio and capture state for that process; empty or relative values retain the usual `~/.minutes` location. Set the test config's output directory separately. Ordinary launches retain their existing paths.

## Everyday interaction refinement

The app header opens Dictation directly. Writing shows the actual saved shortcut,
three gesture instructions, and an optional practice field. Practice follows normal
dictation settings; it is not a separate audio sandbox. App-specific writing and
recovery preferences expand in place. The app/website editor separates scope from
name, so users do not enter config prefixes.

Recent text searches the latest 25 loaded records across saved text, raw text and
app name. Needs recovery isolates retained audio with no finished transcript. The
count explicitly states the search boundary; this is not full-library search.
Empty filters offer Clear filters without changing history storage.

Review opens above the history list and retains record identity, app and time. The
full saved original remains available separately from the working text. Manual
Copy and guarded Paste do not require a model. Close retains the session draft;
opening a different record with changes requires Keep editing or Load other text.
This draft is in memory only and does not rewrite the saved history record.

Local edits are optional. Presets fill an instruction without submitting it. An
editor revision invalidates asynchronous suggestions when text, instruction or
source changes, including a close or a change away and back to identical text.
Suggestions never replace the working text or original automatically.

Editing a dictionary entry, snippet or app preference exposes Update and Cancel.
Failed saves keep the form draft, show an adjacent error, and re-enable controls.
Snippet and local-edit inputs respect the backend's UTF-8 byte limits before
submission, including multibyte text. Row actions have contextual accessible names;
Caps Lock/fn choices are keyboard buttons and On/Off toggles expose pressed state.

Capture keeps its non-focusing HUD and existing stop/cancel commands. Its compact
192px pill shows the voice meter, a checkmark to finish and an x to cancel. Held
capture offers Cancel; release guidance stays in the tooltip and accessible announcements.
Capture state text, the duplicate dot and elapsed time are hidden while listening.
An early clipboard-only destination remains explicitly labeled. The surface uses
a fine static dither texture, warm solid meter tones and high-contrast controls
in light and dark appearances, without metallic bar gradients.
The same warm shell now carries startup and final processing. Missing
model completion refers to the user's dictation shortcut, rather than assuming fn.
These controls do not change capture engine defaults or rewriting policy.

## Capture feedback through a whole utterance

Desktop dictation accumulates phrases until the finishing gesture. A thinking
pause can finish transcription of a segment while the microphone session
continues. Segment Processing and Success events therefore do not replace the
active capture presentation. Once stop is requested, later listening events
cannot return the HUD to recording. Only explicit typed, pasted or copied
outcomes acknowledge delivery. This policy changes presentation, not capture,
transcription, cancellation or insertion authority.

The input meter uses seven 16px bars with a measured 4–16px range. A stable
silhouette varies with actual input level, not a time-based decorative wave.
Non-finite levels are ignored and out-of-range levels are clamped. The
meter carries the capture state without a duplicate dot. Startup, final decoding
and insertion share three softly pulsing dots in the same 192px shell; processing
does not restart that animation or cycle through implementation labels. Reduced motion remains supported.

Dictation startup stays silent: the microphone is already open when Listening
arrives, so a start cue can be transcribed as music. Delivery has a brief, quiet
sound acknowledgement after finalization; finishing is shown visually. A rapid
new capture interrupts any remaining delivery cue. Segment success is silent; blocked delivery and retained-audio recovery use the trouble cue.
Fast transitions interrupt the prior cue rather than layering sounds. Cancel
stops playback quietly. Muting immediately stops a playing cue and prevents
future cues. The Writing panel exposes the shared recording/dictation sound
preference directly.

Verified typed/pasted insertion dismisses with the existing 150ms fade, without
a success label. Explicit clipboard delivery shows a clipboard icon for 2.4
seconds. Hover or focus keeps it available; clicking the icon expands the saved
text and cancels dismissal. Failed insertion and actionable recovery explain the
problem and retain their help controls in the warm expanded shell. Accessible
announcements still distinguish capture, processing, insertion and clipboard
delivery. A new session cancels a pending fade so an old success cannot close
the fresh recording. The actual destination field is the routine text preview;
the HUD need not repeat it above the user's work.

`node --test scripts/test_dictation_overlay.mjs` executes the shipped controller
with deterministic DOM/audio/Tauri doubles. The small `dictation_feedback.rs`
policy also has standalone Rust tests. Neither establishes native focus,
microphone teardown, speaker volume or real insertion behavior.

### Cancellation during final decoding

Every live final-decode path now checks cancellation both before and after the
blocking decoder. If Escape arrives during decoding, the new text never reaches
the utterance output handler. Capture returns to its existing discard path,
clearing accumulated phrases and discarding the private recovery capture.
The combined-session output boundary also checks cancellation before writing.
This concerns pending output; it cannot undo text already delivered by a CLI
stream or interrupt an output operation that has already started.

The shared cancellation gate has a threaded test: decoding waits on a channel,
another thread cancels, and only then does decoding return. Its four standalone
tests and focused Clippy check pass. Two additional core tests exercise the
actual output helpers with temporary files and daily notes, including an
uncancelled positive control. Their full-crate execution remains a separate
qualification gate. Dictation is compiled only with both `streaming` and
`whisper`, so a no-feature test run cannot qualify these paths.

The processing HUD still has no Cancel button. Exposing it awaits full core
qualification and signed native Escape/delivery testing; the source guard alone
does not establish keyboard routing, microphone teardown or destination state.


## Readiness and practice in Writing

Writing now checks the saved dictation shortcut, the usable microphone candidates,
the selected model's real filesystem preflight, and text delivery in one inline
section. Each snapshot has a six-second deadline; a failed or stale response
cannot leave a misleading Ready label. Microphone selection follows the capture
engine's preferred-device, virtual-device and lid-state rules. Checking does not
open the microphone, download a model or call a transcription provider.

Shortcut and microphone permission are required to capture. Automation permission
is displayed separately from capture readiness: explicit clipboard delivery can
work without it, and insert mode still needs an actual destination operation and
readback. Permission flags alone never establish successful insertion. Adjacent
actions focus the existing shortcut control, run the existing microphone test,
open permission help, or explicitly install the selected supported Whisper model.
The installation progress uses the real download event; failed installation leaves
the selection intact and the error visible.

Ready offers a practice action that focuses the existing textarea. Practice uses
the normal dictation and history policy. Healthy checks can collapse while that
action stays available; a new blocker expands the section again. Collapsing is
only a local presentation preference. The layout retains the paper/ink palette,
Instrument Serif headings and Geist controls. The primary action uses sanctioned
blue and cream tokens with measured 5.05:1 contrast in both themes.

Quick thought, dictation, Paste last and History now share one shortcut recorder.
Recovery actions show their keys and actual registered state instead of requiring
raw accelerator strings. Recording temporarily suspends the selected binding;
Escape, focus loss, clicking elsewhere, leaving Writing or a thirty-second expiry
restores the old binding. Pending probes cannot save after cancellation, conflicting
bindings retain the previous keys, and restoration failure stays visible as an error.
Recovery bindings use combinations; Caps Lock and fn remain capture choices.

The readiness/recorder suite runs the shipped controllers with deterministic IPC
and DOM doubles. Browser fixtures exercise the actual settings wiring and key
maps, with synthetic permission, device, model and shortcut responses. These
checks do not qualify real native shortcut registration, microphone input,
clipboard delivery or permissions. The signed development app remains the
required acceptance target before committing these UI changes.


## Retained history and recovery review

Recent requests all retained entries, bounded by the existing100-record policy.
Search includes saved, raw and preserved original text plus app names. Twenty-five
rows appear initially; Show more expands in batches. The recovery filter includes
retained audio even when some text was copied successfully. Verified insertion,
copied fallback, unverified insertion and recovered-but-not-pasted outcomes have
separate labels.

The existing review opens saved recovery audio only after Listen. Playback is
restricted to the main window and a history record ID; the client cannot submit
an arbitrary file path. The core reader validates private ownership/containment,
uses a capability-relative open, checks WAV format and rejects files above16MiB
before allocating their contents. Closing or switching the review releases the
media source. This does not retain audio for successful routine dictations.

Preview transcription performs the existing local retranscription but stages its
result in memory. A maximum of four previews expire after ten minutes. The UI
shows saved and proposed text together; preview/discard never write history or
paste. Use this transcript accepts the staged token only if the complete saved
record still matches the source of that preview. Acceptance serializes with
history writes and preserves the first raw/finished transcript and its delivery
provenance. Existing review edits prevent acceptance over a working draft.

Deleting audio requires an inline confirmation with Keep audio focused first.
Successful deletion keeps text; failure leaves a visible retry. Deletion also
checks that the record has not changed. No preview, acceptance or deletion
implicitly inserts text into another app. Native destination guards remain the
existing separate paste operation.

The recovery controller has13 executable synthetic tests. A focused harness
compiles the production storage module and exact Unix file-privacy helpers with
fixture-only configuration and runs14 storage tests. Four preview-store tests
exercise retention, expiration and consumption. These tests qualify their
bounded scopes; full desktop compilation and signed native audio/destination
acceptance remain required.


## October 5 native qualification

The signed canonical Dev app passed native retained-history search, audio review,
local recovery preview and explicit acceptance, preserving original text and
original delivery provenance without changing the clipboard. Actual selected-model
installation, microphone check completion and shortcut registration/restoration
also passed in an isolated QA profile. Full desktop tests pass on Intel and Apple
Silicon (422 each); the integrated Intel dictation suite passes 88 tests.

Physical shortcut capture, current live HUD/cancellation and destination readback
remain native acceptance gates. Remote synthetic key injection did not start a
capture. Desktop input is paused after identifying a concurrent PDF Tools Cua
session, per Mat's direction. The change remains an uncommitted candidate until
required native click testing finishes. The dated evaluation records scope and
receipts; human recognition/latency qualification remains separate.


## October 6 native qualification and capture presentation

The latest signed Minutes Dev build was inspected with the actual warm, lightly
dithered waveform pill during microphone capture. Cancel stopped capture. The
ordinary capture surface has no listening text, timer or status dot; meaningful
fallback warnings and terminal recovery controls remain. Dedicated brief start
and delivery cues leave phrase checkpoints and processing quiet; subjective
sound acceptance remains with Mat.

The isolated native profile also verified dictionary creation, snippet saving
from prefilled history text, and audio deletion confirmation. Actual persisted
state confirms deletion retains text and original provenance. A changed TextEdit
destination exercised clipboard fallback without a false pasted outcome. Fresh
successful insertion, physical Fn, noisy real speakers and perceived sound/latency
remain separate acceptance work. The October 5 pause is historical; desktop
access resumed and these changed surfaces were click-tested in the canonical
signed app before source publication. See the dated evaluation for exact scope.
