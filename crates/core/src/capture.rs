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

/// Seconds of silence before checking if the audio device changed.
/// Shorter than silence_reminder_secs to enable fast reconnection.
const DEVICE_CHECK_SILENCE_SECS: u64 = 5;

/// Build a cpal input stream that writes resampled 16kHz mono into the shared WAV writer.
/// Returns the stream handle and the device name string.
fn build_capture_stream(
    device: &cpal::Device,
    writer: &Arc<std::sync::Mutex<Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>>>,
    stop_flag: &Arc<AtomicBool>,
    sample_count: &Arc<std::sync::atomic::AtomicU64>,
    err_flag: &Arc<AtomicBool>,
) -> Result<cpal::Stream, CaptureError> {
    use cpal::traits::{DeviceTrait, StreamTrait};

    let supported_config = device
        .default_input_config()
        .map_err(|e| CaptureError::Io(std::io::Error::other(format!("input config: {}", e))))?;

    let sample_rate = supported_config.sample_rate().0;
    let channels = supported_config.channels();
    let ratio = sample_rate as f64 / 16000.0;

    tracing::info!(
        sample_rate,
        channels,
        format = ?supported_config.sample_format(),
        "audio capture config"
    );

    let writer_clone = Arc::clone(writer);
    let stop_clone = Arc::clone(stop_flag);
    let sample_count_clone = Arc::clone(sample_count);
    let err_flag_clone = Arc::clone(err_flag);

    let stream = match supported_config.sample_format() {
        cpal::SampleFormat::F32 => {
            let ch = channels as usize;
            let mut resample_pos: f64 = 0.0;
            let mut input_samples: Vec<f32> = Vec::new();
            let mut level_accum: f64 = 0.0;
            let mut level_count: u32 = 0;
            let level_interval = sample_rate / 10; // ~10 updates/sec

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
            let level_interval = sample_rate / 10;

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

    stream
        .play()
        .map_err(|e| CaptureError::Io(std::io::Error::other(format!("stream play: {}", e))))?;

    Ok(stream)
}

/// Try to reconnect to the current default audio device.
/// Returns the new stream and device name on success.
fn try_reconnect(
    host: &cpal::Host,
    device_override: Option<&str>,
    writer: &Arc<std::sync::Mutex<Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>>>,
    stop_flag: &Arc<AtomicBool>,
    sample_count: &Arc<std::sync::atomic::AtomicU64>,
    err_flag: &Arc<AtomicBool>,
) -> Option<(cpal::Stream, String)> {
    use cpal::traits::DeviceTrait;

    // Reset error flag for the new stream
    err_flag.store(false, Ordering::Relaxed);

    let device = match select_input_device(host, device_override) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("reconnect: device selection failed: {}", e);
            return None;
        }
    };

    let name = device.name().unwrap_or_else(|_| "unknown".into());

    match build_capture_stream(&device, writer, stop_flag, sample_count, err_flag) {
        Ok(stream) => {
            tracing::info!(device = %name, "audio stream reconnected");
            Some((stream, name))
        }
        Err(e) => {
            tracing::warn!(device = %name, "reconnect: build stream failed: {}", e);
            None
        }
    }
}

/// Start recording audio from the default input device.
/// Blocks until `stop_flag` is set to true (via signal handler) or a stop
/// sentinel file is detected (from `minutes stop`).
/// Writes raw PCM to a WAV file at the given path.
/// If screen context is enabled, also captures periodic screenshots.
/// Automatically reconnects if the audio device changes mid-recording.
pub fn record_to_wav(
    output_path: &Path,
    stop_flag: Arc<AtomicBool>,
    config: &Config,
) -> Result<(), CaptureError> {
    use cpal::traits::DeviceTrait;

    // Clear any stale stop sentinel from a previous session
    crate::pid::check_and_clear_sentinel();

    let host = cpal::default_host();
    let device_override = config.recording.device.as_deref();
    let device = select_input_device(&host, device_override)?;

    let device_name = device.name().unwrap_or_else(|_| "unknown".into());
    eprintln!("[minutes] Using input device: {}", device_name);
    tracing::info!(device = %device_name, "using audio input device");

    // Create WAV writer — always write as 16kHz mono 16-bit for whisper
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

    let sample_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let err_flag = Arc::new(AtomicBool::new(false));

    // Reset audio level
    AUDIO_LEVEL.store(0, Ordering::Relaxed);

    // Build initial stream (wrapped in Option for reconnection)
    let mut stream = Some(build_capture_stream(
        &device,
        &writer,
        &stop_flag,
        &sample_count,
        &err_flag,
    )?);
    tracing::info!("audio capture started");

    // Device change monitor
    let mut device_monitor = crate::device_monitor::DeviceMonitor::new(&device_name);
    let mut current_device_name = device_name;

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

    // Silence detection state
    let silence_threshold = config.recording.silence_threshold;
    let silence_reminder_secs = config.recording.silence_reminder_secs;
    let mut silence_start: Option<std::time::Instant> = None;
    let mut silence_notified = false;

    // Wait for stop signal (Ctrl+C sets stop_flag, `minutes stop` writes sentinel)
    while !stop_flag.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));

        if crate::pid::check_and_clear_sentinel() {
            tracing::info!("stop sentinel detected — stopping recording");
            break;
        }

        // Check for stream error or device change → attempt reconnection
        let should_reconnect = if err_flag.load(Ordering::Relaxed) {
            tracing::warn!("audio stream error detected — checking for device change");
            true
        } else if device_monitor.has_device_changed() {
            tracing::info!("default audio device changed — will reconnect");
            true
        } else {
            false
        };

        // Also check for silence-triggered device change (device went silent because it changed)
        let silence_triggered_reconnect = if !should_reconnect && silence_reminder_secs > 0 {
            let level = audio_level();
            if level <= silence_threshold {
                let start = silence_start.get_or_insert_with(std::time::Instant::now);
                let silent_secs = start.elapsed().as_secs();

                // Check device change after a few seconds of silence (faster than silence reminder)
                if silent_secs >= DEVICE_CHECK_SILENCE_SECS && device_monitor.has_device_changed() {
                    true
                } else if silent_secs >= silence_reminder_secs && !silence_notified {
                    silence_notified = true;
                    tracing::info!(
                        silent_secs,
                        "silence detected — sending reminder notification"
                    );
                    send_silence_notification(silent_secs);
                    false
                } else {
                    false
                }
            } else {
                // Audio resumed — reset silence tracking
                if silence_notified {
                    tracing::info!("audio resumed after silence notification");
                }
                silence_start = None;
                silence_notified = false;
                false
            }
        } else {
            false
        };

        if should_reconnect || silence_triggered_reconnect {
            // Drop old stream before building a new one
            stream.take();

            // Try reconnecting (with one retry after 1s)
            let reconnected = try_reconnect(
                &host,
                device_override,
                &writer,
                &stop_flag,
                &sample_count,
                &err_flag,
            )
            .or_else(|| {
                tracing::info!("reconnect failed, retrying in 1s...");
                std::thread::sleep(std::time::Duration::from_secs(1));
                try_reconnect(
                    &host,
                    device_override,
                    &writer,
                    &stop_flag,
                    &sample_count,
                    &err_flag,
                )
            });

            match reconnected {
                Some((new_stream, new_name)) => {
                    let old_name = current_device_name.clone();
                    current_device_name = new_name;
                    device_monitor.update_device(&current_device_name);
                    stream = Some(new_stream);
                    silence_start = None;
                    silence_notified = false;

                    eprintln!(
                        "[minutes] Audio device switched: {} → {}",
                        old_name, current_device_name
                    );
                    send_device_change_notification(&old_name, &current_device_name);

                    // Log event for agent reactivity
                    crate::events::append_event(crate::events::MinutesEvent::DeviceChanged {
                        old_device: old_name,
                        new_device: current_device_name.clone(),
                    });
                }
                None => {
                    tracing::error!("could not reconnect to any audio device — stopping recording");
                    break;
                }
            }
        }
    }

    // Stop and finalize
    drop(stream);

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

    // Set restrictive permissions on the recording (contains sensitive audio)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output_path, std::fs::Permissions::from_mode(0o600)).ok();
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

/// Select the best input device.
///
/// If `device_name` is provided, matches by name against available devices.
/// Otherwise, queries the macOS system default (via `system_profiler`),
/// then falls back to cpal's `default_input_device()`.
///
/// cpal's `default_input_device()` picks the first device in enumeration order,
/// which on macOS is often a virtual device (Descript Loopback, Zoom Audio, etc.)
/// rather than the actual system default.
pub fn select_input_device(
    host: &cpal::Host,
    device_name: Option<&str>,
) -> Result<cpal::Device, CaptureError> {
    use cpal::traits::{DeviceTrait, HostTrait};

    // If a specific device was requested, find it by name
    if let Some(requested) = device_name {
        if let Ok(devices) = host.input_devices() {
            for device in devices {
                if let Ok(name) = device.name() {
                    if name == requested {
                        tracing::info!(device = %name, "using requested input device");
                        return Ok(device);
                    }
                }
            }
        }
        // Collect available device names for a helpful error message
        let available: Vec<String> = host
            .input_devices()
            .map(|devs| devs.filter_map(|d| d.name().ok()).collect())
            .unwrap_or_default();
        tracing::error!(
            requested = %requested,
            available = ?available,
            "requested audio device not found"
        );
        return Err(CaptureError::Io(std::io::Error::other(format!(
            "audio device '{}' not found. Available devices: {}",
            requested,
            if available.is_empty() {
                "(none)".to_string()
            } else {
                available.join(", ")
            }
        ))));
    }

    // Try to get the macOS system default input device name
    #[cfg(target_os = "macos")]
    if let Some(system_default_name) = get_macos_default_input_name() {
        // Search cpal's device list for a matching name
        if let Ok(devices) = host.input_devices() {
            for device in devices {
                if let Ok(name) = device.name() {
                    if name == system_default_name {
                        tracing::info!(
                            device = %name,
                            "matched macOS system default input device"
                        );
                        return Ok(device);
                    }
                }
            }
        }
        tracing::warn!(
            system_default = %system_default_name,
            "could not find macOS default input in cpal devices, using cpal default"
        );
    }

    // Fallback: cpal's default (works on all platforms)
    host.default_input_device()
        .ok_or(CaptureError::DeviceNotFound)
}

/// Query macOS for the actual system default input device name.
/// Uses `system_profiler` which is more reliable than AppleScript for audio devices.
#[cfg(target_os = "macos")]
pub fn get_macos_default_input_name() -> Option<String> {
    // Try AppleScript to get the system-level default input device
    let output = std::process::Command::new("system_profiler")
        .args(["SPAudioDataType", "-json"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let items = json.get("SPAudioDataType")?.as_array()?;

    // Devices are nested under _items in each top-level entry
    for item in items {
        if let Some(sub_items) = item.get("_items").and_then(|v| v.as_array()) {
            for sub in sub_items {
                let is_default_input = sub
                    .get("coreaudio_default_audio_input_device")
                    .and_then(|v| v.as_str())
                    .map(|s| s == "spaudio_yes")
                    .unwrap_or(false);

                if is_default_input {
                    return sub
                        .get("_name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
        }
    }

    None
}

/// Send a macOS notification when silence is detected during recording.
fn send_silence_notification(silent_secs: u64) {
    let minutes = silent_secs / 60;
    let body = if minutes >= 2 {
        format!(
            "No audio detected for {} minutes. Still recording — run `minutes stop` when done.",
            minutes
        )
    } else {
        format!(
            "No audio detected for {} seconds. Still recording — run `minutes stop` when done.",
            silent_secs
        )
    };

    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"Minutes\" sound name \"Submarine\"",
            body.replace('\\', "\\\\").replace('"', "\\\"")
        );
        match std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
        {
            Ok(_) => tracing::debug!("silence notification sent"),
            Err(e) => tracing::warn!("failed to send notification: {}", e),
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("[minutes] {}", body);
    }
}

/// Send a macOS notification when the audio input device changes mid-recording.
fn send_device_change_notification(old_device: &str, new_device: &str) {
    let body = format!(
        "Audio input switched from \"{}\" to \"{}\".",
        old_device, new_device
    );

    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"Minutes\" sound name \"Blow\"",
            body.replace('\\', "\\\\").replace('"', "\\\"")
        );
        match std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
        {
            Ok(_) => tracing::debug!("device change notification sent"),
            Err(e) => tracing::warn!("failed to send notification: {}", e),
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("[minutes] {}", body);
    }
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
