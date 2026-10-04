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
