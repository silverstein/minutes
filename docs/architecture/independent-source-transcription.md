# Independent source transcription

Final meeting processing recognizes a saved microphone WAV and system WAV
separately when both are present and contain signal beside the recording.
This applies to native `.mov` recordings and dual-source `.wav` recordings.
Both foreground processing and background transcription use the same dispatch.

The previous mix was an intentional workaround for the macOS dual-track MOV
decoder duration bug (#234, #235), not a live-transcription optimization.
Recognizing the saved WAVs separately continues to avoid that decoder while
allowing quieter microphone speech to survive overlapping system speech (#1176).
Live capture, live draft transcription and optional consumers are unchanged.

The two recognition calls run sequentially to bound model memory. Per-source
word thresholds are disabled; the meeting-level threshold still applies after
combining results. Empty/no-speech source results can coexist with speech from
the other source. Decoder/model failures propagate; there is no mixed-audio
fallback that could silently discard one side's words.

Whisper preserves quiet intervals on this path, including a retry without VAD.
Compressing each source's silence independently would move words off the shared
recording clock. An exact digital-zero prefix can be omitted for recognition,
with its sample-exact offset restored to both structured segments and rendered
timestamps. Nonzero quiet samples are never removed by that adjustment.
Natural-pause chunks retain their original source offsets.
Merged lines are sorted stably by start time, using precise engine timings when
available and timestamped text otherwise. Meeting duration is the longer source
duration, not the sum of the two tracks.
These remain ASR timestamps, not forced alignment of individual words; noise
before an utterance and coarse engine segments can still limit their precision.

Before diarization, lines carry `MICROPHONE` or `SYSTEM` source labels. These
labels do not identify a person. Source-aware diarization may add existing
speaker labels only when an utterance has unambiguous coverage in the matching
source; otherwise the source label remains. Several voices inside the system
stream remain a diarization/recognition problem within that stream.

Final merging conservatively suppresses high-confidence microphone echoes of
system speech by default. A normalized microphone line must have at least four
words. System lines starting within that microphone span plus or minus 1.5 seconds
are concatenated in time order; their word-level longest common subsequence must
cover at least 85% of the microphone words with at most two unmatched words.
The microphone span must also fit inside the span of the system lines that
actually matched, plus or minus 1.5 seconds. Only microphone lines are removed;
system lines, short replies such as "yes", untimed lines and uncertain matches
remain. Comparisons lowercase words and trim surrounding punctuation. Whole-second
chunk timestamps and inferred ends limit precision; this is a text heuristic,
not proof of acoustic echo. Simultaneous near-verbatim speech can still match.

Set `transcription.cross_source_echo_suppression = false` to retain all source
lines, including echo duplicates. The `cross_source_echo_suppressed` counter and
per-source diagnoses are logged during final merging. This does not perform
acoustic echo cancellation or separate voices already mixed within one source,
and cannot recover audio that was never recorded.

Single surviving stems retain the existing explicit recording-health warning.
If neither source track is usable but the original meeting WAV has a valid
decodable signal, preparation can recover that WAV with an explicit warning:
coverage of both sides cannot be verified and source labels are unavailable.
This recovery happens before recognition, never after a source decoder/model
failure. The original WAV and damaged source files remain preserved. MOV files
retain their strict source-recovery gate to avoid the dual-track duration bug;
unsafe/failed source-header repair also remains an error.
Ordinary single-source inputs and non-meeting processing retain their previous
routes. Descriptor/proof-authorized processing never discovers ambient sibling
files: those entry points continue to process only their exact authorized
capability. A recovery job authorized as one mixed capability remains one input.
Independent recognition there requires separately authorized source members;
it must not be enabled by pathname discovery or a caller-supplied token.
