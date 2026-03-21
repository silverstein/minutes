use crate::config::Config;
use crate::error::CaptureError;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

/// Shared audio level (0–100 scale) for UI visualization.
/// Updated ~10x per second from the cpal callback.
static AUDIO_LEVEL: AtomicU32 = AtomicU32::new(0);

/// Get the current audio input level (0–100).
pub fn audio_level() -> u32 {
    AUDIO_LEVEL.load(Ordering::Relaxed)
}

// ──────────────────────────────────────────────────────────────
// Audio capture using cpal (cross-platform audio I/O).
//
// Two modes:
//   1. Default input device (built-in mic) — works out of the box
//      Good for: voice memos, in-person meetings
//   2. BlackHole virtual audio device — captures system audio
//      Good for: Zoom/Meet/Teams calls
//      Requires: brew install blackhole-2ch + Multi-Output Device setup
//
// The recording runs as a foreground process. On SIGTERM/SIGINT:
//   stop capture → flush WAV → run pipeline → clean up → exit
// ──────────────────────────────────────────────────────────────

/// Start recording audio from the default input device.
/// Blocks until `stop_flag` is set to true (via signal handler).
/// Writes raw PCM to a WAV file at the given path.
/// If screen context is enabled, also captures periodic screenshots.
pub fn record_to_wav(
    output_path: &Path,
    stop_flag: Arc<AtomicBool>,
    config: &Config,
) -> Result<(), CaptureError> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    // Get the default input device
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or(CaptureError::DeviceNotFound)?;

    let device_name = device.name().unwrap_or_else(|_| "unknown".into());
    eprintln!("[minutes] Using input device: {}", device_name);
    tracing::info!(device = %device_name, "using audio input device");

    // Get the default input config
    let supported_config = device
        .default_input_config()
        .map_err(|e| CaptureError::Io(std::io::Error::other(format!("input config: {}", e))))?;

    let sample_rate = supported_config.sample_rate().0;
    let channels = supported_config.channels();
    tracing::info!(
        sample_rate,
        channels,
        format = ?supported_config.sample_format(),
        "audio capture config"
    );

    // Create WAV writer — always write as 16kHz mono 16-bit for whisper
    // We'll downsample in real-time during capture
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let wav_spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let writer = hound::WavWriter::create(output_path, wav_spec)
        .map_err(|e| CaptureError::Io(std::io::Error::other(format!("WAV create: {}", e))))?;
    let writer = Arc::new(std::sync::Mutex::new(Some(writer)));

    // Set up the resampler state
    let ratio = sample_rate as f64 / 16000.0;
    let writer_clone = Arc::clone(&writer);
    let stop_clone = Arc::clone(&stop_flag);
    let sample_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let sample_count_clone = Arc::clone(&sample_count);

    // Build the input stream
    let err_flag = Arc::new(AtomicBool::new(false));
    let err_flag_clone = Arc::clone(&err_flag);

    // Reset audio level
    AUDIO_LEVEL.store(0, Ordering::Relaxed);

    let stream = match supported_config.sample_format() {
        cpal::SampleFormat::F32 => {
            let ch = channels as usize;
            let mut resample_pos: f64 = 0.0;
            let mut input_samples: Vec<f32> = Vec::new();
            let mut level_accum: f64 = 0.0;
            let mut level_count: u32 = 0;
            let level_interval = (sample_rate / 10) as u32; // ~10 updates/sec

            device
                .build_input_stream(
                    &supported_config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if stop_clone.load(Ordering::Relaxed) {
                            return;
                        }

                        // Mix to mono, compute RMS for level meter
                        for chunk in data.chunks(ch) {
                            let mono: f32 = chunk.iter().sum::<f32>() / ch as f32;
                            input_samples.push(mono);
                            level_accum += (mono as f64) * (mono as f64);
                            level_count += 1;
                            if level_count >= level_interval {
                                let rms = (level_accum / level_count as f64).sqrt();
                                // Scale to 0-100 (raw mic levels are low, ~0.001–0.05)
                                let level = (rms * 2000.0).min(100.0) as u32;
                                AUDIO_LEVEL.store(level, Ordering::Relaxed);
                                level_accum = 0.0;
                                level_count = 0;
                            }
                        }

                        // Downsample to 16kHz using simple decimation with averaging
                        let mut guard = writer_clone.lock().unwrap();
                        if let Some(ref mut w) = *guard {
                            while resample_pos < input_samples.len() as f64 {
                                let idx = resample_pos as usize;
                                if idx < input_samples.len() {
                                    let sample = (input_samples[idx] * 32767.0)
                                        .clamp(-32768.0, 32767.0)
                                        as i16;
                                    if w.write_sample(sample).is_err() {
                                        return;
                                    }
                                    sample_count_clone.fetch_add(1, Ordering::Relaxed);
                                }
                                resample_pos += ratio;
                            }
                            // Keep remainder for next callback
                            let consumed = resample_pos as usize;
                            if consumed > 0 && consumed <= input_samples.len() {
                                input_samples.drain(..consumed);
                                resample_pos -= consumed as f64;
                            }
                        }
                    },
                    move |err| {
                        tracing::error!("audio stream error: {}", err);
                        err_flag_clone.store(true, Ordering::Relaxed);
                    },
                    None,
                )
                .map_err(|e| {
                    CaptureError::Io(std::io::Error::other(format!("build stream: {}", e)))
                })?
        }
        cpal::SampleFormat::I16 => {
            let ch = channels as usize;
            let mut resample_pos: f64 = 0.0;
            let mut input_samples: Vec<f32> = Vec::new();
            let mut level_accum: f64 = 0.0;
            let mut level_count: u32 = 0;
            let level_interval = (sample_rate / 10) as u32;

            device
                .build_input_stream(
                    &supported_config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if stop_clone.load(Ordering::Relaxed) {
                            return;
                        }

                        for chunk in data.chunks(ch) {
                            let mono: f32 =
                                chunk.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / ch as f32;
                            input_samples.push(mono);
                            level_accum += (mono as f64) * (mono as f64);
                            level_count += 1;
                            if level_count >= level_interval {
                                let rms = (level_accum / level_count as f64).sqrt();
                                let level = (rms * 300.0).min(100.0) as u32;
                                AUDIO_LEVEL.store(level, Ordering::Relaxed);
                                level_accum = 0.0;
                                level_count = 0;
                            }
                        }

                        let mut guard = writer_clone.lock().unwrap();
                        if let Some(ref mut w) = *guard {
                            while resample_pos < input_samples.len() as f64 {
                                let idx = resample_pos as usize;
                                if idx < input_samples.len() {
                                    let sample = (input_samples[idx] * 32767.0)
                                        .clamp(-32768.0, 32767.0)
                                        as i16;
                                    if w.write_sample(sample).is_err() {
                                        return;
                                    }
                                    sample_count_clone.fetch_add(1, Ordering::Relaxed);
                                }
                                resample_pos += ratio;
                            }
                            let consumed = resample_pos as usize;
                            if consumed > 0 && consumed <= input_samples.len() {
                                input_samples.drain(..consumed);
                                resample_pos -= consumed as f64;
                            }
                        }
                    },
                    move |err| {
                        tracing::error!("audio stream error: {}", err);
                        err_flag_clone.store(true, Ordering::Relaxed);
                    },
                    None,
                )
                .map_err(|e| {
                    CaptureError::Io(std::io::Error::other(format!("build stream: {}", e)))
                })?
        }
        format => {
            return Err(CaptureError::Io(std::io::Error::other(format!(
                "unsupported sample format: {:?}",
                format
            ))));
        }
    };

    // Start the stream
    stream
        .play()
        .map_err(|e| CaptureError::Io(std::io::Error::other(format!("stream play: {}", e))))?;

    tracing::info!("audio capture started");

    // Start screen context capture if enabled (with permission check)
    let _screen_handle = if config.screen_context.enabled {
        if !crate::screen::check_screen_permission() {
            eprintln!("[minutes] Screen context disabled — grant Screen Recording permission in System Settings > Privacy & Security");
            None
        } else {
            let screen_dir = crate::screen::screens_dir_for(output_path);
            match crate::screen::start_capture(
                &screen_dir,
                std::time::Duration::from_secs(config.screen_context.interval_secs),
                Arc::clone(&stop_flag),
            ) {
                Ok(handle) => {
                    eprintln!(
                        "[minutes] Screen context capture enabled (every {}s)",
                        config.screen_context.interval_secs
                    );
                    Some(handle)
                }
                Err(e) => {
                    tracing::warn!(
                        "screen capture init failed: {} — continuing without screen context",
                        e
                    );
                    None
                }
            }
        }
    } else {
        None
    };

    // Wait for stop signal
    while !stop_flag.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));

        if err_flag.load(Ordering::Relaxed) {
            tracing::error!("audio stream encountered an error, stopping");
            break;
        }
    }

    // Stop and finalize
    drop(stream); // Stop the audio stream

    let total_samples = sample_count.load(Ordering::Relaxed);
    let duration_secs = total_samples as f64 / 16000.0;
    tracing::info!(
        samples = total_samples,
        duration_secs = format!("{:.1}", duration_secs),
        "audio capture stopped"
    );

    // Finalize the WAV file
    let mut guard = writer.lock().unwrap();
    if let Some(w) = guard.take() {
        w.finalize()
            .map_err(|e| CaptureError::Io(std::io::Error::other(format!("WAV finalize: {}", e))))?;
    }

    eprintln!(
        "[minutes] Captured {} samples ({:.1}s), peak audio level during recording: {}",
        total_samples,
        duration_secs,
        AUDIO_LEVEL.load(Ordering::Relaxed)
    );

    if total_samples == 0 {
        return Err(CaptureError::EmptyRecording);
    }

    Ok(())
}

/// List available audio input devices (for diagnostics / `minutes setup`).
pub fn list_input_devices() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    let mut devices = Vec::new();

    if let Ok(input_devices) = host.input_devices() {
        for device in input_devices {
            if let Ok(name) = device.name() {
                let info = if let Ok(config) = device.default_input_config() {
                    format!(
                        "{} ({}Hz, {} ch)",
                        name,
                        config.sample_rate().0,
                        config.channels()
                    )
                } else {
                    name
                };
                devices.push(info);
            }
        }
    }

    devices
}
