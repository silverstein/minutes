# Live audio coverage diagnostics

`[live_transcript] coverage_shadow = true` enables an experimental, text-free recording-sidecar coverage file. It defaults to false. Final meeting recognition and existing live JSONL/event fields are unchanged.

Recorder sends carry a stream identifier, a source category and a 16 kHz forwarded-stream sample interval. The sample counter advances before each send attempt, so a later successful send exposes a dropped optional-consumer batch. Native and dual-source feeds are marked mixed. Single-device capture remains unknown because a selected input device is not proof that the source is a physical microphone. These positions are not hardware capture timestamps, independently bound source files or verified source continuity.

VAD exclusions, submitted spans, dropped recognition jobs, skipped-at-Stop jobs and finalized transcript lines are recorded when diagnostics can acquire the existing writer without waiting. Speech spanning an input discontinuity retains separate ranges. Final records hash normalized text, link its existing JSONL line, and include resolved backend, configured model name and language intent. Drafts never enter the coverage file. Model names are not model-content integrity proofs.

The diagnostic is `live-transcript.coverage.jsonl` beside the ordinary scratch JSONL. It has a versioned manifest, owner-only creation on Unix, a 16 MiB budget, and an incomplete checkpoint. A new session atomically replaces the slot, so a late old writer cannot corrupt the new file. Windows may refuse replacement while an old writer holds the slot open. That session continues without diagnostics and logs a warning; a later session can replace the slot after the old handle closes. Failures disable the diagnostic alone; a failed or interrupted write can leave an incomplete last line. The file is overwritten by the next enabled sidecar that can open it, and is not copied into archived standalone sessions. Turning the option off does not remove the prior diagnostic.

Every manifest/final/checkpoint explicitly denies reuse authority. Capture end and source-file binding remain unknown, queue/diagnostic observations can be missing, and a stopped worker can skip speech. Legacy `run_sidecar_mpsc` callers still work, but their inputs are unpositioned. No reader may infer silence from absent text or use this file to skip final recognition. No current product code reads it for that purpose.

This is the recording-sidecar foundation. Standalone capture, per-source channels and epoch/reset binding, source-integrity checkpoints, retention integration and matched language/quality assessment remain prerequisites for accepted live-text handoff. Current recording-sidecar Whisper output is not automatically equivalent to final Parakeet output.

## File-only smoke test

Use a private isolated data directory and an explicit test config with local models. Do not set HOME or point the harness at the normal Minutes data directory. Use public input, at most one minute, in 16 kHz mono i16 WAV format. The harness copies the input, appends two seconds of silence, and drives the production native-stem feeder for fifteen seconds without opening an audio device.

Build `replay_live_coverage` with `--no-default-features --features streaming,whisper` (and `metal` for the Mac accelerated path). Set `MINUTES_DATA_DIR` and `MINUTES_CONFIG_PATH`, then invoke the example with `INPUT.wav`, followed by `--coverage` for the enabled comparison. Separate enabled and disabled runs should preserve finalized text hashes. Counts and hashes are emitted; the input words are not printed. This smoke test does not qualify actual GUI Stop, native-device capture, battery performance or transcription accuracy.
