use crate::error::CaptureError;
use cpal::traits::{DeviceTrait, StreamTrait};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

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
// Both APIs share the same cpal + resampling logic. Eventually
// record_to_wav can be reimplemented on top of AudioStream
// (DRY consolidation).
// ──────────────────────────────────────────────────────────────

/// A chunk of 16kHz mono f32 audio samples (~100ms each).
#[derive(Clone)]
pub struct AudioChunk {
    /// 16kHz mono f32 samples, typically 1600 samples (100ms).
    pub samples: Vec<f32>,
    /// RMS energy of this chunk (0.0–1.0 scale).
    pub rms: f32,
}

/// Shared audio level (0–100) for UI visualization.
/// Separate from capture.rs AUDIO_LEVEL to allow both APIs to coexist.
static STREAM_AUDIO_LEVEL: AtomicU32 = AtomicU32::new(0);

/// Get the current streaming audio input level (0–100).
pub fn stream_audio_level() -> u32 {
    STREAM_AUDIO_LEVEL.load(Ordering::Relaxed)
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
        let host = cpal::default_host();
        let device = crate::capture::select_input_device(&host, device_override)?;

        let device_name = device.name().unwrap_or_else(|_| "unknown".into());
        let config = device
            .default_input_config()
            .map_err(|e| CaptureError::Io(std::io::Error::other(format!("input config: {}", e))))?;

        let native_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let ratio = native_rate as f64 / 16000.0;

        // Bounded channel: 64 chunks = ~6.4 seconds of buffered audio.
        let (tx, rx): (Sender<AudioChunk>, Receiver<AudioChunk>) = bounded(64);

        let stop = Arc::new(AtomicBool::new(false));
        let err_flag = Arc::new(AtomicBool::new(false));
        let chunk_size: usize = 1600; // 100ms at 16kHz

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let mut resample_buf: Vec<f32> = Vec::new();
                let mut resample_pos: f64 = 0.0;
                let mut chunk_buf: Vec<f32> = Vec::with_capacity(chunk_size);
                let tx = tx.clone();
                let stop_clone = Arc::clone(&stop);
                let err_flag_clone = Arc::clone(&err_flag);

                device
                    .build_input_stream(
                        &config.into(),
                        move |data: &[f32], _: &cpal::InputCallbackInfo| {
                            if stop_clone.load(Ordering::Relaxed) {
                                return;
                            }

                            // Mix to mono
                            for frame in data.chunks(channels) {
                                let mono: f32 = frame.iter().sum::<f32>() / channels as f32;
                                resample_buf.push(mono);
                            }

                            // Resample to 16kHz
                            while resample_pos < resample_buf.len() as f64 {
                                let idx = resample_pos as usize;
                                if idx < resample_buf.len() {
                                    chunk_buf.push(resample_buf[idx]);
                                }
                                resample_pos += ratio;

                                if chunk_buf.len() >= chunk_size {
                                    let samples: Vec<f32> = chunk_buf.drain(..chunk_size).collect();
                                    let rms = compute_rms(&samples);
                                    let level = (rms * 2000.0).min(100.0) as u32;
                                    STREAM_AUDIO_LEVEL.store(level, Ordering::Relaxed);
                                    let _ = tx.try_send(AudioChunk { samples, rms });
                                }
                            }

                            let consumed = (resample_pos as usize).min(resample_buf.len());
                            if consumed > 0 {
                                resample_buf.drain(..consumed);
                                resample_pos -= consumed as f64;
                            }
                        },
                        move |err| {
                            tracing::error!("streaming audio error: {}", err);
                            err_flag_clone.store(true, Ordering::Relaxed);
                        },
                        None,
                    )
                    .map_err(|e| {
                        CaptureError::Io(std::io::Error::other(format!("build stream: {}", e)))
                    })?
            }
            cpal::SampleFormat::I16 => {
                let mut resample_buf: Vec<f32> = Vec::new();
                let mut resample_pos: f64 = 0.0;
                let mut chunk_buf: Vec<f32> = Vec::with_capacity(chunk_size);
                let tx = tx.clone();
                let stop_clone = Arc::clone(&stop);
                let err_flag_clone = Arc::clone(&err_flag);

                device
                    .build_input_stream(
                        &config.into(),
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            if stop_clone.load(Ordering::Relaxed) {
                                return;
                            }

                            for frame in data.chunks(channels) {
                                let mono: f32 =
                                    frame.iter().map(|&s| s as f32 / 32768.0).sum::<f32>()
                                        / channels as f32;
                                resample_buf.push(mono);
                            }

                            while resample_pos < resample_buf.len() as f64 {
                                let idx = resample_pos as usize;
                                if idx < resample_buf.len() {
                                    chunk_buf.push(resample_buf[idx]);
                                }
                                resample_pos += ratio;

                                if chunk_buf.len() >= chunk_size {
                                    let samples: Vec<f32> = chunk_buf.drain(..chunk_size).collect();
                                    let rms = compute_rms(&samples);
                                    let level = (rms * 2000.0).min(100.0) as u32;
                                    STREAM_AUDIO_LEVEL.store(level, Ordering::Relaxed);
                                    let _ = tx.try_send(AudioChunk { samples, rms });
                                }
                            }

                            let consumed = (resample_pos as usize).min(resample_buf.len());
                            if consumed > 0 {
                                resample_buf.drain(..consumed);
                                resample_pos -= consumed as f64;
                            }
                        },
                        move |err| {
                            tracing::error!("streaming audio error: {}", err);
                            err_flag_clone.store(true, Ordering::Relaxed);
                        },
                        None,
                    )
                    .map_err(|e| {
                        CaptureError::Io(std::io::Error::other(format!("build stream: {}", e)))
                    })?
            }
            fmt => {
                return Err(CaptureError::Io(std::io::Error::other(format!(
                    "unsupported format: {:?}",
                    fmt
                ))));
            }
        };

        stream
            .play()
            .map_err(|e| CaptureError::Io(std::io::Error::other(format!("play: {}", e))))?;

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
