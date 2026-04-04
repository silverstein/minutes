use crate::config::Config;
use crate::error::CaptureError;
use crate::pid::CaptureMode;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Shared audio level (0–100 scale) for UI visualization.
/// Updated ~10x per second from the cpal callback.
static AUDIO_LEVEL: AtomicU32 = AtomicU32::new(0);

/// Get the current audio input level (0–100).
pub fn audio_level() -> u32 {
    AUDIO_LEVEL.load(Ordering::Relaxed)
}

// ──────────────────────────────────────────────────────────────
// Recording Safety Guard — protects against forgotten recordings
// ──────────────────────────────────────────────────────────────

/// Why the guard wants to stop the recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Silence,
    TimeCapReached,
    DiskSpaceLow,
}

/// Action the caller should take after a safety check.
#[derive(Debug)]
pub enum SafetyAction {
    /// No action needed.
    None,
    /// Show a non-urgent notification (silence nudge).
    Nudge(String),
    /// Show an urgent warning (auto-stop approaching).
    Warning(String),
    /// Stop the recording immediately.
    Stop(StopReason, String),
}

/// Reusable guard that monitors recording health and signals when to nudge,
/// warn, or auto-stop. Used by `record_to_wav`, native call capture, and
/// live transcript (time cap + disk only).
pub struct RecordingSafetyGuard {
    silence_reminder_secs: u64,
    silence_auto_stop_secs: u64,
    silence_threshold: u32,
    max_duration_secs: u64,
    min_disk_space_mb: u64,
    output_path: std::path::PathBuf,

    recording_start: Instant,
    silence_start: Option<Instant>,
    nudge_count: u32,
    grace_start: Option<Instant>,
    last_disk_check: Instant,
    last_available_mb: Option<u64>,
    time_cap_warned: bool,
    intent: Option<RecordingIntent>,
    extended: bool,
}

/// Nudge schedule: 5 min, 15 min, 30 min, then every 30 min.
fn nudge_threshold_secs(base: u64, count: u32) -> u64 {
    match count {
        0 => base,
        1 => base * 3,
        _ => base * 6,
    }
}

/// Grace period before auto-stop: if audio resumes, defer.
const GRACE_PERIOD_SECS: u64 = 60;

impl RecordingSafetyGuard {
    pub fn new(config: &crate::config::RecordingConfig, output_path: &Path) -> Self {
        let now = Instant::now();
        Self {
            silence_reminder_secs: config.silence_reminder_secs,
            silence_auto_stop_secs: config.silence_auto_stop_secs,
            silence_threshold: config.silence_threshold,
            max_duration_secs: config.max_duration_secs,
            min_disk_space_mb: config.min_disk_space_mb,
            output_path: output_path.to_path_buf(),
            recording_start: now,
            silence_start: None,
            nudge_count: 0,
            grace_start: None,
            last_disk_check: now,
            last_available_mb: None,
            time_cap_warned: false,
            intent: None,
            extended: false,
        }
    }

    pub fn with_intent(mut self, intent: RecordingIntent) -> Self {
        self.intent = Some(intent);
        self
    }

    /// Reset the silence timer (called when user clicks "Keep Recording").
    pub fn extend(&mut self) {
        self.silence_start = None;
        self.nudge_count = 0;
        self.grace_start = None;
        self.extended = true;
    }

    /// Check all safety tiers. Call this every ~100ms from the capture loop.
    pub fn check(&mut self, current_audio_level: u32, call_app_active: bool) -> SafetyAction {
        // Tier 4: Disk space guard (checked first, most urgent)
        if let Some(action) = self.check_disk_space() {
            return action;
        }

        // Tier 3: Hard time cap
        if let Some(action) = self.check_time_cap() {
            return action;
        }

        // Tier 1+2: Silence detection (nudge + auto-stop)
        self.check_silence(current_audio_level, call_app_active)
    }

    /// Check only time cap and disk space (for live transcript mode).
    pub fn check_time_and_disk(&mut self) -> SafetyAction {
        if let Some(action) = self.check_disk_space() {
            return action;
        }
        if let Some(action) = self.check_time_cap() {
            return action;
        }
        SafetyAction::None
    }

    fn check_disk_space(&mut self) -> Option<SafetyAction> {
        if self.min_disk_space_mb == 0 {
            return None;
        }

        let check_interval = match self.last_available_mb {
            Some(mb) if mb < 500 => std::time::Duration::from_secs(2),
            Some(mb) if mb < 1000 => std::time::Duration::from_secs(10),
            _ => std::time::Duration::from_secs(60),
        };

        if self.last_disk_check.elapsed() < check_interval {
            return None;
        }
        self.last_disk_check = Instant::now();

        match available_disk_space_mb(&self.output_path) {
            Some(available_mb) => {
                self.last_available_mb = Some(available_mb);
                if available_mb < self.min_disk_space_mb {
                    Some(SafetyAction::Stop(
                        StopReason::DiskSpaceLow,
                        format!(
                            "Disk space critically low ({}MB remaining). Recording auto-stopped to prevent data loss.",
                            available_mb
                        ),
                    ))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn check_time_cap(&mut self) -> Option<SafetyAction> {
        if self.max_duration_secs == 0 {
            return None;
        }

        let elapsed = self.recording_start.elapsed().as_secs();

        if elapsed >= self.max_duration_secs {
            let hours = self.max_duration_secs / 3600;
            return Some(SafetyAction::Stop(
                StopReason::TimeCapReached,
                format!(
                    "Recording reached the {}-hour time limit. Auto-stopped and processing.",
                    hours
                ),
            ));
        }

        // Warn at 90% of cap
        let warn_at = self.max_duration_secs * 9 / 10;
        if elapsed >= warn_at && !self.time_cap_warned {
            self.time_cap_warned = true;
            let remaining_min = (self.max_duration_secs - elapsed) / 60;
            return Some(SafetyAction::Warning(format!(
                "Recording will auto-stop in {} minutes (time limit).",
                remaining_min.max(1)
            )));
        }

        None
    }

    fn check_silence(&mut self, current_audio_level: u32, call_app_active: bool) -> SafetyAction {
        if self.silence_reminder_secs == 0 && self.silence_auto_stop_secs == 0 {
            return SafetyAction::None;
        }

        if current_audio_level > self.silence_threshold {
            // Audio resumed
            if self.silence_start.is_some() {
                self.silence_start = None;
                self.nudge_count = 0;
                self.grace_start = None;
                self.extended = false;
            }
            return SafetyAction::None;
        }

        // Audio is silent
        let start = self.silence_start.get_or_insert_with(Instant::now);
        let silent_secs = start.elapsed().as_secs();

        // Suppress silence actions for active calls (user is likely muted/listening)
        let is_active_call = self.intent == Some(RecordingIntent::Call) && call_app_active;

        // Tier 2: Auto-stop on prolonged silence
        if self.silence_auto_stop_secs > 0 && !is_active_call {
            let effective_limit = if self.intent == Some(RecordingIntent::Call) {
                // Call intent but no active call app: use 2x threshold
                self.silence_auto_stop_secs * 2
            } else {
                self.silence_auto_stop_secs
            };

            if silent_secs >= effective_limit {
                // Grace period: check if audio just resumed
                if let Some(grace) = self.grace_start {
                    if grace.elapsed().as_secs() >= GRACE_PERIOD_SECS {
                        // Grace period expired, still silent: stop
                        let minutes = silent_secs / 60;
                        return SafetyAction::Stop(
                            StopReason::Silence,
                            format!(
                                "No audio for {} minutes. Recording auto-stopped and processing.",
                                minutes
                            ),
                        );
                    }
                    // Still in grace period, wait
                    return SafetyAction::None;
                }
                // Enter grace period
                self.grace_start = Some(Instant::now());
                let minutes = silent_secs / 60;
                return SafetyAction::Warning(format!(
                    "No audio for {} minutes. Auto-stopping in 1 minute unless audio resumes.",
                    minutes
                ));
            }
        }

        // Tier 1: Silence nudges (escalating)
        if self.silence_reminder_secs > 0 && !is_active_call {
            let next_nudge_at = nudge_threshold_secs(self.silence_reminder_secs, self.nudge_count);
            if silent_secs >= next_nudge_at {
                self.nudge_count += 1;
                let minutes = silent_secs / 60;
                let msg = if minutes >= 2 {
                    format!(
                        "No audio detected for {} minutes. Still recording.",
                        minutes
                    )
                } else {
                    format!(
                        "No audio detected for {} seconds. Still recording.",
                        silent_secs
                    )
                };
                return SafetyAction::Nudge(msg);
            }
        }

        SafetyAction::None
    }

    /// Whether silence was detected long enough to trigger a device reconnect check.
    pub fn silence_duration_secs(&self) -> Option<u64> {
        self.silence_start.map(|start| start.elapsed().as_secs())
    }
}

/// Get available disk space in MB for the filesystem containing the given path.
#[allow(clippy::unnecessary_cast)] // statvfs field types vary across platforms
pub fn available_disk_space_mb(path: &Path) -> Option<u64> {
    let check_path = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(Path::new("/")).to_path_buf()
    };

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut c_path = check_path.as_os_str().as_bytes().to_vec();
        c_path.push(0);
        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c_path.as_ptr() as *const libc::c_char, &mut stat) == 0 {
                let available_bytes = (stat.f_bavail as u64) * (stat.f_frsize as u64);
                return Some(available_bytes / (1024 * 1024));
            }
        }
        None
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = check_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_bytes: u64 = 0;
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_bytes as *mut u64,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok != 0 {
            return Some(free_bytes / (1024 * 1024));
        }
        None
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = check_path;
        None
    }
}

/// Check for an extend sentinel (used by CLI `minutes extend`).
pub fn check_and_clear_extend_sentinel() -> bool {
    let sentinel = crate::config::Config::minutes_dir().join("extend.sentinel");
    if sentinel.exists() {
        std::fs::remove_file(&sentinel).ok();
        true
    } else {
        false
    }
}

/// Write the extend sentinel (used by CLI `minutes extend` command).
pub fn write_extend_sentinel() -> std::io::Result<()> {
    let sentinel = crate::config::Config::minutes_dir().join("extend.sentinel");
    std::fs::write(&sentinel, b"extend")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordingIntent {
    Memo,
    Room,
    Call,
}

impl RecordingIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memo => "memo",
            Self::Room => "room",
            Self::Call => "call",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CapturePreflight {
    pub intent: RecordingIntent,
    pub inferred_call_app: Option<String>,
    pub input_device: String,
    pub system_audio_ready: bool,
    pub allow_degraded: bool,
    pub blocking_reason: Option<String>,
    pub warnings: Vec<String>,
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
///
/// Delegates mono-downmix + decimation to `resample::build_resampled_input_stream`,
/// then converts f32 samples to i16 for the WAV writer and updates the audio level meter.
fn build_capture_stream(
    device: &cpal::Device,
    writer: &Arc<std::sync::Mutex<Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>>>,
    stop_flag: &Arc<AtomicBool>,
    sample_count: &Arc<std::sync::atomic::AtomicU64>,
    err_flag: &Arc<AtomicBool>,
) -> Result<cpal::Stream, CaptureError> {
    let writer_clone = Arc::clone(writer);
    let sample_count_clone = Arc::clone(sample_count);

    // Level meter state — updated from the resampled samples (~10x/sec)
    let mut level_accum: f64 = 0.0;
    let mut level_count: u32 = 0;

    // We'll set the level_interval once we know the native sample rate.
    // For now, use a placeholder; it gets set after build_resampled_input_stream returns the config.
    // Actually, the callback receives resampled 16kHz samples, so the interval should be
    // based on 16kHz: ~1600 samples for 10 updates/sec.
    let level_interval: u32 = 1600; // 16000 / 10

    let (stream, _device_name, _config) = crate::resample::build_resampled_input_stream(
        device,
        stop_flag,
        err_flag,
        move |resampled: &[f32]| {
            // Update audio level meter from resampled mono f32 samples
            for &sample in resampled {
                level_accum += (sample as f64) * (sample as f64);
                level_count += 1;
                if level_count >= level_interval {
                    let rms = (level_accum / level_count as f64).sqrt();
                    let level = (rms * 2000.0).min(100.0) as u32;
                    AUDIO_LEVEL.store(level, Ordering::Relaxed);
                    level_accum = 0.0;
                    level_count = 0;
                }
            }

            // Write resampled samples to WAV as i16
            let mut guard = writer_clone.lock().unwrap();
            if let Some(ref mut w) = *guard {
                for &sample in resampled {
                    let s16 = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    if w.write_sample(s16).is_err() {
                        return;
                    }
                    sample_count_clone.fetch_add(1, Ordering::Relaxed);
                }
            }
        },
    )?;

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

    let name = device
        .description()
        .map_or_else(|_| "unknown".to_string(), |d| d.name().to_string());

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

    let device_name = device
        .description()
        .map_or_else(|_| "unknown".to_string(), |d| d.name().to_string());
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

    // Safety guard for auto-stop on silence, time cap, disk space
    let preflight_intent = config
        .recording
        .auto_call_intent
        .then(|| detect_active_call_app(config).map(|_| RecordingIntent::Call))
        .flatten();
    let mut safety_guard = RecordingSafetyGuard::new(&config.recording, output_path);
    if let Some(intent) = preflight_intent {
        safety_guard = safety_guard.with_intent(intent);
    }

    // Wait for stop signal (Ctrl+C sets stop_flag, `minutes stop` writes sentinel)
    while !stop_flag.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));

        if crate::pid::check_and_clear_sentinel() {
            tracing::info!("stop sentinel detected — stopping recording");
            break;
        }

        // Check for extend sentinel from CLI `minutes extend`
        if check_and_clear_extend_sentinel() {
            tracing::info!("extend sentinel detected — resetting safety timers");
            safety_guard.extend();
        }

        // Safety guard check (silence, time cap, disk space)
        let call_app_active = detect_active_call_app(config).is_some();
        match safety_guard.check(audio_level(), call_app_active) {
            SafetyAction::None => {}
            SafetyAction::Nudge(msg) => {
                tracing::info!("{}", msg);
                send_silence_notification_msg(&msg);
            }
            SafetyAction::Warning(msg) => {
                tracing::warn!("{}", msg);
                send_silence_notification_msg(&msg);
            }
            SafetyAction::Stop(reason, msg) => {
                tracing::warn!(reason = ?reason, "{}", msg);
                send_silence_notification_msg(&msg);
                break;
            }
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

        // Also check for silence-triggered device change
        let silence_triggered_reconnect = if !should_reconnect {
            safety_guard
                .silence_duration_secs()
                .map(|secs| {
                    secs >= DEVICE_CHECK_SILENCE_SECS && device_monitor.has_device_changed()
                })
                .unwrap_or(false)
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
                    safety_guard.extend(); // reset silence timers after reconnect

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

    tracing::info!(host_id = ?host.id(), "cpal host for input device selection");

    // If a specific device was requested, find it by name
    if let Some(requested) = device_name {
        if let Ok(devices) = host.input_devices() {
            for device in devices {
                if let Ok(desc) = device.description() {
                    let name = desc.name().to_string();
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
            .map(|devs| {
                devs.filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
                    .collect()
            })
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
                if let Ok(desc) = device.description() {
                    let name = desc.name().to_string();
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

fn detect_call_app_from_processes(
    running: &[String],
    config: &crate::config::CallDetectionConfig,
) -> Option<String> {
    for config_app in &config.apps {
        let config_lower = config_app.to_lowercase();
        if running.iter().any(|process_name| {
            let process_lower = process_name.to_lowercase();
            process_lower.contains(&config_lower) || config_lower.contains(&process_lower)
        }) {
            return Some(match config_app.as_str() {
                "zoom.us" => "Zoom".into(),
                "Microsoft Teams" | "Microsoft Teams (work or school)" => "Teams".into(),
                "FaceTime" => "FaceTime".into(),
                "Webex" => "Webex".into(),
                "Slack" => "Slack".into(),
                other => other.into(),
            });
        }
    }
    None
}

fn running_process_names() -> Vec<String> {
    let output = std::process::Command::new("ps")
        .args(["-eo", "comm="])
        .output();

    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None;
                }
                Some(trimmed.rsplit('/').next().unwrap_or(trimmed).to_string())
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub fn detect_active_call_app(config: &Config) -> Option<String> {
    detect_call_app_from_processes(&running_process_names(), &config.call_detection)
}

pub fn is_system_audio_device_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    [
        "blackhole",
        "loopback",
        "soundflower",
        "vb-cable",
        "stereo mix",
        "multi-output",
        "aggregate",
    ]
    .iter()
    .any(|hint| lower.contains(hint))
}

pub fn selected_input_device_name(config: &Config) -> Result<String, CaptureError> {
    use cpal::traits::DeviceTrait;

    let host = cpal::default_host();
    let device = select_input_device(&host, config.recording.device.as_deref())?;
    device
        .description()
        .map(|d| d.name().to_string())
        .map_err(|error| CaptureError::Io(std::io::Error::other(error.to_string())))
}

fn infer_recording_intent(
    mode: CaptureMode,
    requested_intent: Option<RecordingIntent>,
    detected_call_app: Option<&str>,
    config: &Config,
) -> Result<RecordingIntent, String> {
    if mode == CaptureMode::QuickThought {
        if let Some(intent) = requested_intent {
            if intent != RecordingIntent::Memo {
                return Err(
                    "Quick thoughts only support memo intent. Use meeting mode for room or call capture."
                        .into(),
                );
            }
        }
        return Ok(RecordingIntent::Memo);
    }

    if let Some(intent) = requested_intent {
        return Ok(intent);
    }

    if config.recording.auto_call_intent && detected_call_app.is_some() {
        Ok(RecordingIntent::Call)
    } else {
        Ok(RecordingIntent::Room)
    }
}

fn evaluate_capture_preflight(
    mode: CaptureMode,
    requested_intent: Option<RecordingIntent>,
    detected_call_app: Option<String>,
    input_device: String,
    allow_degraded: bool,
    config: &Config,
) -> Result<CapturePreflight, String> {
    let intent =
        infer_recording_intent(mode, requested_intent, detected_call_app.as_deref(), config)?;
    let system_audio_ready = is_system_audio_device_name(&input_device);
    let allow_degraded = allow_degraded || config.recording.allow_degraded_call_capture;
    let mut warnings = Vec::new();
    let mut blocking_reason = None;

    if intent == RecordingIntent::Call {
        if let Some(app_name) = detected_call_app.as_deref() {
            warnings.push(format!("Detected active {} call.", app_name));
        }
        if system_audio_ready {
            warnings.push(format!(
                "Using '{}' as the input route for call capture.",
                input_device
            ));
        } else if allow_degraded {
            warnings.push(format!(
                "Starting degraded call capture from '{}'. This will likely miss the remote side of the call.",
                input_device
            ));
        } else {
            blocking_reason = Some(format!(
                "Minutes inferred a call capture, but '{}' looks like a microphone input, not a call-audio route. To record both sides, use the desktop app's native call capture path or choose a system-audio device like BlackHole. If you intentionally want mic-only capture, explicitly allow degraded call capture.",
                input_device
            ));
        }
    }

    Ok(CapturePreflight {
        intent,
        inferred_call_app: detected_call_app,
        input_device,
        system_audio_ready,
        allow_degraded,
        blocking_reason,
        warnings,
    })
}

pub fn preflight_recording(
    mode: CaptureMode,
    requested_intent: Option<RecordingIntent>,
    allow_degraded: bool,
    config: &Config,
) -> Result<CapturePreflight, String> {
    let detected_call_app = detect_active_call_app(config);
    let input_device = selected_input_device_name(config).map_err(|error| error.to_string())?;
    evaluate_capture_preflight(
        mode,
        requested_intent,
        detected_call_app,
        input_device,
        allow_degraded,
        config,
    )
}

/// Send a macOS notification when silence is detected during recording.
fn send_silence_notification_msg(body: &str) {
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
            Ok(_) => tracing::debug!("safety notification sent"),
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
    tracing::info!(host_id = ?host.id(), "cpal host for input device listing");
    let mut devices = Vec::new();

    if let Ok(input_devices) = host.input_devices() {
        for device in input_devices {
            if let Ok(desc) = device.description() {
                let name = desc.name().to_string();
                let info = if let Ok(config) = device.default_input_config() {
                    format!(
                        "{} ({}Hz, {} ch)",
                        name,
                        config.sample_rate(),
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

/// A device with its category for the `minutes sources` command.
#[derive(Debug, Clone)]
pub struct CategorizedDevice {
    pub name: String,
    pub category: DeviceCategory,
    pub sample_rate: u32,
    pub channels: u16,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceCategory {
    Microphone,
    SystemAudio,
    Virtual,
}

/// List audio input devices grouped by category.
pub fn list_devices_categorized() -> Vec<CategorizedDevice> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();
    tracing::info!(host_id = ?host.id(), "cpal host for categorized device listing");
    let default_name = host
        .default_input_device()
        .and_then(|d| d.description().ok().map(|desc| desc.name().to_string()))
        .unwrap_or_default();

    let mut devices = Vec::new();

    if let Ok(input_devices) = host.input_devices() {
        for device in input_devices {
            let Ok(desc) = device.description() else { continue };
            let name = desc.name().to_string();
            let (sample_rate, channels) = device
                .default_input_config()
                .map(|c| (c.sample_rate(), c.channels()))
                .unwrap_or((0, 0));

            let category = if is_system_audio_device_name(&name) {
                DeviceCategory::SystemAudio
            } else if name.to_lowercase().contains("virtual")
                || name.to_lowercase().contains("pipewire")
                || name.to_lowercase().contains("pulse")
            {
                DeviceCategory::Virtual
            } else {
                DeviceCategory::Microphone
            };

            devices.push(CategorizedDevice {
                is_default: name == default_name,
                name,
                category,
                sample_rate,
                channels,
            });
        }
    }

    devices
}

/// Auto-detect a loopback/system-audio device for `--call` / `call = "auto"`.
/// Returns the device name if found, None otherwise.
pub fn detect_loopback_device() -> Option<String> {
    let devices = list_devices_categorized();
    devices
        .into_iter()
        .find(|d| d.category == DeviceCategory::SystemAudio)
        .map(|d| d.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_call_app_matches_configured_processes() {
        let running = vec![
            "/Applications/Microsoft Teams.app/Contents/MacOS/Microsoft Teams".to_string(),
            "/System/Library/CoreServices/Finder.app/Contents/MacOS/Finder".to_string(),
        ];
        let config = crate::config::CallDetectionConfig::default();

        let detected = detect_call_app_from_processes(&running, &config);

        assert_eq!(detected.as_deref(), Some("Teams"));
    }

    #[test]
    fn evaluate_capture_preflight_blocks_plain_mic_for_call_intent() {
        let config = Config::default();
        let preflight = evaluate_capture_preflight(
            CaptureMode::Meeting,
            Some(RecordingIntent::Call),
            Some("Teams".into()),
            "Built-in Microphone".into(),
            false,
            &config,
        )
        .unwrap();

        assert_eq!(preflight.intent, RecordingIntent::Call);
        assert!(!preflight.system_audio_ready);
        assert!(preflight.blocking_reason.is_some());
    }

    #[test]
    fn evaluate_capture_preflight_allows_known_system_audio_route() {
        let config = Config::default();
        let preflight = evaluate_capture_preflight(
            CaptureMode::Meeting,
            Some(RecordingIntent::Call),
            Some("Zoom".into()),
            "BlackHole 2ch".into(),
            false,
            &config,
        )
        .unwrap();

        assert!(preflight.system_audio_ready);
        assert!(preflight.blocking_reason.is_none());
        assert!(!preflight.warnings.is_empty());
    }

    #[test]
    fn evaluate_capture_preflight_honors_degraded_override() {
        let config = Config::default();
        let preflight = evaluate_capture_preflight(
            CaptureMode::Meeting,
            Some(RecordingIntent::Call),
            Some("Meet".into()),
            "Built-in Microphone".into(),
            true,
            &config,
        )
        .unwrap();

        assert!(preflight.blocking_reason.is_none());
        assert!(preflight.allow_degraded);
        assert!(!preflight.warnings.is_empty());
    }

    fn test_config() -> crate::config::RecordingConfig {
        crate::config::RecordingConfig {
            silence_reminder_secs: 10,
            silence_threshold: 3,
            silence_auto_stop_secs: 30,
            max_duration_secs: 60,
            min_disk_space_mb: 0,
            ..Default::default()
        }
    }

    #[test]
    fn safety_guard_no_action_when_audio_present() {
        let config = test_config();
        let mut guard = RecordingSafetyGuard::new(&config, Path::new("/tmp/test.wav"));
        assert!(matches!(guard.check(50, false), SafetyAction::None));
    }

    #[test]
    fn safety_guard_escalating_nudge_schedule() {
        assert_eq!(nudge_threshold_secs(300, 0), 300);
        assert_eq!(nudge_threshold_secs(300, 1), 900);
        assert_eq!(nudge_threshold_secs(300, 2), 1800);
        assert_eq!(nudge_threshold_secs(300, 3), 1800);
    }

    #[test]
    fn safety_guard_suppresses_for_active_call() {
        let config = test_config();
        let mut guard = RecordingSafetyGuard::new(&config, Path::new("/tmp/test.wav"))
            .with_intent(RecordingIntent::Call);
        assert!(matches!(guard.check(0, true), SafetyAction::None));
    }

    #[test]
    fn safety_guard_extend_resets_silence() {
        let config = test_config();
        let mut guard = RecordingSafetyGuard::new(&config, Path::new("/tmp/test.wav"));
        guard.check(0, false);
        assert!(guard.silence_start.is_some());
        guard.extend();
        assert!(guard.silence_start.is_none());
        assert_eq!(guard.nudge_count, 0);
    }

    #[test]
    fn safety_guard_audio_resume_resets_silence() {
        let config = test_config();
        let mut guard = RecordingSafetyGuard::new(&config, Path::new("/tmp/test.wav"));
        guard.check(0, false);
        assert!(guard.silence_start.is_some());
        guard.check(50, false);
        assert!(guard.silence_start.is_none());
    }

    #[test]
    fn safety_guard_time_cap_warning_at_90_percent() {
        let config = crate::config::RecordingConfig {
            max_duration_secs: 10,
            silence_reminder_secs: 0,
            silence_auto_stop_secs: 0,
            min_disk_space_mb: 0,
            ..Default::default()
        };
        let mut guard = RecordingSafetyGuard::new(&config, Path::new("/tmp/test.wav"));
        guard.recording_start = Instant::now() - std::time::Duration::from_secs(9);
        let action = guard.check(50, false);
        assert!(matches!(action, SafetyAction::Warning(_)));
        assert!(guard.time_cap_warned);
    }

    #[test]
    fn safety_guard_time_cap_stops_at_limit() {
        let config = crate::config::RecordingConfig {
            max_duration_secs: 10,
            silence_reminder_secs: 0,
            silence_auto_stop_secs: 0,
            min_disk_space_mb: 0,
            ..Default::default()
        };
        let mut guard = RecordingSafetyGuard::new(&config, Path::new("/tmp/test.wav"));
        guard.recording_start = Instant::now() - std::time::Duration::from_secs(11);
        guard.time_cap_warned = true;
        let action = guard.check(50, false);
        assert!(matches!(
            action,
            SafetyAction::Stop(StopReason::TimeCapReached, _)
        ));
    }

    #[test]
    fn safety_guard_disabled_when_zeros() {
        let config = crate::config::RecordingConfig {
            silence_reminder_secs: 0,
            silence_auto_stop_secs: 0,
            max_duration_secs: 0,
            min_disk_space_mb: 0,
            ..Default::default()
        };
        let mut guard = RecordingSafetyGuard::new(&config, Path::new("/tmp/test.wav"));
        assert!(matches!(guard.check(0, false), SafetyAction::None));
    }

    #[test]
    fn safety_guard_call_intent_doubles_auto_stop_threshold() {
        let config = test_config();
        let mut guard = RecordingSafetyGuard::new(&config, Path::new("/tmp/test.wav"))
            .with_intent(RecordingIntent::Call);
        guard.silence_start = Some(Instant::now() - std::time::Duration::from_secs(31));
        let action = guard.check(0, false);
        assert!(!matches!(
            action,
            SafetyAction::Stop(StopReason::Silence, _)
        ));
    }

    #[test]
    fn available_disk_space_returns_some_for_valid_path() {
        let result = available_disk_space_mb(&std::env::temp_dir());
        assert!(result.is_some());
        assert!(result.unwrap() > 0);
    }

    // Skipped on Windows CI: calling cpal twice in the same process (here + health test)
    // triggers STATUS_ACCESS_VIOLATION on runners without audio hardware.
    #[test]
    #[cfg_attr(target_os = "windows", ignore)]
    fn list_input_devices_returns_vec_of_strings() {
        let devices = list_input_devices();
        // Should return a Vec<String> (may be empty in CI, but must not panic)
        assert!(devices.iter().all(|d| !d.is_empty()));
    }
}
