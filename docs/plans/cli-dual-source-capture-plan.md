# CLI Dual-Source Capture Plan

**Status:** implementation plan for `minutes-kb83`  
**Date:** 2026-04-12  
**Related:** GitHub issue [#115](https://github.com/silverstein/minutes/issues/115), [MULTI-SOURCE-CAPTURE.md](MULTI-SOURCE-CAPTURE.md), [CALL-CAPTURE-DURABLE-PLAN.md](CALL-CAPTURE-DURABLE-PLAN.md)

## Problem

The repo already contains several source-aware pieces:

- `RecordingConfig.sources`
- loopback auto-detection (`detect_loopback_device`)
- dual-stream runtime primitives (`AudioStream`, `MultiAudioStream`, `MultiDeviceMonitor`)
- stem-aware diarization (`.voice.wav` / `.system.wav`)

But `minutes record` still records through the single-device `record_to_wav` path. The result is a product mismatch:

- `--source a --source b` does not produce a two-device capture
- `[recording.sources]` is parsed but not honored as a real two-source record path
- `--call` can detect a loopback device but still does not record mic + system audio together

Issue #115 exposed the practical fallout: users can do everything "right" on an Intel Mac with BlackHole and still get only one side of the call.

## Goal

Ship a real CLI dual-source record path for macOS that:

1. captures microphone + system audio together from two input devices
2. writes:
   - a mixed WAV for the existing transcription pipeline
   - `*.voice.wav` and `*.system.wav` stems for attribution
3. keeps the current pipeline contract intact
4. fails loudly instead of silently degrading when the requested dual-source path is impossible

## Non-goals

- Programmatic CoreAudio aggregate-device creation
- Windows / Linux multi-source rollout in this change
- Perfect sample-accurate source synchronization across arbitrary devices
- Fancy per-source live UI health beyond honest CLI messaging

## Constraints

The implementation must satisfy all of these at once:

1. **Truthful UX**  
   If dual-source capture cannot be started, the CLI must say so clearly. No silent fallback to one device.

2. **Pipeline compatibility**  
   Existing processing should continue to run on the main audio artifact without needing a new job type or a new pipeline entrypoint.

3. **Source-aware artifacts**  
   The capture must leave behind `voice` and `system` stems using the same filename convention already expected by `diarize.rs`.

4. **Stop-path safety**  
   `minutes stop`, Ctrl-C, and queue handoff must remain graceful and must not strand half-written audio.

5. **Portable build behavior inside this repo**  
   `minutes-core` is compiled in multiple contexts. If the implementation depends on streaming-specific helpers, it must remain correctly feature-gated.

## User-facing contract

### Supported

```bash
minutes record --call
minutes record --call --device "Yeti Nano"
minutes record --source "Yeti Nano" --source "BlackHole 2ch"
```

### Config

```toml
[recording.sources]
voice = "Yeti Nano"      # optional, "default" or omitted = system default mic
call = "BlackHole 2ch"   # required for dual-source config mode, or "auto"
```

### Behavior

- `--call`
  - resolves to dual-source recording when a loopback/system-audio route is found
  - uses the default mic unless `--device` is supplied
- `--source a --source b`
  - `a` is the voice source
  - `b` is the call/system source
- `[recording.sources]`
  - dual-source mode only when `call` is present
  - `voice` missing means "default mic"

If the CLI cannot resolve a real call/system source, the user should see an explicit failure or explicit degraded-mode messaging. There should never be an ambiguous "maybe it worked" state.

## Design decisions

### 1. Mixed WAV is produced from the two live sources

The pipeline still wants one main audio path. We will continue to feed it a mixed WAV, but we will also persist separate stems.

Artifact set during capture:

- `current.wav` — mixed audio for transcription
- `current.voice.wav` — microphone stem
- `current.system.wav` — call/system-audio stem

### 2. Stems move with the queued job

Today `queue_live_capture` only moves the main WAV into the jobs directory. This change must also move:

- `job-id.voice.wav`
- `job-id.system.wav`

Otherwise diarization will never see the stems after handoff.

### 3. Final artifacts preserve stems alongside the transcript

When the queued job is preserved next to the final markdown artifact, the main audio plus stems should move together so reprocessing still works.

### 4. Coarse 100 ms slot alignment is acceptable for v1

The streaming runtime already works in ~100 ms chunks at 16 kHz mono. For CLI dual-source capture, we will align the two inputs into shared 100 ms slots and:

- write exact per-source chunks to the corresponding stem
- mix one chunk per slot into the main WAV
- fill missing chunks with silence

This is not sample-perfect DSP sync, but it is sufficient for:

- intelligible transcription
- robust stem-based attribution
- truthful capture of both sides of a call

If the slot model proves too lossy in practice, that is a quality follow-up, not a reason to keep shipping no CLI feature at all.

## Implementation plan

### A. CLI resolution

Update CLI config resolution so it can produce either:

- single-source recording config, or
- dual-source recording config

Rules:

1. `--source` repeated twice => dual-source config
2. `--call` + detected loopback => dual-source config with:
   - `voice = --device or default`
   - `call = detected loopback`
3. explicit `--device` without dual-source request => single-source config
4. existing dual-source config in `config.toml` => dual-source config

### B. Preflight

Teach capture preflight to reason about dual-source configs.

For call intent, preflight should pass when:

- the voice source is a mic/default mic
- the call source resolves to a known system-audio route

Preflight should block when:

- the call source does not resolve
- both sources resolve to the same device
- the requested call source looks like a plain mic rather than a loopback/system route

### C. Recording

Add a dual-source branch inside `record_to_wav`:

1. resolve actual voice + call devices
2. start one stream per device
3. write:
   - voice chunks to `*.voice.wav`
   - call chunks to `*.system.wav`
   - one mixed chunk per shared slot to `current.wav`
4. on stop:
   - finalize all writers
   - verify the mixed capture is non-empty

### D. Queue handoff / preservation

Extend jobs logic so the stem files travel with the main recording:

- `queue_live_capture` / `move_capture_into_job`
- `preserve_audio_alongside_output`

### E. Tests

Add focused tests for:

- CLI resolution
- dual-source preflight
- stem path movement in queued jobs
- stem preservation next to final artifacts

## Acceptance criteria

This work is done when all of the following are true:

1. `minutes record --source "Mic" --source "BlackHole 2ch"` produces:
   - `current.wav`
   - `current.voice.wav`
   - `current.system.wav`
2. queued processing sees the stems after handoff and can use stem-based diarization
3. final preserved artifacts keep the stems next to the transcript
4. `minutes record --call` on a Mac with a detectable loopback route records both sides
5. failure cases are explicit and honest

## Known tradeoffs

- The v1 mixer is intentionally conservative, not studio-grade.
- Reconnect behavior may restart both streams together rather than doing per-source surgical recovery.
- Live transcript sidecar quality during dual-source capture may lag behind single-source polish until a dedicated follow-up tunes it further.

That is acceptable. The current product gap is "feature absent." The right first durable ship is "feature present, honest, and operationally safe."
