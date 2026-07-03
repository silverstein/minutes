# Live recording sidecar: inline inference starves audio, freezes status, and can go dark for the rest of the meeting

## TL;DR

The recording sidecar ran transcription inference **synchronously on the audio-consumer thread**. Any slow or wedged engine call (parakeet cold-start subprocess, whisper finalize) stopped chunk consumption (bounded channel → silent audio drops → garbled lines), stopped the 1s status heartbeat (CLI/status flips stale), and deferred stop handling. With `engine = "parakeet"` and `parakeet_sidecar_enabled = false`, each utterance paid a **cold subprocess spawn + model load**, and the subprocess was waited on via `Command::output()` with **no timeout** — one wedged call silenced the live transcript for the remainder of the meeting while the recording itself stayed healthy.

Observed live on 2026-06-10 during a real 37-minute Zoom call (Creative Planning discovery, job `job-20260610113836645`).

## Observed symptoms (2026-06-10, Minutes.app 0.18.6 w/ parakeet,metal)

- `current.wav`: healthy, continuous audio for the full 37:29 (sampled RMS 0.05–0.09, peaks 0.68–0.81 at t=60/240/420/634s).
- `live-transcript.jsonl`: **5 lines total**, all garbled, last at offset 396s:
  - line 3 (offset 288s) is recognizably a mangled rendering of what was actually said at that timestamp (verified against a whisper-large-v3-turbo offline transcription of the same audio) — i.e. the sidecar heard the right stream but with chunks missing mid-utterance.
- `live-transcript-status.json`: heartbeat advanced normally to `updated_at 11:10:55, last_offset_ms 588103, state "healthy"`, then **froze for the remaining ~27 minutes**. File still claimed `healthy`.
- `minutes transcript --status`: `active: false, pid: null` (the PATH CLI was also a stale 0.9.4 from `~/.cargo/bin` shadowing the current install — see Environment notes).
- Recording stop at 11:38:36 completed promptly; the post-stop batch pipeline produced a correct full transcript.

## Root cause (3 compounding defects)

1. **Inference on the consumer thread** (`crates/core/src/live_transcript.rs`, `run_sidecar_inner_mpsc`):
   the loop that drains the capture channel (`sync_channel(200)`, producer uses `try_send` and silently drops on full — `capture.rs:761`) also ran VAD *and* per-utterance transcription. Producer chunks are ~10–100ms, so the channel holds only seconds of audio; any engine call longer than that drops audio. Drops were only counted and logged once at recording end (`SIDECAR_DROPS`).
2. **No timeout on the parakeet one-shot subprocess** (`crates/core/src/transcribe.rs`, `run_parakeet_command_with_cpu_fallback` → `Command::output()`):
   with the warm sidecar disabled, every utterance spawns a fresh subprocess with a cold model load. A wedged subprocess blocked the consumer thread indefinitely; `stop_flag` is only checked at loop top.
3. **Wasted whisper partials in whisper-fallback mode**:
   the sidecar fed `StreamingWhisper::feed()` whose partial results were explicitly discarded ("intentionally not emitted in event-bus v0") — full re-transcription of the growing utterance buffer every 2s, on the consumer thread, for nothing.

## Fix (shipped in this change)

- **Worker-thread architecture** (`run_sidecar_inner_mpsc`): the consumer now only does recv + VAD + utterance buffering + heartbeat; finalized utterances go to a dedicated `live-sidecar-transcribe` worker over a bounded queue (cap 3). A backlogged engine now costs individual utterances (counted) instead of the session. Consumer never blocks > 100ms → heartbeat stays truthful, capture channel never backs up, stop is responsive.
- **Backlog visibility**: `LiveStatus` gains optional `pending_utterances` / `dropped_utterances` fields and a human-readable `diagnostic` when utterances are dropped (additive, serde-compatible).
- **Hard subprocess timeout** (`transcribe.rs`): parakeet one-shot invocations are killed after `clamp(3× audio duration, 90s, 1800s)`; pipes drained on threads to avoid stdout deadlock. Applies to batch and live paths. On timeout/failure the live session falls back to whisper permanently (existing fallback warning preserved).
- **Whisper partials removed** in sidecar mode: samples accumulate raw; one transcription per utterance on the worker.
- **Post-stop drain**: after `stop_flag`, the worker drains its queue without transcribing — the recording's batch transcript supersedes the live feed, and stop latency is bounded by at most one in-flight utterance (≤ subprocess timeout).

## Not fixed here (follow-ups)

- **Standalone `minutes live`** (`run_inner`) still transcribes inline on its loop; it benefits from the subprocess timeout but should get the same worker split. (Less acute: utterance cadence is the only thing it starves, not a recording.)
- **`[live_transcript] device` config key** is silently ignored (no such field in `LiveTranscriptConfig`) — present in at least one real config. Either support it for standalone live or warn on unknown keys.
- **Stale CLI shadowing**: `~/.cargo/bin/minutes` (0.9.4!) shadowed the current CLI on PATH; `minutes verify` should detect PATH shadowing / app-vs-CLI version skew.
- **CLAUDE.md manual build instructions** omit `--features parakeet,metal` (the script and CI both set them) — a dev following the manual line builds a parakeet-less binary and gets the silent whisper fallback. (A parakeet-less dev binary produced exactly this warning at 10:30 the same morning.)
- **Timeout-policy unification**: parakeet now has two independent timeout formulas — one-shot subprocess (3× duration, clamp 90–1800s, `transcribe.rs`) vs warm sidecar (`parakeet_sidecar.rs::request_timeout`, 2× duration) vs whisper's abort callback (300 + 3×, cap 3600s). One engine-level timeout policy would prevent divergence.
- **Duration-helper duplication**: `transcribe.rs::estimate_wav_secs` (hound, wav-only) vs `diarize.rs::audio_duration_secs` (symphonia, any format). Consolidate into a shared audio util; symphonia version is strictly more capable but lives behind the diarize module today.
- **Chunk-drop visibility**: capture-side `SIDECAR_DROPS` (the root-cause counter) is still only logged at recording end; utterance drops are now in the status file but chunk drops are not. Surface both, and consider a `live.backlog.*` event on the events bus so the app UI can react rather than poll.
- The earlier, separate diarization issue: `ISSUE_silent_system_stem_diarization.md`.

## Repro / verification evidence

- Runtime artifacts from the incident: `~/.minutes/live-transcript.jsonl` (5 lines), frozen `live-transcript-status.json` (captured in session logs), `~/.minutes/jobs/job-20260610113836645-92169-0.wav` (72MB, healthy).
- New tests: `sidecar_utterance_queue_drops_newest_when_full_and_counts`, `sidecar_utterance_queue_disconnected_worker_is_silent`, `status_file_surfaces_transcription_backlog` (`crates/core/src/live_transcript.rs`).
