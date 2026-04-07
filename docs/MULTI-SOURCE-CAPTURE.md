# Multi-Source Capture

> Addendum to [Call Capture Durable Plan](CALL-CAPTURE-DURABLE-PLAN.md) for [#34](https://github.com/silverstein/minutes/issues/34).
> Adds CLI multi-source capture, stem-based speaker attribution, and live mode speaker tags.

**Status:** Reviewed (eng review + codex outside voice, 2026-03-31)
**Author:** @silverstein
**Date:** 2026-03-31

---

## Relationship to Call Capture Durable Plan

This is **not** a standalone architecture. It extends the existing Call Capture plan, which already shipped:

- `RecordingIntent` (Memo/Room/Call) in `capture.rs`
- `CapturePreflight` with blocking degraded-mode policy
- ScreenCaptureKit native macOS system audio capture in `call_capture.rs`
- `CallSourceHealth` with mic_live / call_audio_live tracking
- Intent parsing in CLI (`--intent call`) and Tauri (call detection banner)
- MCP delegation to desktop app for call intent

**The user-facing abstraction remains `call`, not "select two devices."** This addendum adds:

1. Source-aware artifacts for better speaker attribution
2. Energy-based stem attribution in `diarize.rs` (skip pyannote for 1-on-1 calls)
3. CLI `--source` as an escape hatch for unusual setups
4. `minutes sources` as a diagnostic command
5. `MultiAudioStream` for live mode speaker tags
6. Linux PipeWire source-preserving backend (after macOS is proven)

## Problem

When mic and call audio are captured as separate streams, speaker attribution becomes trivial for 1-on-1 calls. The existing ScreenCaptureKit helper already captures both sources. What's missing is **using that separation** for better transcripts.

Secondary: Linux users (issue #34) want multi-device recording without manually creating virtual sources.

## Product contract

The product surface is intent-driven. `--call` is the primary UX. `--source` is an escape hatch for unusual setups (not "power user" -- stock Linux with PipeWire is a normal setup, not a power-user scenario).

```bash
# Primary UX (intent-driven, unchanged from durable plan)
minutes record --intent call          # Tauri routes through ScreenCaptureKit
minutes record --call                 # shorthand, auto-detects loopback on all platforms

# Escape hatch for unusual setups
minutes record --source "Yeti" --source "BlackHole 2ch"

# Diagnostic command (new)
minutes sources                       # categorized device list
```

`minutes sources` shows devices grouped by category (Microphones / System Audio / Virtual Devices). It's for debugging and initial setup, not everyday use.

## Architecture

### Shared source contract

The architecture should be source-aware internally even when the user-facing action is just `minutes record --call`.

Every backend should produce one of these:

- explicit per-source stems plus timing metadata, or
- timestamped source frames that can later be written as stems

That gives live mode and batch mode the same foundation.

```rust
struct SourceFrame {
    source_id: SourceId,
    source_kind: LogicalSourceKind, // Mic | SystemAudio | VirtualCombined | Experimental
    samples: Vec<f32>,
    sample_rate: u32,
    captured_at: CaptureTimestamp,
}
```

User-facing source selection may still use names, but backend bindings should prefer stable device IDs where the platform or library exposes them.

### Source artifacts and attribution (batch recording)

After recording stops, the pipeline should work from explicit source-aware artifacts:

```
capture (existing)
    │
    ▼
source-aware backend output
    │
    ├──► explicit stems from backend, or
    └──► a recorded artifact that is proven to preserve separable tracks/channels
    ▼
stem materialization (NEW)
    │
    ├──► voice.wav   (stem)
    ├──► call.wav    (stem)
    └──► mixed.wav   (for whisper — sum of both)
    │
    ▼
transcribe(mixed.wav)  — unchanged interface
    │
    ▼
diarize(mixed.wav, stems: Some(&stem_paths))  — extended interface
    │
    ├── stems present → energy-based attribution (no ML)
    └── stems absent  → pyannote (existing behavior)
    │
    ▼
pipeline continues (summarize, markdown) — unchanged
```

Important constraint: the current macOS helper writes a single `.mov` via `SCRecordingOutput`, but this document should not assume that file already preserves microphone and system audio as separable tracks. Before building attribution on top of post-capture splitting, we must either:

- prove the recorded artifact preserves distinct tracks/channels in a way we can reliably extract, or
- change the native backend to write explicit stems directly from the live callbacks

If a backend only emits a mixed artifact, it should be treated as mixed capture, not source-aware capture.

`ffmpeg` is still useful for materializing stems when the backend output is already proven separable. It is not the source of truth for separation by itself.

**ffmpeg fallback**: If `ffmpeg` isn't installed and the backend did not already write stems, skip stem materialization and proceed with the single-file pipeline (existing behavior). Log a warning: "Install ffmpeg for per-speaker stem extraction when using separable tracks."

Stems are a best-effort enhancement, never a hard requirement.

### Stem-based speaker attribution

Extend `diarize()` with an optional `stems` parameter:

```rust
// diarize.rs
pub fn diarize(
    audio_path: &Path,
    stems: Option<&StemPaths>,  // NEW
    config: &Config,
) -> Option<DiarizationResult> {
    if let Some(stems) = stems {
        return diarize_from_stems(stems);
    }
    // existing pyannote path unchanged...
}
```

Energy comparison per whisper segment:
- voice.wav has energy → "You" (or configured identity name)
- call.wav has energy → "Remote"
- Both have energy → dominant source wins (lossy, known limitation)
- Neither has energy → skip (leave unlabeled)

**Known limitations:**
- Sidetone bleed: your voice may appear in the call stem via speaker playback. This degrades attribution quality in some setups.
- Whisper segments (~30s) may span speaker boundaries. Per-segment attribution is coarse.
- "Strong for clean 1-on-1 setups, degrades with sidetone bleed and overlapping speech." Not "near-perfect."
- For group calls: run pyannote on the call stem only (fewer speakers, cleaner audio than the full mix).

| Scenario | Method | Cost |
|----------|--------|------|
| 1-on-1 call, mic + loopback | Energy comparison on stems | Zero ML |
| Group call, mic + loopback | Stems for "you" vs "them"; pyannote on call stem only | Reduced ML |
| In-person meeting, single mic | Pyannote on full audio (unchanged) | Full ML |
| Single device recording | No change to current behavior | Current behavior |

### Live mode: MultiAudioStream

**Prerequisite:** Add timing metadata to `AudioChunk` in `streaming.rs`. Without timestamps, two interleaved independent streams can't produce stable speaker tags.

```rust
// streaming.rs
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub rms: f32,
    pub timestamp: Instant,  // NEW — capture time for attribution windows
}

pub struct TaggedChunk {
    pub chunk: AudioChunk,
    pub source: SourceRole,  // Voice | Call
}
```

`MultiAudioStream` wraps two `AudioStream` instances. Consumers receive `TaggedChunk` from a merged channel. The live transcript loop uses source tags to write speaker labels into JSONL:

```jsonl
{"ts": "0:42", "speaker": "You", "text": "I think we should..."}
{"ts": "0:45", "speaker": "Remote", "text": "Agreed, let me check..."}
```

`Instant` alone is not the full synchronization model. The implementation also needs:

- a consistent transcript time base
- attribution windows shared across both sources
- explicit fallback behavior when independent devices drift
- deterministic handling when both sources are active inside one whisper segment

For the fallback path, live speaker tags should be considered best-effort until the sync windowing logic is proven in dogfood.

### DRY prerequisite: shared stream builder

Before building `MultiAudioStream`, consolidate the duplicated mono-downmix and resampling code between `capture.rs:build_capture_stream` and `streaming.rs:AudioStream::start` into a shared `build_resampled_input_stream()` function.

### Output files

```
~/meetings/2026-03-31_standup.md          # transcript (unchanged)
~/meetings/2026-03-31_standup.wav         # mixed audio (unchanged)
~/meetings/2026-03-31_standup.voice.wav   # voice stem (new, optional)
~/meetings/2026-03-31_standup.call.wav    # call stem (new, optional)
```

## Config

```toml
# ~/.config/minutes/config.toml

[recording]
device = "Yeti Nano"             # existing — single device, backwards compat

[recording.sources]              # new — multi-source mode
voice = "Yeti Nano"              # or "default"
call = "BlackHole 2ch"           # or "auto" (detect loopback devices)
```

**Precedence:** CLI flags > `[recording.sources]` > `device`. If `sources` is present, `device` is ignored.

Longer-term, source config should resolve to stable device IDs in persisted runtime metadata even if the user picked the source by display name.

The `--call` CLI flag is equivalent to setting `call = "auto"`.

### Auto-detection of loopback devices

`call = "auto"` scans `host.input_devices()` for the first device the
categorizer flags as `SystemAudio`:

- **macOS**: virtual loopback drivers — BlackHole, Loopback, Soundflower, MMAudio,
  LoomAudioDevice, ZoomAudioDevice, Microsoft Teams Audio (see
  `crates/core/src/capture.rs::is_system_audio_device_name` for the full list)
- **Linux / PipeWire**: cpal exposes `Audio/Sink` nodes (your speakers and
  headphones) with `direction = Duplex`, so they appear in `host.input_devices()`
  automatically. The categorizer flags any device where
  `supports_input() && supports_output()` and the host is PipeWire as
  `SystemAudio`. Recording from one transparently uses the sink's monitor port
  via the `STREAM_CAPTURE_SINK` PipeWire stream property at stream creation
  (cpal handles this internally — see `cpal/src/host/pipewire/device.rs:133-135`).
- **Linux / PulseAudio**: monitor sources are exposed as separate `Source`
  devices with names ending in `.monitor`, caught by the name heuristic.
- **Linux / ALSA-only**: no system audio capture available without installing a
  loopback module.

If no system audio device is found:

```
No system audio device detected.

To capture call audio:
  macOS:           brew install blackhole-2ch
  Linux PipeWire:  your speakers should appear automatically — check `wpctl status`
  Linux PulseAudio: monitor sources should appear automatically — check `pactl list sources`
  Linux ALSA-only: install pipewire or pulseaudio for monitor capture

Or use the Minutes desktop app for native call capture (no driver needed).
```

## macOS backend strategy

Two backends, one interface. Both produce audio that the stem-splitting pipeline can consume:

1. **ScreenCaptureKit helper** (existing, Tauri-owned) -- captures mic + system audio natively, no BlackHole needed. Outputs .mov. Primary path for desktop users.

   Before downstream stem attribution depends on this path, we must verify whether the `.mov` preserves separable microphone/system tracks. If not, the helper should be extended to write stems directly.

2. **Manual BlackHole + `--source`** (CLI power-user path) -- user installs BlackHole, uses `--source` to specify both devices. Minutes records via cpal. Fallback for headless/CLI-only users.

CoreAudio aggregate device creation (programmatic) deferred to v2. The ScreenCaptureKit path already solves "just works" for Tauri. Aggregate device would help CLI users avoid manual BlackHole setup, but it's ~400 lines of unsafe CoreAudio code and the user base for CLI-only call capture is small.

## Linux backend strategy

Detect PipeWire at runtime. The target backend should preserve source identity, not just produce one mixed source.

Preferred v2 shape:

- create a PipeWire-backed virtual source or multichannel route that preserves microphone vs call-audio channel mapping
- keep timing under PipeWire's clock domain
- record from that route while still materializing source-aware artifacts

If the only practical Linux backend we can build in the first pass is a mixed combined source, we should describe it honestly as a setup simplification backend, not a true source-aware attribution backend.

PulseAudio fallback: `pactl load-module module-combine-source`.

**Sequencing:** Linux support ships after macOS multi-source is proven and dogfooded.

## Windows backend strategy

Windows is deferred, not blocked. The likely future backend is:

- microphone capture + WASAPI loopback on the render endpoint for system/call audio

That gives the architecture a viable Windows path without making it part of v1.

## Codebase changes

Ordered by dependency (build bottom-up):

| Layer | File | Change |
|-------|------|--------|
| **DRY** | `capture.rs`, `streaming.rs` | Extract shared `build_resampled_input_stream()` |
| **Config** | `config.rs` | Add `SourcesConfig { voice: Option<String>, call: Option<String> }` |
| **Device** | `capture.rs` | New `select_loopback_device()`, `list_devices_categorized()` |
| **Pipeline** | `pipeline.rs` | Add `PipelineStage::SplittingStems`, ffmpeg stem split step |
| **Diarize** | `diarize.rs` | Add optional `stems: Option<&StemPaths>` parameter, `diarize_from_stems()` |
| **Streaming** | `streaming.rs` | Add `timestamp` to `AudioChunk`, build `MultiAudioStream` |
| **Live** | `live_transcript.rs` | Use `TaggedChunk` source for speaker labels in JSONL |
| **CLI** | `main.rs` | `--source` (repeatable), `minutes sources` subcommand |
| **Monitor** | `device_monitor.rs` | Per-device monitoring (`kAudioDevicePropertyDeviceIsAlive`) |
| **Preflight** | `capture.rs` | Add `stems_available` to `CapturePreflight` |
| **Backend** | `system_audio_record.swift` or native helper layer | Verify separable tracks or write explicit stems directly |

### Files that don't change

- `transcribe.rs` -- receives `mixed.wav` by default; per-stem transcription (v1 step 5) adds an optional multi-file path but the single-file interface is unchanged
- `summarize.rs` -- receives attributed transcript
- `markdown.rs` -- speaker labels already flow through
- `whisper-guard/` -- operates on each audio input (mixed or per-stem)
- `call_capture.rs` -- ScreenCaptureKit helper unchanged

## Clock drift

Only relevant for the fallback path (two independent cpal streams). The ScreenCaptureKit and PipeWire paths should keep both sources under a shared backend clock domain when implemented correctly.

| Recording length | Max drift | Perceptible? |
|-----------------|-----------|--------------|
| 30 min | ~25ms | No |
| 1 hour | ~50ms | Barely |
| 2 hours | ~100ms | Maybe |

For the fallback path: linear resample after recording may be good enough for batch mode, but live attribution should remain conservative until sync windows are proven.

## Rollout

### v1: Stem splitting + attribution + live speaker tags

**Sequence:** macOS stems and attribution first. PipeWire device auto-detection ships alongside (not after) macOS, since the desktop app is confirmed working on Linux (Discussion #44). Full source-aware attribution pipeline on Linux ships after macOS dogfood.

1. DRY consolidation (shared resampler)
2. Prove ScreenCaptureKit artifact separability or extend the helper to write explicit stems
3. Stem materialization path in pipeline (`ffmpeg` only when artifact is already separable)
4. Energy-based stem attribution in `diarize.rs`
5. Per-stem transcription path (transcribe each stem separately, merge by timestamp, compare quality against mixed -- promote to default if quality proves out) ([Discussion #43](https://github.com/silverstein/minutes/discussions/43))
6. `AudioChunk` timestamps + `MultiAudioStream`
7. Sync windowing for live speaker tags from `TaggedChunk`
8. Config `[recording.sources]` + CLI `--source` + `minutes sources`
9. PipeWire `*.monitor` source auto-detection for `--call` on Linux ([#62](https://github.com/silverstein/minutes/issues/62))
10. Per-device `DeviceMonitor`

### v2: Aggregate device + full Linux attribution

- CoreAudio aggregate device creation (programmatic, no BlackHole needed for CLI)
- Linux PipeWire source-preserving backend (full stem-based attribution)
- PulseAudio fallback

### v3: Arbitrary multi-source + smart defaults

- `--source` repeated N times
- Per-source gain, label, role config
- Auto-detect active call app for CLI
- Suggest loopback driver installation on first run

## NOT in scope (v1)

- CoreAudio aggregate device creation (deferred to v2, ~400 lines unsafe code)
- Full Linux PipeWire source-aware attribution (deferred to v2; PipeWire device auto-detection is v1)
- Arbitrary N-device capture (v3)
- Echo cancellation / sidetone cleanup (post-launch quality)
- Windows system audio capture implementation (backend deferred, likely WASAPI loopback)
- Per-source gain/label config (v3)

## References

- [cpal multi-device threading](https://docs.rs/cpal/latest/cpal/)
- [CPAL `DeviceTrait`](https://docs.rs/cpal/latest/cpal/traits/trait.DeviceTrait.html) -- stable device IDs and richer device metadata
- [CPAL `InputStreamTimestamp`](https://docs.rs/cpal/latest/aarch64-apple-ios/cpal/struct.InputStreamTimestamp.html) -- input capture vs callback timing
- [cubeb-coreaudio-rs](https://github.com/mozilla/cubeb-coreaudio-rs) -- Mozilla's Rust aggregate device implementation (v2 reference)
- [PipeWire combine-stream](https://docs.pipewire.org/page_module_combine_stream.html) -- virtual combined source (v2 reference only; preserve channel/source mapping explicitly)
- [PipeWire `pw_time`](https://pipewire.pages.freedesktop.org/pipewire/structpw__time.html) -- PipeWire timing model for synchronization
- [Microsoft Learn: Loopback Recording](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording) -- supported Windows system-audio capture path
- [Clock drift in multimedia](https://protyposis.net/clockdrift/)
- [AssemblyAI multichannel diarization](https://www.assemblyai.com/blog/multichannel-speaker-diarization)
