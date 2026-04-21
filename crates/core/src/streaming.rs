use crate::error::CaptureError;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

// ──────────────────────────────────────────────────────────────
// Streaming audio capture — channel-based alternative to record_to_wav.
//
//   Microphone ──▶ cpal callback ──▶ mono 16kHz f32
//        │
//        ├──▶ AudioChunk channel (for VAD, whisper, or any consumer)
//        └──▶ audio level (atomic, for UI meter)
//
// The existing record_to_wav blocks and writes to a file.
// AudioStream is non-blocking: consumers pull chunks via a
// crossbeam channel at their own pace. If the channel fills,
// oldest chunks are dropped (bounded channel) — consumers
// need fresh data, not stale audio.
//
// Mono-downmix + decimation resampling is shared with capture.rs
// via `resample::build_resampled_input_stream`.
//
// MultiAudioStream wraps two AudioStreams for multi-source capture,
// tagging each chunk with its source role for speaker attribution.
// ──────────────────────────────────────────────────────────────

/// Which logical source produced a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRole {
    /// The user's microphone (voice).
    Voice,
    /// System/call audio (remote participants).
    Call,
    /// Single source (no multi-source capture).
    Default,
}

/// A chunk of 16kHz mono f32 audio samples (~100ms each).
#[derive(Clone)]
pub struct AudioChunk {
    /// 16kHz mono f32 samples, typically 1600 samples (100ms).
    pub samples: Vec<f32>,
    /// RMS energy of this chunk (0.0–1.0 scale).
    pub rms: f32,
    /// Wall-clock timestamp when this chunk was captured.
    pub timestamp: Instant,
    /// Monotonic per-stream chunk index (0, 1, 2, ...). Each AudioStream
    /// increments independently. Useful for debugging chunk ordering and
    /// stream-local diagnostics.
    pub index: u64,
    /// Which source produced this chunk.
    pub source: SourceRole,
}

/// Shared audio level (0–100) for UI visualization.
/// Separate from capture.rs AUDIO_LEVEL to allow both APIs to coexist.
static STREAM_AUDIO_LEVEL: AtomicU32 = AtomicU32::new(0);

/// Get the current streaming audio input level (0–100).
pub fn stream_audio_level() -> u32 {
    STREAM_AUDIO_LEVEL.load(Ordering::Relaxed)
}

// ──────────────────────────────────────────────────────────────
// Mic mute — Minutes-local toggle that drops the user's microphone
// from the recording while system audio keeps flowing. Only meaningful
// when dual-source capture is active; single-source mic recording with
// mute on would produce a silent file.
//
// AtomicBool is the fast per-process check. Cross-process signaling
// (CLI toggles a Tauri-initiated recording, or vice versa) goes through
// a sentinel file at ~/.minutes/mic_mute — record loops call
// `refresh_mic_mute_from_sentinel` once per iteration to sync the flag.
// ──────────────────────────────────────────────────────────────

static MIC_MUTED: AtomicBool = AtomicBool::new(false);

/// Returns true if the mic is currently muted (recording-local).
pub fn is_mic_muted() -> bool {
    MIC_MUTED.load(Ordering::Relaxed)
}

/// Set the in-process mic-mute flag. Does not touch the sentinel file.
/// Use `set_mic_muted_with_sentinel` when the change should also be
/// visible to other processes (Tauri vs CLI).
pub fn set_mic_muted(muted: bool) {
    MIC_MUTED.store(muted, Ordering::Relaxed);
}

/// Path to the mute sentinel file. Presence means "mic muted for the
/// current recording". Absence means normal capture.
pub fn mic_mute_sentinel_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".minutes")
        .join("mic_mute")
}

/// Set the mute flag and write/remove the sentinel so other processes
/// (e.g. the CLI toggling a Tauri recording) see the change. Returns
/// the new muted state. Emits a `MicMuted`/`MicUnmuted` event only when
/// the state actually changes. Failures to touch the sentinel or event
/// log are logged but non-fatal — the in-process flag is still updated.
///
/// "Previous state" is derived from the sentinel file, not the in-process
/// flag. This matters for short-lived CLI subcommands (`minutes mic-toggle`)
/// whose AtomicBool starts at false every invocation — without this, a
/// `mic-toggle --state off` right after a previous mute would not emit a
/// `MicUnmuted` event because the fresh process sees "false → false".
pub fn set_mic_muted_with_sentinel(muted: bool) -> bool {
    let path = mic_mute_sentinel_path();
    let previous = path.exists();
    MIC_MUTED.store(muted, Ordering::Relaxed);
    if muted {
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(error = %e, "failed to create mic_mute sentinel parent dir");
            }
        }
        if let Err(e) = std::fs::write(&path, b"") {
            tracing::warn!(error = %e, "failed to write mic_mute sentinel");
        }
    } else if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!(error = %e, "failed to remove mic_mute sentinel");
        }
    }
    if previous != muted {
        append_mic_mute_event(muted, "toggle");
    }
    muted
}

/// Toggle the mute flag (and sentinel) atomically. Returns the new state.
/// Uses sentinel presence as the source of truth so toggles work correctly
/// from fresh CLI subprocesses that don't have in-memory state.
pub fn toggle_mic_mute_with_sentinel() -> bool {
    let currently_muted = mic_mute_sentinel_path().exists();
    set_mic_muted_with_sentinel(!currently_muted)
}

fn append_mic_mute_event(muted: bool, source: &str) {
    let event = if muted {
        crate::events::MinutesEvent::MicMuted {
            source: source.to_string(),
        }
    } else {
        crate::events::MinutesEvent::MicUnmuted {
            source: source.to_string(),
        }
    };
    crate::events::append_event(event);
}

/// Sync the in-process flag to the sentinel file. Called once per
/// iteration of a record loop so CLI toggles reach Tauri-initiated
/// recordings (and vice versa). Absence of the sentinel always clears
/// the flag — there is no "mute without sentinel" state.
pub fn refresh_mic_mute_from_sentinel() {
    let present = mic_mute_sentinel_path().exists();
    MIC_MUTED.store(present, Ordering::Relaxed);
}

/// Clear both the in-process flag and the sentinel. Called at the start
/// of every new recording so mute state never leaks between sessions.
pub fn clear_mic_mute_for_new_recording() {
    MIC_MUTED.store(false, Ordering::Relaxed);
    let path = mic_mute_sentinel_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}

/// Handle to a running audio stream. Drop to stop capture.
pub struct AudioStream {
    _stream: cpal::Stream,
    stop: Arc<AtomicBool>,
    err_flag: Arc<AtomicBool>,
    /// Receive audio chunks from this channel.
    pub receiver: Receiver<AudioChunk>,
    /// The sample rate of output chunks (always 16000).
    pub sample_rate: u32,
    /// Name of the audio input device being used.
    pub device_name: String,
}

impl AudioStream {
    /// Start capturing from the specified (or default) input device.
    /// Returns a stream handle with a channel receiver for audio chunks.
    /// Chunks arrive at ~10Hz (100ms each at 16kHz = 1600 samples).
    pub fn start(device_override: Option<&str>) -> Result<Self, CaptureError> {
        let host = crate::capture::cached_default_host();
        let device = crate::capture::select_input_device(&host, device_override)?;

        // Bounded channel: 64 chunks = ~6.4 seconds of buffered audio.
        let (tx, rx): (Sender<AudioChunk>, Receiver<AudioChunk>) = bounded(64);

        let stop = Arc::new(AtomicBool::new(false));
        let err_flag = Arc::new(AtomicBool::new(false));
        let chunk_size: usize = 1600; // 100ms at 16kHz

        let mut chunk_buf: Vec<f32> = Vec::with_capacity(chunk_size);
        let mut chunk_counter: u64 = 0;

        let (stream, device_name, _config) = crate::resample::build_resampled_input_stream(
            &device,
            &stop,
            &err_flag,
            move |resampled: &[f32]| {
                for &sample in resampled {
                    chunk_buf.push(sample);

                    if chunk_buf.len() >= chunk_size {
                        let samples: Vec<f32> = chunk_buf.drain(..chunk_size).collect();
                        let rms = compute_rms(&samples);
                        let level = (rms * 2000.0).min(100.0) as u32;
                        STREAM_AUDIO_LEVEL.store(level, Ordering::Relaxed);
                        let idx = chunk_counter;
                        chunk_counter += 1;
                        let _ = tx.try_send(AudioChunk {
                            samples,
                            rms,
                            timestamp: Instant::now(),
                            index: idx,
                            source: SourceRole::Default,
                        });
                    }
                }
            },
        )?;

        tracing::info!(device = %device_name, "streaming audio capture started");

        Ok(AudioStream {
            _stream: stream,
            stop,
            err_flag,
            receiver: rx,
            sample_rate: 16000,
            device_name,
        })
    }

    /// Returns true if the audio stream has encountered an error.
    pub fn has_error(&self) -> bool {
        self.err_flag.load(Ordering::Relaxed)
    }

    /// Stop the audio stream.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for AudioStream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Handle to two running audio streams (voice + call) for multi-source capture.
/// Produces tagged chunks from both sources on a single merged receiver.
pub struct MultiAudioStream {
    voice: AudioStream,
    call: AudioStream,
    _merge_thread: std::thread::JoinHandle<()>,
    stop: Arc<AtomicBool>,
    /// Receive tagged audio chunks from both sources.
    pub receiver: Receiver<AudioChunk>,
}

impl MultiAudioStream {
    /// Start capturing from two devices: one for voice (microphone) and one for
    /// call/system audio. Chunks from both sources arrive on a single receiver,
    /// tagged with their `SourceRole`.
    pub fn start(voice_device: Option<&str>, call_device: &str) -> Result<Self, CaptureError> {
        let voice = AudioStream::start(voice_device)?;
        let call = AudioStream::start(Some(call_device))?;

        let (tx, rx): (Sender<AudioChunk>, Receiver<AudioChunk>) = bounded(128);
        let stop = Arc::new(AtomicBool::new(false));

        let voice_rx = voice.receiver.clone();
        let call_rx = call.receiver.clone();
        let stop_clone = Arc::clone(&stop);
        let tx_clone = tx.clone();

        let merge_thread = std::thread::spawn(move || {
            let timeout = std::time::Duration::from_millis(50);
            while !stop_clone.load(Ordering::Relaxed) {
                // Drain voice chunks. If mic is muted, zero the samples in
                // place so downstream timing stays intact (slot alignment,
                // stem writers) but no voice energy reaches the transcript.
                while let Ok(mut chunk) = voice_rx.try_recv() {
                    chunk.source = SourceRole::Voice;
                    if MIC_MUTED.load(Ordering::Relaxed) {
                        for s in chunk.samples.iter_mut() {
                            *s = 0.0;
                        }
                        chunk.rms = 0.0;
                    }
                    let _ = tx.try_send(chunk);
                }
                // Drain call chunks — always forwarded regardless of mute
                // (the whole point is to keep system audio flowing).
                while let Ok(mut chunk) = call_rx.try_recv() {
                    chunk.source = SourceRole::Call;
                    let _ = tx_clone.try_send(chunk);
                }
                std::thread::sleep(timeout);
            }
        });

        tracing::info!(
            voice = %voice.device_name,
            call = %call.device_name,
            "multi-source audio capture started"
        );

        Ok(MultiAudioStream {
            voice,
            call,
            _merge_thread: merge_thread,
            stop,
            receiver: rx,
        })
    }

    /// Returns true if either audio stream has encountered an error.
    pub fn has_error(&self) -> bool {
        self.voice.has_error() || self.call.has_error()
    }

    /// Name of the voice (microphone) device.
    pub fn voice_device_name(&self) -> &str {
        &self.voice.device_name
    }

    /// Name of the call (system audio) device.
    pub fn call_device_name(&self) -> &str {
        &self.call.device_name
    }
}

impl Drop for MultiAudioStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.voice.stop();
        self.call.stop();
    }
}

#[cfg(test)]
mod mic_mute_tests {
    use super::*;
    use std::sync::Mutex;

    // Tests in this module mutate the process-global MIC_MUTED flag and the
    // sentinel file on disk. Serialize them so parallel test runs don't stomp
    // on each other's assertions.
    static GUARD: Mutex<()> = Mutex::new(());

    fn reset() {
        let _ = std::fs::remove_file(mic_mute_sentinel_path());
        MIC_MUTED.store(false, Ordering::Relaxed);
    }

    #[test]
    fn set_and_read_flag() {
        let _g = GUARD.lock().unwrap();
        reset();
        assert!(!is_mic_muted());
        set_mic_muted(true);
        assert!(is_mic_muted());
        set_mic_muted(false);
        assert!(!is_mic_muted());
        reset();
    }

    #[test]
    fn sentinel_round_trip() {
        let _g = GUARD.lock().unwrap();
        reset();
        assert!(!mic_mute_sentinel_path().exists());
        set_mic_muted_with_sentinel(true);
        assert!(is_mic_muted());
        assert!(mic_mute_sentinel_path().exists());
        set_mic_muted_with_sentinel(false);
        assert!(!is_mic_muted());
        assert!(!mic_mute_sentinel_path().exists());
        reset();
    }

    #[test]
    fn refresh_syncs_from_sentinel() {
        let _g = GUARD.lock().unwrap();
        reset();
        // Sentinel absent -> flag cleared
        MIC_MUTED.store(true, Ordering::Relaxed);
        refresh_mic_mute_from_sentinel();
        assert!(!is_mic_muted());
        // Sentinel present -> flag set
        std::fs::create_dir_all(mic_mute_sentinel_path().parent().unwrap()).unwrap();
        std::fs::write(mic_mute_sentinel_path(), b"").unwrap();
        refresh_mic_mute_from_sentinel();
        assert!(is_mic_muted());
        reset();
    }

    #[test]
    fn clear_for_new_recording_removes_sentinel_and_flag() {
        let _g = GUARD.lock().unwrap();
        reset();
        set_mic_muted_with_sentinel(true);
        assert!(is_mic_muted());
        assert!(mic_mute_sentinel_path().exists());
        clear_mic_mute_for_new_recording();
        assert!(!is_mic_muted());
        assert!(!mic_mute_sentinel_path().exists());
        reset();
    }

    #[test]
    fn toggle_flips_state() {
        let _g = GUARD.lock().unwrap();
        reset();
        assert!(toggle_mic_mute_with_sentinel());
        assert!(is_mic_muted());
        assert!(!toggle_mic_mute_with_sentinel());
        assert!(!is_mic_muted());
        reset();
    }
}
