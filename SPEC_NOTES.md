# PR-F capture-reliability audit

## Shipped-invariant audit (before PR-F)

The two incident investigations and the current `capture`, `live_transcript`,
`streaming*`, `transcribe`, `pipeline`, and `jobs` paths were audited before
adding tests.

- Consumer/worker split was already shipped in
  `live_transcript::run_sidecar_inner_mpsc`: capture-channel receive, VAD,
  buffering, and heartbeat stay on the consumer; inference runs on
  `live-sidecar-transcribe`; the utterance queue is a bounded `sync_channel(3)`
  and overflow drops newest. Existing tests
  `sidecar_utterance_queue_drops_newest_when_full_and_counts`,
  `sidecar_utterance_queue_disconnected_worker_is_silent`, and
  `status_file_surfaces_transcription_backlog` covered the queue helper and
  status serialization. They did not block a real worker or prove that a
  capture consumer continued under a deadline. PR-F adds
  `blocked_inference_worker_does_not_starve_capture_and_queue_stays_bounded`
  over the production non-blocking queue primitive, using barriers, injected
  counters, and `recv_timeout` as a hard deadline.
- The parakeet one-shot path already computed a duration-scaled timeout and
  killed/reaped timed-out children while draining pipes. There were no
  subprocess-boundary tests, and process construction was not mechanically
  centralized. PR-F moves timeout execution into `engine_process`, adds
  timeout/reap and oversized-pipe tests, and makes Clippy reject direct
  `std::process::Command::new` calls in `minutes-core`.
- Stop was already decoupled from transcription by
  `jobs::queue_live_capture`, which moves a finalized capture into the durable
  queue before processing. Existing tests proved the basic move
  (`queue_live_capture_moves_audio_and_writes_job_file`), no-speech terminal
  classification (`no_speech_artifacts_require_review_and_preserve_capture`),
  stale worker recovery (`list_jobs_recovers_stale_worker_claims` and
  `list_jobs_demotes_to_failed_when_retry_cap_exceeded`), and successful-output
  preservation (`preserve_audio_alongside_output_*`). None drove the real job
  state machine through all four historical failure outcomes with a valid WAV
  and stop deadline. PR-F adds
  `stop_deadline_preserves_sample_bearing_wav_for_every_processing_failure_outcome`
  with an injected fake transcriber for NoSpeech, an aborted engine subprocess,
  a timed-out engine subprocess, and an explicit transcription error.
- The #412/#414 guard was already implemented as
  `pipeline::detect_silent_remote_stem_warning`. Existing contract tests
  `native_call_recovery_marker_warns_and_degrades` and
  `in_person_system_silent_capture_does_not_warn_without_recovery_marker`
  directly cover the required positive and negative cases. Additional shipped
  negatives (`active_voice_zero_system_ratio_does_not_warn_without_recovery_marker`,
  `sparse_but_captured_remote_does_not_warn`,
  `non_call_mic_only_recording_does_not_warn`, and
  `healthy_call_with_both_stems_active_does_not_warn`) prevent heuristic drift.
  These tests already run in the no-default-features library target, so PR-F
  does not duplicate them.

## Spawn-site inventory

Before consolidation, `minutes-core` directly constructed subprocesses in:

- Engine/audio paths: `transcribe.rs` (ffmpeg decode, Apple helper, parakeet),
  `parakeet_sidecar.rs` (warm server and probes), `streaming_diarize.rs`
  (diarization sidecar), `transcription_coordinator.rs` (Apple helper),
  `diarize.rs` (ffmpeg/Python probes), `pipeline.rs` (ffmpeg stem mix and hook
  tests), and `autoresearch.rs` (ffmpeg).
- Platform/helper paths: `apple_fm.rs`, `apple_speech.rs`, `calendar.rs`,
  `capture.rs`, `dictation.rs`, `health.rs`, `hotkey_macos.rs`, `jobs.rs`,
  `screen.rs`, `summarize.rs`, and `system_audio_backend.rs`.
- Build-time helper path: `crates/core/build.rs` (Swift calendar helper); the
  build script imports the same `engine_process` module rather than defining a
  second constructor boundary.
- There is no `tokio::process::Command` use or Tokio dependency in
  `minutes-core`, so no Tokio constructor is configured.

All production constructors above now route through the single
`crate::engine_process::command` boundary. The only direct constructor is the
wrapper itself, with the sole `#[allow(clippy::disallowed_methods)]`.
`crates/core/clippy.toml` disallows `std::process::Command::new`, and the crate
enables that restriction lint so the exact acceptance command (without extra
lint flags) also enforces it. The required negative check was performed: a
temporary direct constructor in `minutes-core` made Clippy fail with `use of a
disallowed method`, and the scratch module was then removed. The root
`clippy.toml` is an empty workspace fallback: Clippy 1.95 searches
`CARGO_MANIFEST_DIR` first, so core gets the strict package-local policy while
CLI/Tauri keep their unrelated existing process launches. A root-level ban
would make the required
`cargo clippy --all` fail outside PR-F's permitted file scope. CI separately
compiles the boundary with `features = "parakeet"`.

## Historical incident coverage

| Historical incident | Behavioral suite |
| --- | --- |
| Live-sidecar inference starvation; #395/#396, #409 | Blocked-worker consumer-isolation test; shipped queue/drop/status tests; subprocess timeout/reap/pipe tests; parakeet-feature Clippy job |
| Cristy blank transcript and deleted WAV | Four-outcome stop-deadline/WAV-preservation test through `queue_live_capture` and the job state machine |
| #412/#414 silently lost remote audio | Native-call recovery positive contract plus in-person system-silence negative contract in `pipeline` |
| #467 capture reliability regression class | AST-enforced single process boundary plus deadline-based suites that fail instead of hanging CI |

Maintainer follow-up: add `actionlint` as a required status check in branch protection — it cannot join ci_gate.needs (separate workflow).

PR-D completion also requires verifying that the `actionlint` check is required by branch protection:

```bash
test "$(gh api repos/silverstein/minutes/branches/main/protection \
  --jq '[.required_status_checks.contexts[], .required_status_checks.checks[].context] | any(. == "actionlint")')" = true
```
