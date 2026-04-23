use crate::config::Config;
use crate::error::CaptureError;
use crate::pid::CaptureMode;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[cfg(feature = "streaming")]
use crate::streaming::AudioStream;

/// Shared audio level (0–100 scale) for UI visualization.
/// Updated ~10x per second from the cpal callback.
static AUDIO_LEVEL: AtomicU32 = AtomicU32::new(0);

/// Count of audio chunks dropped by the live sidecar channel (buffer full).
static SIDECAR_DROPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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

#[derive(Debug, Clone)]
struct SingleCapturePlan {
    device_override: Option<String>,
    device_name: String,
}

#[cfg(feature = "streaming")]
#[derive(Debug, Clone)]
struct DualCapturePlan {
    voice_override: Option<String>,
    voice_device_name: String,
    call_override: String,
    call_device_name: String,
}

#[derive(Debug, Clone)]
enum CapturePlan {
    Single(SingleCapturePlan),
    #[cfg(feature = "streaming")]
    Dual(DualCapturePlan),
}

impl CapturePlan {
    fn input_summary(&self) -> String {
        match self {
            Self::Single(plan) => plan.device_name.clone(),
            #[cfg(feature = "streaming")]
            Self::Dual(plan) => format!("{} + {}", plan.voice_device_name, plan.call_device_name),
        }
    }

    fn system_audio_ready(&self) -> bool {
        match self {
            Self::Single(plan) => is_system_audio_device_name(&plan.device_name),
            #[cfg(feature = "streaming")]
            Self::Dual(plan) => is_system_audio_device_name(&plan.call_device_name),
        }
    }
}

const MISSING_DUAL_SOURCE_LOOPBACK_MESSAGE: &str =
    "no loopback/system-audio device detected for dual-source capture";

pub fn stem_paths_for(audio_path: &Path) -> Option<crate::diarize::StemPaths> {
    let stem = audio_path.file_stem()?.to_str()?;
    let dir = audio_path.parent()?;
    Some(crate::diarize::StemPaths {
        voice: dir.join(format!("{}.voice.wav", stem)),
        system: dir.join(format!("{}.system.wav", stem)),
    })
}

pub fn meeting_audio_artifact_paths(markdown_path: &Path) -> Vec<PathBuf> {
    let audio_path = markdown_path.with_extension("wav");
    let mut paths = vec![audio_path.clone()];

    if let Some(stems) = stem_paths_for(&audio_path) {
        paths.push(stems.voice);
        paths.push(stems.system);
    }

    paths.push(crate::voice::meeting_embeddings_sidecar_path(markdown_path));
    paths
}

fn normalize_source_name(value: Option<&str>) -> Option<String> {
    match value.map(str::trim) {
        Some("") | None => None,
        Some(value) if value.eq_ignore_ascii_case("default") => None,
        Some(value) => Some(value.to_string()),
    }
}

fn select_device_with_override(
    host: &cpal::Host,
    device_override: Option<&str>,
) -> Result<(cpal::Device, String), CaptureError> {
    use cpal::traits::DeviceTrait;

    let device = select_input_device(host, device_override)?;
    let name = device
        .description()
        .map_or_else(|_| "unknown".to_string(), |d| d.name().to_string());
    Ok((device, name))
}

fn resolve_capture_plan(config: &Config) -> Result<CapturePlan, CaptureError> {
    let host = cached_default_host();
    resolve_capture_plan_with_host(host, config)
}

fn resolve_capture_plan_with_host(
    host: &cpal::Host,
    config: &Config,
) -> Result<CapturePlan, CaptureError> {
    let voice_override = normalize_source_name(
        config
            .recording
            .sources
            .as_ref()
            .and_then(|sources| sources.voice.as_deref()),
    );
    let call_override = config
        .recording
        .sources
        .as_ref()
        .and_then(|sources| sources.call.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());

    #[cfg(not(feature = "streaming"))]
    if call_override.is_some() {
        return Err(CaptureError::Io(std::io::Error::other(
            "dual-source capture requires the streaming feature",
        )));
    }

    #[cfg(feature = "streaming")]
    if let Some(call_override) = call_override {
        let (_, voice_name) = select_device_with_override(host, voice_override.as_deref())?;
        let resolved_call = if call_override.eq_ignore_ascii_case("auto") {
            detect_loopback_device().ok_or_else(|| {
                CaptureError::Io(std::io::Error::other(MISSING_DUAL_SOURCE_LOOPBACK_MESSAGE))
            })?
        } else {
            call_override.to_string()
        };
        let (_, call_name) = select_device_with_override(host, Some(&resolved_call))?;
        if voice_name == call_name {
            return Err(CaptureError::Io(std::io::Error::other(
                "voice and call sources resolved to the same device",
            )));
        }
        return Ok(CapturePlan::Dual(DualCapturePlan {
            voice_override,
            voice_device_name: voice_name,
            call_override: resolved_call,
            call_device_name: call_name,
        }));
    }

    let single_override = voice_override.or_else(|| config.recording.device.clone());
    let (_, device_name) = select_device_with_override(host, single_override.as_deref())?;
    Ok(CapturePlan::Single(SingleCapturePlan {
        device_override: single_override,
        device_name,
    }))
}

fn resolve_native_call_preflight_capture_plan_with_host(
    host: &cpal::Host,
    config: &Config,
) -> Result<CapturePlan, CaptureError> {
    let voice_override = normalize_source_name(
        config
            .recording
            .sources
            .as_ref()
            .and_then(|sources| sources.voice.as_deref()),
    );
    let single_override = voice_override.or_else(|| config.recording.device.clone());
    let (_, device_name) = select_device_with_override(host, single_override.as_deref())?;
    Ok(CapturePlan::Single(SingleCapturePlan {
        device_override: single_override,
        device_name,
    }))
}

fn configured_call_source_is_auto(config: &Config) -> bool {
    config
        .recording
        .sources
        .as_ref()
        .and_then(|sources| sources.call.as_deref())
        .map(str::trim)
        .is_some_and(|value| value.eq_ignore_ascii_case("auto"))
}

fn should_bypass_loopback_preflight_for_native_call_capture(
    intent: RecordingIntent,
    native_call_capture_available: bool,
    config: &Config,
    error: &CaptureError,
) -> bool {
    if intent != RecordingIntent::Call
        || !native_call_capture_available
        || !configured_call_source_is_auto(config)
    {
        return false;
    }

    matches!(error, CaptureError::Io(io_error) if io_error.to_string() == MISSING_DUAL_SOURCE_LOOPBACK_MESSAGE)
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
    live_tx: &Option<std::sync::mpsc::SyncSender<Vec<f32>>>,
) -> Result<cpal::Stream, CaptureError> {
    let writer_clone = Arc::clone(writer);
    let sample_count_clone = Arc::clone(sample_count);
    let live_tx_clone = live_tx.clone();

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

            // Fork samples to live transcript sidecar (non-blocking, drops if full)
            if let Some(ref tx) = live_tx_clone {
                if tx.try_send(resampled.to_vec()).is_err() {
                    SIDECAR_DROPS.fetch_add(1, Ordering::Relaxed);
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
    live_tx: &Option<std::sync::mpsc::SyncSender<Vec<f32>>>,
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

    match build_capture_stream(&device, writer, stop_flag, sample_count, err_flag, live_tx) {
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

#[cfg(feature = "streaming")]
const DUAL_SOURCE_SLOT_SAMPLES: usize = 1600;

#[cfg(feature = "streaming")]
struct DualCaptureWriters {
    mixed: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    voice: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    system: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    mixed_sample_count: u64,
}

fn wav_spec_16k_mono() -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

fn create_wav_writer(
    output_path: &Path,
) -> Result<hound::WavWriter<std::io::BufWriter<std::fs::File>>, CaptureError> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    hound::WavWriter::create(output_path, wav_spec_16k_mono())
        .map_err(|e| CaptureError::Io(std::io::Error::other(format!("WAV create: {}", e))))
}

#[cfg(feature = "streaming")]
impl DualCaptureWriters {
    fn new(output_path: &Path) -> Result<Self, CaptureError> {
        let stems = stem_paths_for(output_path).ok_or_else(|| {
            CaptureError::Io(std::io::Error::other(
                "could not derive per-source stem paths for dual-source capture",
            ))
        })?;

        Ok(Self {
            mixed: create_wav_writer(output_path)?,
            voice: create_wav_writer(&stems.voice)?,
            system: create_wav_writer(&stems.system)?,
            mixed_sample_count: 0,
        })
    }

    fn write_slot(
        &mut self,
        voice_samples: &[f32],
        system_samples: &[f32],
        live_tx: &Option<std::sync::mpsc::SyncSender<Vec<f32>>>,
    ) -> Result<(), CaptureError> {
        write_samples_to_wav(&mut self.voice, voice_samples)?;
        write_samples_to_wav(&mut self.system, system_samples)?;

        let mixed = mix_dual_source_slot(voice_samples, system_samples);
        update_audio_level_from_samples(&mixed);
        write_samples_to_wav(&mut self.mixed, &mixed)?;
        self.mixed_sample_count += mixed.len() as u64;

        if let Some(ref tx) = live_tx {
            if tx.try_send(mixed).is_err() {
                SIDECAR_DROPS.fetch_add(1, Ordering::Relaxed);
            }
        }

        Ok(())
    }

    fn finalize(self) -> Result<u64, CaptureError> {
        finalize_wav_writer(self.voice)?;
        finalize_wav_writer(self.system)?;
        let mixed_sample_count = self.mixed_sample_count;
        finalize_wav_writer(self.mixed)?;
        Ok(mixed_sample_count)
    }
}

#[cfg(feature = "streaming")]
fn update_audio_level_from_samples(samples: &[f32]) {
    if samples.is_empty() {
        AUDIO_LEVEL.store(0, Ordering::Relaxed);
        return;
    }

    let rms = (samples
        .iter()
        .map(|sample| (*sample as f64) * (*sample as f64))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt();
    let level = (rms * 2000.0).min(100.0) as u32;
    AUDIO_LEVEL.store(level, Ordering::Relaxed);
}

#[cfg(feature = "streaming")]
fn write_samples_to_wav(
    writer: &mut hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    samples: &[f32],
) -> Result<(), CaptureError> {
    for &sample in samples {
        let s16 = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
        writer
            .write_sample(s16)
            .map_err(|e| CaptureError::Io(std::io::Error::other(format!("WAV write: {}", e))))?;
    }
    Ok(())
}

#[cfg(feature = "streaming")]
fn finalize_wav_writer(
    writer: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
) -> Result<(), CaptureError> {
    writer
        .finalize()
        .map_err(|e| CaptureError::Io(std::io::Error::other(format!("WAV finalize: {}", e))))
}

fn set_capture_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
    }
}

#[cfg(feature = "streaming")]
fn mix_dual_source_slot(voice_samples: &[f32], system_samples: &[f32]) -> Vec<f32> {
    let slot_len = voice_samples
        .len()
        .max(system_samples.len())
        .max(DUAL_SOURCE_SLOT_SAMPLES);
    let mut mixed = vec![0.0f32; slot_len];

    for (index, sample) in voice_samples.iter().enumerate() {
        mixed[index] += *sample;
    }
    for (index, sample) in system_samples.iter().enumerate() {
        mixed[index] += *sample;
    }

    for sample in &mut mixed {
        *sample = sample.clamp(-1.0, 1.0);
    }

    mixed
}

#[cfg(feature = "streaming")]
fn padded_slot(samples: Option<Vec<f32>>) -> Vec<f32> {
    let mut padded = vec![0.0f32; DUAL_SOURCE_SLOT_SAMPLES];
    if let Some(samples) = samples {
        for (index, sample) in samples
            .into_iter()
            .enumerate()
            .take(DUAL_SOURCE_SLOT_SAMPLES)
        {
            padded[index] = sample;
        }
    }
    padded
}

#[cfg(feature = "streaming")]
#[derive(Default)]
struct DualSlotStats {
    both: u64,
    voice_only: u64,
    system_only: u64,
}

#[cfg(feature = "streaming")]
fn flush_dual_source_slots(
    next_slot: &mut Option<u64>,
    max_slot: Option<u64>,
    pending_voice: &mut std::collections::BTreeMap<u64, Vec<f32>>,
    pending_system: &mut std::collections::BTreeMap<u64, Vec<f32>>,
    writers: &mut DualCaptureWriters,
    live_tx: &Option<std::sync::mpsc::SyncSender<Vec<f32>>>,
    slot_stats: &mut DualSlotStats,
) -> Result<(), CaptureError> {
    let (Some(current_slot), Some(max_slot)) = (*next_slot, max_slot) else {
        return Ok(());
    };
    if current_slot > max_slot {
        return Ok(());
    }

    let mut slot = current_slot;
    while slot <= max_slot {
        let has_voice = pending_voice.contains_key(&slot);
        let has_system = pending_system.contains_key(&slot);
        match (has_voice, has_system) {
            (true, true) => slot_stats.both += 1,
            (true, false) => slot_stats.voice_only += 1,
            (false, true) => slot_stats.system_only += 1,
            (false, false) => {} // silence slot, both padded
        }
        let voice = padded_slot(pending_voice.remove(&slot));
        let system = padded_slot(pending_system.remove(&slot));
        writers.write_slot(&voice, &system, live_tx)?;
        slot += 1;
    }
    *next_slot = Some(slot);
    Ok(())
}

#[cfg(feature = "streaming")]
fn record_to_wav_dual_source(
    output_path: &Path,
    stop_flag: Arc<AtomicBool>,
    config: &Config,
    plan: DualCapturePlan,
) -> Result<(), CaptureError> {
    crate::pid::check_and_clear_sentinel();
    // Refresh the in-process mic-mute flag from the sentinel. The CLI
    // `--mute-mic` flag writes the sentinel before getting here, and the
    // loop below keeps re-reading it so Tauri/CLI toggles made during the
    // recording take effect.
    crate::streaming::refresh_mic_mute_from_sentinel();

    let mut writers = DualCaptureWriters::new(output_path)?;
    AUDIO_LEVEL.store(0, Ordering::Relaxed);

    let (live_tx, sidecar_handle) = start_live_sidecar(config, &stop_flag);

    let mut voice_stream = Some(AudioStream::start(plan.voice_override.as_deref())?);
    let mut system_stream = Some(AudioStream::start(Some(&plan.call_override))?);
    // Call side is always a pinned override; voice side is pinned iff the caller
    // supplied an explicit override. Pinned sides skip default-device-change polling.
    let voice_pinned = plan.voice_override.is_some();
    let mut device_monitor = crate::device_monitor::MultiDeviceMonitor::with_pinned(
        &voice_stream.as_ref().expect("voice stream").device_name,
        voice_pinned,
        &system_stream.as_ref().expect("system stream").device_name,
        true,
    );

    eprintln!(
        "[minutes] Using voice input device: {}",
        voice_stream.as_ref().expect("voice stream").device_name
    );
    eprintln!(
        "[minutes] Using system audio device: {}",
        system_stream.as_ref().expect("system stream").device_name
    );
    tracing::info!(
        voice = %voice_stream.as_ref().expect("voice stream").device_name,
        system = %system_stream.as_ref().expect("system stream").device_name,
        "using dual-source audio input devices"
    );

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

    let preflight_intent = config
        .recording
        .auto_call_intent
        .then(|| detect_active_call_app(config).map(|_| RecordingIntent::Call))
        .flatten();
    let mut safety_guard = RecordingSafetyGuard::new(&config.recording, output_path);
    if let Some(intent) = preflight_intent {
        safety_guard = safety_guard.with_intent(intent);
    }

    let session_start = Instant::now();
    let mut next_slot: Option<u64> = None;
    let mut max_voice_slot: Option<u64> = None;
    let mut max_system_slot: Option<u64> = None;
    let mut pending_voice = std::collections::BTreeMap::<u64, Vec<f32>>::new();
    let mut pending_system = std::collections::BTreeMap::<u64, Vec<f32>>::new();
    // Mixer stats for diagnosing dual-source issues
    let mut slot_stats = DualSlotStats::default();
    let mut peak_level: u32 = 0;

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        if crate::pid::check_and_clear_sentinel() {
            tracing::info!("stop sentinel detected — stopping dual-source recording");
            break;
        }

        if check_and_clear_extend_sentinel() {
            tracing::info!("extend sentinel detected — resetting safety timers");
            safety_guard.extend();
        }

        // Sync mute state from sentinel so CLI/Tauri toggles made in
        // another process are picked up by this recording loop.
        crate::streaming::refresh_mic_mute_from_sentinel();

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

        if voice_stream
            .as_ref()
            .is_some_and(crate::streaming::AudioStream::has_error)
            || system_stream
                .as_ref()
                .is_some_and(crate::streaming::AudioStream::has_error)
            || device_monitor.check_changes().is_some()
        {
            tracing::warn!("dual-source stream issue detected — attempting restart");
            voice_stream.take();
            system_stream.take();

            match (
                AudioStream::start(plan.voice_override.as_deref()),
                AudioStream::start(Some(&plan.call_override)),
            ) {
                (Ok(new_voice), Ok(new_system)) => {
                    eprintln!(
                        "[minutes] Dual-source capture reconnected: {} + {}",
                        new_voice.device_name, new_system.device_name
                    );
                    device_monitor = crate::device_monitor::MultiDeviceMonitor::with_pinned(
                        &new_voice.device_name,
                        voice_pinned,
                        &new_system.device_name,
                        true,
                    );
                    voice_stream = Some(new_voice);
                    system_stream = Some(new_system);
                    safety_guard.extend();
                }
                (voice_result, system_result) => {
                    if let Err(error) = voice_result {
                        tracing::error!(error = %error, "failed to restart voice stream");
                    }
                    if let Err(error) = system_result {
                        tracing::error!(error = %error, "failed to restart system stream");
                    }
                    break;
                }
            }
        }

        let voice_rx = voice_stream
            .as_ref()
            .expect("voice stream should stay active")
            .receiver
            .clone();
        let system_rx = system_stream
            .as_ref()
            .expect("system stream should stay active")
            .receiver
            .clone();

        // Derive slot from wall-clock timestamp relative to session start.
        // Both streams share the same session_start anchor, so slots from
        // different devices correspond to the same real-world time window.
        let slot_for = |chunk: &crate::streaming::AudioChunk| -> u64 {
            chunk
                .timestamp
                .checked_duration_since(session_start)
                .unwrap_or_default()
                .as_millis() as u64
                / 100
        };

        // Drain all available chunks from both channels before flushing.
        // The old code used select! to grab ONE chunk per iteration, which
        // on slower machines meant one source got flushed before the other
        // source's chunk for that same slot arrived. (#118)
        //
        // Voice samples pass through `mute_voice_if_needed`: when the mic
        // is muted, samples are zeroed but their length (and therefore the
        // slot's sample count) is preserved so dual-source alignment and
        // stem writers stay in lockstep.
        let mute_voice_if_needed = |samples: Vec<f32>| -> Vec<f32> {
            if crate::streaming::is_mic_muted() {
                vec![0.0; samples.len()]
            } else {
                samples
            }
        };

        let mut got_any = false;
        while let Ok(chunk) = voice_rx.try_recv() {
            let slot = slot_for(&chunk);
            next_slot.get_or_insert(slot);
            max_voice_slot = Some(max_voice_slot.map_or(slot, |s| s.max(slot)));
            pending_voice.insert(slot, mute_voice_if_needed(chunk.samples));
            got_any = true;
        }
        while let Ok(chunk) = system_rx.try_recv() {
            let slot = slot_for(&chunk);
            next_slot.get_or_insert(slot);
            max_system_slot = Some(max_system_slot.map_or(slot, |s| s.max(slot)));
            pending_system.insert(slot, chunk.samples);
            got_any = true;
        }

        // If nothing was available, block briefly for the next chunk
        if !got_any {
            crossbeam_channel::select! {
                recv(voice_rx) -> chunk => {
                    if let Ok(chunk) = chunk {
                        let slot = slot_for(&chunk);
                        next_slot.get_or_insert(slot);
                        max_voice_slot = Some(max_voice_slot.map_or(slot, |s| s.max(slot)));
                        pending_voice.insert(slot, mute_voice_if_needed(chunk.samples));
                    }
                }
                recv(system_rx) -> chunk => {
                    if let Ok(chunk) = chunk {
                        let slot = slot_for(&chunk);
                        next_slot.get_or_insert(slot);
                        max_system_slot = Some(max_system_slot.map_or(slot, |s| s.max(slot)));
                        pending_system.insert(slot, chunk.samples);
                    }
                }
                default(std::time::Duration::from_millis(50)) => {}
            }
        }

        // Track peak audio level (actual peak, not just last value)
        let current_level = audio_level();
        if current_level > peak_level {
            peak_level = current_level;
        }

        // Flush only slots where BOTH sources have had a chance to deliver.
        // Use min(max_voice, max_system) - 1 so we never flush a slot before
        // both sources have reached it.
        let safe_slot = match (max_voice_slot, max_system_slot) {
            (Some(v), Some(s)) => Some(v.min(s).saturating_sub(1)),
            _ => None,
        };
        flush_dual_source_slots(
            &mut next_slot,
            safe_slot,
            &mut pending_voice,
            &mut pending_system,
            &mut writers,
            &live_tx,
            &mut slot_stats,
        )?;
    }

    // Final flush: write any remaining slots (pad with silence for missing sources)
    let final_max = match (max_voice_slot, max_system_slot) {
        (Some(v), Some(s)) => Some(v.max(s)),
        (Some(v), None) => Some(v),
        (None, Some(s)) => Some(s),
        (None, None) => None,
    };
    flush_dual_source_slots(
        &mut next_slot,
        final_max,
        &mut pending_voice,
        &mut pending_system,
        &mut writers,
        &live_tx,
        &mut slot_stats,
    )?;

    voice_stream.take();
    system_stream.take();
    drop(live_tx);
    if let Some(handle) = sidecar_handle {
        handle.join().ok();
    }

    // Clear mute state so the next recording starts fresh.
    crate::streaming::clear_mic_mute_for_new_recording();

    let sidecar_drops = SIDECAR_DROPS.swap(0, Ordering::Relaxed);
    if sidecar_drops > 0 {
        tracing::warn!(
            dropped_chunks = sidecar_drops,
            "live transcript sidecar: chunks dropped (sidecar channel full, transcript may have gaps)"
        );
    }

    let total_samples = writers.finalize()?;
    let duration_secs = total_samples as f64 / 16000.0;
    let total_slots = slot_stats.both + slot_stats.voice_only + slot_stats.system_only;

    set_capture_permissions(output_path);
    if let Some(stems) = stem_paths_for(output_path) {
        set_capture_permissions(&stems.voice);
        set_capture_permissions(&stems.system);
    }

    eprintln!(
        "[minutes] Captured {} mixed samples ({:.1}s), peak audio level: {}",
        total_samples, duration_secs, peak_level
    );
    if total_slots > 0 {
        let pct_both = (slot_stats.both as f64 / total_slots as f64 * 100.0) as u64;
        eprintln!(
            "[minutes] Mixer stats: {} slots total, {}% with both sources ({} both, {} voice-only, {} system-only)",
            total_slots, pct_both, slot_stats.both, slot_stats.voice_only, slot_stats.system_only
        );
        tracing::info!(
            total_slots,
            slots_both = slot_stats.both,
            slots_voice_only = slot_stats.voice_only,
            slots_system_only = slot_stats.system_only,
            pct_both,
            "dual-source mixer stats"
        );
    }

    if total_samples == 0 {
        return Err(CaptureError::EmptyRecording);
    }

    Ok(())
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

    let capture_plan = resolve_capture_plan(config)?;
    #[cfg(feature = "streaming")]
    if let CapturePlan::Dual(plan) = capture_plan.clone() {
        return record_to_wav_dual_source(output_path, stop_flag, config, plan);
    }

    // Clear any stale stop sentinel from a previous session
    crate::pid::check_and_clear_sentinel();
    // Single-source recording has no gate (muting a mic-only stream just
    // produces silence, not a useful outcome), but we clear any stale
    // mute sentinel so it doesn't leak into the next dual-source session.
    #[cfg(feature = "streaming")]
    crate::streaming::clear_mic_mute_for_new_recording();

    let host = cached_default_host();
    let device_override = match &capture_plan {
        CapturePlan::Single(plan) => plan.device_override.as_deref(),
        #[cfg(feature = "streaming")]
        CapturePlan::Dual(_) => None,
    };
    let device = select_input_device(host, device_override)?;

    let device_name = device
        .description()
        .map_or_else(|_| "unknown".to_string(), |d| d.name().to_string());
    eprintln!("[minutes] Using input device: {}", device_name);
    tracing::info!(device = %device_name, "using audio input device");

    // Create WAV writer — always write as 16kHz mono 16-bit for whisper
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let writer = create_wav_writer(output_path)?;
    let writer = Arc::new(std::sync::Mutex::new(Some(writer)));

    let sample_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let err_flag = Arc::new(AtomicBool::new(false));

    // Reset audio level
    AUDIO_LEVEL.store(0, Ordering::Relaxed);

    // Start live transcript sidecar — streams real-time transcription to JSONL
    // so agents can see what's being discussed during the recording.
    let (live_tx, sidecar_handle) = start_live_sidecar(config, &stop_flag);

    // Build initial stream (wrapped in Option for reconnection)
    let mut stream = Some(build_capture_stream(
        &device,
        &writer,
        &stop_flag,
        &sample_count,
        &err_flag,
        &live_tx,
    )?);
    tracing::info!("audio capture started");

    // Device change monitor. When the user pinned a specific device via config
    // or --device, don't auto-switch on default-device changes — the override is
    // explicit intent. Also avoids a spurious reconnect loop on Linux where
    // cpal's "default" name doesn't match the pinned name (e.g. "pulse").
    let mut device_monitor = if device_override.is_some() {
        crate::device_monitor::DeviceMonitor::pinned(&device_name)
    } else {
        crate::device_monitor::DeviceMonitor::new(&device_name)
    };
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
                host,
                device_override,
                &writer,
                &stop_flag,
                &sample_count,
                &err_flag,
                &live_tx,
            )
            .or_else(|| {
                tracing::info!("reconnect failed, retrying in 1s...");
                std::thread::sleep(std::time::Duration::from_secs(1));
                try_reconnect(
                    host,
                    device_override,
                    &writer,
                    &stop_flag,
                    &sample_count,
                    &err_flag,
                    &live_tx,
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

    // Disconnect the live sidecar channel and wait for it to finish
    drop(live_tx);
    if let Some(handle) = sidecar_handle {
        handle.join().ok();
    }
    #[cfg(all(feature = "whisper", feature = "streaming"))]
    crate::live_transcript::clear_status_file();
    let sidecar_drops = SIDECAR_DROPS.swap(0, Ordering::Relaxed);
    if sidecar_drops > 0 {
        tracing::warn!(
            dropped_chunks = sidecar_drops,
            "live sidecar: audio chunks dropped (transcript may have gaps)"
        );
    }

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
        #[cfg(feature = "streaming")]
        finalize_wav_writer(w)?;
        #[cfg(not(feature = "streaming"))]
        w.finalize()
            .map_err(|e| CaptureError::Io(std::io::Error::other(format!("WAV finalize: {}", e))))?;
    }

    // Set restrictive permissions on the recording (contains sensitive audio)
    set_capture_permissions(output_path);

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

/// Spawn a live transcript sidecar thread that receives audio samples and
/// produces a JSONL transcript in real-time. Returns (sender, join_handle).
/// When the whisper+streaming features are not available, returns (None, None).
#[cfg(all(feature = "whisper", feature = "streaming"))]
fn start_live_sidecar(
    config: &Config,
    stop_flag: &Arc<AtomicBool>,
) -> (
    Option<std::sync::mpsc::SyncSender<Vec<f32>>>,
    Option<std::thread::JoinHandle<()>>,
) {
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(200);
    let sidecar_config = config.clone();
    let sidecar_stop = stop_flag.clone();
    match std::thread::Builder::new()
        .name("live-sidecar".into())
        .spawn(move || {
            crate::live_transcript::run_sidecar_mpsc(rx, sidecar_stop, &sidecar_config);
        }) {
        Ok(handle) => (Some(tx), Some(handle)),
        Err(e) => {
            tracing::warn!("failed to spawn live sidecar thread: {}", e);
            (None, None)
        }
    }
}

#[cfg(not(all(feature = "whisper", feature = "streaming")))]
fn start_live_sidecar(
    _config: &Config,
    _stop_flag: &Arc<AtomicBool>,
) -> (
    Option<std::sync::mpsc::SyncSender<Vec<f32>>>,
    Option<std::thread::JoinHandle<()>>,
) {
    (None, None)
}

/// If `name` ends with the decorated format produced by [`list_input_devices`]
/// (e.g. `"Ground Control (16000Hz, 1 ch)"`), return the bare device name.
/// Otherwise, return the input unchanged.
///
/// This lets callers accept either form: saved configs that captured the
/// decorated label from the Tauri picker still resolve to the bare CPAL name.
pub fn strip_device_format_suffix(name: &str) -> &str {
    let Some(open_idx) = name.rfind(" (") else {
        return name;
    };
    let inside = &name[open_idx + 2..];
    let Some(inside) = inside.strip_suffix(')') else {
        return name;
    };
    let Some((hz_part, ch_part)) = inside.split_once(", ") else {
        return name;
    };
    let Some(hz_num) = hz_part.strip_suffix("Hz") else {
        return name;
    };
    let Some(ch_num) = ch_part.strip_suffix(" ch") else {
        return name;
    };
    if hz_num.parse::<u32>().is_err() || ch_num.parse::<u16>().is_err() {
        return name;
    }
    &name[..open_idx]
}

/// Normalize a persisted input-device setting to the canonical CPAL device
/// name by trimming whitespace and stripping any UI decoration suffix like
/// `" (16000Hz, 1 ch)"`.
pub fn canonicalize_input_device_setting(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(strip_device_format_suffix(trimmed).to_string())
}

/// Select the best input device.
///
/// If `device_name` is provided, matches by name against available devices.
/// The match first tries the exact string, then falls back to the bare name
/// if `device_name` is in the decorated `"Name (NHz, N ch)"` format. That
/// fallback is what lets legacy configs saved from the Tauri settings UI
/// (which used to persist the decorated label) continue to resolve.
/// Otherwise, queries the macOS system default (via `system_profiler`),
/// then falls back to cpal's `default_input_device()`.
///
/// cpal's `default_input_device()` picks the first device in enumeration order,
/// which on macOS is often a virtual device (Descript Loopback, Zoom Audio, etc.)
/// rather than the actual system default.
/// Remembers which cpal `HostId` successfully resolved a pinned device name
/// this process. cpal's Linux `default_host()` is nondeterministic when PipeWire
/// and ALSA are both compiled in (successive calls can return different hosts,
/// and re-creating PipeWire via `host_from_id` sometimes enumerates zero
/// devices). Caching the first host that works keeps later lookups stable.
static PREFERRED_HOST: std::sync::OnceLock<std::sync::Mutex<Option<cpal::HostId>>> =
    std::sync::OnceLock::new();

/// AIDEV-NOTE: Keeps the cpal `Host` alive for the entire process lifetime.
/// On Linux with the PipeWire backend, dropping a `cpal::Host` calls `pw_deinit()`,
/// and re-creating one calls `pw_init()` again — but PipeWire's internal state is
/// corrupted after deinit/reinit, causing a segfault in `pw_main_loop_new()`.
/// By leaking the Host (intentionally never freeing it), we prevent Drop from
/// running, so PipeWire stays initialized. The leak is negligible: one Host per
/// process lifetime. This matches cpal's own `PwInitGuard` design intent (the
/// Host holds the guard), but ensures the guard survives across multiple
/// `default_host()` call sites within minutes.
static HOST_CACHE: std::sync::OnceLock<&'static cpal::Host> = std::sync::OnceLock::new();

/// Return a reference to the process-wide cached `cpal::Host`, creating it on
/// first call. Uses `cpal::default_host()` internally, but ensures the Host
/// (and its PipeWire init guard) is never dropped, preventing the segfault
/// described in HOST_CACHE.
pub fn cached_default_host() -> &'static cpal::Host {
    HOST_CACHE.get_or_init(|| {
        // Intentionally leak: Host must live for the process duration so that
        // PipeWire's pw_deinit() is never called between enumeration and stream
        // creation, which would corrupt PipeWire state and segfault on re-init.
        Box::leak(Box::new(cpal::default_host()))
    })
}

fn preferred_host_id() -> Option<cpal::HostId> {
    *PREFERRED_HOST
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .ok()?
}

fn set_preferred_host_id(id: cpal::HostId) {
    if let Ok(mut guard) = PREFERRED_HOST
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
    {
        *guard = Some(id);
    }
}

/// Look up a device by exact name on the given host. Retries enumeration a few
/// times because the PipeWire cpal backend occasionally reports zero devices on
/// the first `input_devices()` call after a fresh `host_from_id`.
fn find_device_on_host(host: &cpal::Host, requested: &str) -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let bare = strip_device_format_suffix(requested);
    for attempt in 0..3 {
        if let Ok(devices) = host.input_devices() {
            for device in devices {
                if let Ok(desc) = device.description() {
                    let name = desc.name();
                    if name == requested || name == bare {
                        tracing::info!(
                            device = %name,
                            host_id = ?host.id(),
                            attempt,
                            "using requested input device"
                        );
                        return Some(device);
                    }
                }
            }
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    None
}

pub fn select_input_device(
    host: &cpal::Host,
    device_name: Option<&str>,
) -> Result<cpal::Device, CaptureError> {
    use cpal::traits::{DeviceTrait, HostTrait};

    tracing::info!(host_id = ?host.id(), "cpal host for input device selection");

    // If a specific device was requested, find it by name
    if let Some(requested) = device_name {
        // Fast path: previous call in this process already proved a host works.
        // (Bare-name fallback for decorated device labels like "Mic (16000Hz, 1 ch)"
        // is handled inside find_device_on_host via strip_device_format_suffix.)
        if let Some(preferred) = preferred_host_id() {
            if preferred == host.id() {
                if let Some(device) = find_device_on_host(host, requested) {
                    return Ok(device);
                }
            } else if let Ok(preferred_host) = cpal::host_from_id(preferred) {
                if let Some(device) = find_device_on_host(&preferred_host, requested) {
                    tracing::info!(
                        device = %requested,
                        from_host = ?preferred,
                        called_with_host = ?host.id(),
                        "using cached preferred cpal host"
                    );
                    return Ok(device);
                }
            }
        }

        // Try the host the caller handed us.
        let primary_id = host.id();
        if let Some(device) = find_device_on_host(host, requested) {
            set_preferred_host_id(primary_id);
            return Ok(device);
        }

        // Fallback: cpal's `default_host()` on Linux is nondeterministic when both
        // PipeWire and ALSA are available — successive calls can return different
        // hosts. Try every other compiled-in host before giving up so a pinned
        // device name (e.g. "sink_default" on PipeWire, "pulse" on ALSA) keeps
        // working regardless of which host was handed to us.
        let mut searched_hosts = vec![format!("{:?}", primary_id)];
        for host_id in cpal::available_hosts() {
            if host_id == primary_id {
                continue;
            }
            searched_hosts.push(format!("{:?}", host_id));
            if let Ok(alt_host) = cpal::host_from_id(host_id) {
                if let Some(device) = find_device_on_host(&alt_host, requested) {
                    tracing::info!(
                        device = %requested,
                        from_host = ?host_id,
                        primary_host = ?primary_id,
                        "recovered pinned device from alternate cpal host"
                    );
                    set_preferred_host_id(host_id);
                    return Ok(device);
                }
            }
        }

        let available: Vec<String> = host
            .input_devices()
            .map(|devs| {
                devs.filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        tracing::error!(
            requested = %requested,
            searched_hosts = ?searched_hosts,
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

/// True if the device name matches a known system audio capture driver.
///
/// Three groups of patterns, all substring-matched against the lowercased name:
///
/// 1. Generic loopback/aggregate drivers: `blackhole`, `loopback`, `soundflower`,
///    `vb-cable`, `stereo mix`, `multi-output`, `aggregate`. These are the
///    drivers most users install for system audio capture on macOS / Windows.
///
/// 2. App-installed virtual drivers on macOS: `mmaudio`, `loomaudio`,
///    `zoomaudio`, `teams audio`. These are installed by Loom, Zoom, Microsoft
///    Teams, and similar apps for screen-recording / call-capture and
///    consistently report `supports_input == true && supports_output == true`
///    on CoreAudio. Empirically verified on Mat's machine 2026-04-06.
///    Substrings are chosen specifically enough to avoid false positives on
///    real microphones (e.g., "Camo Microphone" is a real mic via a 2-way
///    iPhone bridge that also reports as Duplex).
///
/// 3. PulseAudio monitor sources: anything containing `.monitor`. PulseAudio
///    exposes monitor sources as `Device::Source` entries with names ending in
///    `<sink_name>.monitor` (cpal/host/pulseaudio/mod.rs:107), so they pass
///    through `host.input_devices()` like regular sources and would otherwise
///    be lumped under Microphone. The PipeWire backend uses a different model
///    (Sinks-as-Duplex, see `categorize_device` for the host-gated check),
///    so this pattern only kicks in on PulseAudio.
pub fn is_system_audio_device_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    [
        // Generic loopback / aggregate
        "blackhole",
        "loopback",
        "soundflower",
        "vb-cable",
        "stereo mix",
        "multi-output",
        "aggregate",
        // macOS app-installed virtual drivers (verified 2026-04-06)
        "mmaudio",
        "loomaudio",
        "zoomaudio",
        "teams audio",
        // PulseAudio monitor source pattern
        ".monitor",
    ]
    .iter()
    .any(|hint| lower.contains(hint))
}

/// Categorize a device into Microphone / SystemAudio / Virtual.
///
/// On PipeWire, `Audio/Sink` nodes (your speakers, headphones) are exposed
/// with `direction = Duplex` so they appear in `host.input_devices()`. Recording
/// from one transparently uses the sink's monitor port via `STREAM_CAPTURE_SINK`
/// at stream creation. We surface that to users by categorizing them as
/// `SystemAudio` rather than `Microphone`. The PipeWire-only gate is necessary
/// because (a) ALSA backend Duplex devices are typically USB headsets where the
/// user wants the mic, not the speaker monitor, and (b) on macOS, virtual
/// drivers like Camo Microphone, Loom, Zoom, and Teams all report
/// `supports_output() == true` and would be wrongly bucketed.
///
/// Test fixture for unit tests — pure function, no cpal dependency.
fn categorize_device(
    name: &str,
    supports_input: bool,
    supports_output: bool,
    is_pipewire: bool,
) -> DeviceCategory {
    if is_pipewire && supports_input && supports_output {
        // PipeWire sink with monitor port — see doc comment above.
        return DeviceCategory::SystemAudio;
    }
    if is_system_audio_device_name(name) {
        return DeviceCategory::SystemAudio;
    }
    let lower = name.to_lowercase();
    if lower.contains("virtual") || lower.contains("pipewire") || lower.contains("pulse") {
        return DeviceCategory::Virtual;
    }
    DeviceCategory::Microphone
}

/// True when the cpal default host is the PipeWire backend.
///
/// Compiles to a const `false` on platforms where cpal doesn't expose a
/// `HostId::PipeWire` variant (macOS, Windows, etc.), so callers don't need
/// their own cfg guards. The `#[cfg]` set matches cpal's own pipewire feature
/// gate (`linux | dragonfly | freebsd | netbsd`) so BSD-with-PipeWire users
/// get the fix too.
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd"
))]
fn is_pipewire_host(host_id: cpal::HostId) -> bool {
    matches!(host_id, cpal::HostId::PipeWire)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd"
)))]
fn is_pipewire_host(_: cpal::HostId) -> bool {
    false
}

pub fn selected_input_device_name(config: &Config) -> Result<String, CaptureError> {
    use cpal::traits::DeviceTrait;

    let host = cached_default_host();
    let device = select_input_device(host, config.recording.device.as_deref())?;
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
    preflight_recording_with_native_call_capture(
        mode,
        requested_intent,
        allow_degraded,
        false,
        config,
    )
}

pub fn preflight_recording_with_native_call_capture(
    mode: CaptureMode,
    requested_intent: Option<RecordingIntent>,
    allow_degraded: bool,
    native_call_capture_available: bool,
    config: &Config,
) -> Result<CapturePreflight, String> {
    let host = cached_default_host();
    let detected_call_app = detect_active_call_app(config);
    let intent =
        infer_recording_intent(mode, requested_intent, detected_call_app.as_deref(), config)?;
    let capture_plan = match resolve_capture_plan_with_host(host, config) {
        Ok(plan) => plan,
        Err(error)
            if should_bypass_loopback_preflight_for_native_call_capture(
                intent,
                native_call_capture_available,
                config,
                &error,
            ) =>
        {
            resolve_native_call_preflight_capture_plan_with_host(host, config)
                .map_err(|fallback_error| fallback_error.to_string())?
        }
        Err(error) => return Err(error.to_string()),
    };
    let mut preflight = evaluate_capture_preflight(
        mode,
        Some(intent),
        detected_call_app,
        capture_plan.input_summary(),
        allow_degraded,
        config,
    )?;

    preflight.system_audio_ready = capture_plan.system_audio_ready();
    if preflight.intent == RecordingIntent::Call {
        if preflight.system_audio_ready {
            preflight.blocking_reason = None;
            if preflight.warnings.is_empty() {
                preflight.warnings.push(format!(
                    "Using '{}' as the capture route for call recording.",
                    preflight.input_device
                ));
            }
        } else if !preflight.allow_degraded {
            preflight.blocking_reason = Some(format!(
                "Minutes inferred a call capture, but '{}' does not include a recognized system-audio route. To record both sides, choose a loopback/system-audio device like BlackHole for the call source or use the desktop app's native call capture path. If you intentionally want mic-only capture, explicitly allow degraded call capture.",
                preflight.input_device
            ));
        }
    }

    Ok(preflight)
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

/// An input device entry with both the canonical CPAL name and the
/// human-readable label shown in diagnostics and device pickers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InputDeviceEntry {
    /// Canonical device name as reported by CPAL. This is the string that
    /// must be stored in `config.recording.device` so that capture can
    /// match it back to a device later.
    pub name: String,
    /// Human-readable label (e.g. `"Ground Control (16000Hz, 1 ch)"`).
    /// Suitable for UI display and `minutes devices` output.
    pub label: String,
}

/// List available audio input devices with both canonical names and labels.
///
/// Prefer this over [`list_input_devices`] when building UIs or storing
/// a device selection: save `name` into config, show `label` to the user.
pub fn list_input_devices_detailed() -> Vec<InputDeviceEntry> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cached_default_host();
    tracing::debug!(host_id = ?host.id(), "cpal host for input device listing");
    let mut devices = Vec::new();

    if let Ok(input_devices) = host.input_devices() {
        for device in input_devices {
            if let Ok(desc) = device.description() {
                let name = desc.name().to_string();
                let label = if let Ok(config) = device.default_input_config() {
                    format!(
                        "{} ({}Hz, {} ch)",
                        name,
                        config.sample_rate(),
                        config.channels()
                    )
                } else {
                    name.clone()
                };
                devices.push(InputDeviceEntry { name, label });
            }
        }
    }

    devices
}

/// List available audio input devices as decorated label strings.
///
/// Kept for backwards compatibility with existing diagnostics output
/// (`minutes devices`, `minutes setup`). For UI pickers or any caller
/// that will persist the selection, use [`list_input_devices_detailed`]
/// so the canonical name can be stored separately from the label.
pub fn list_input_devices() -> Vec<String> {
    list_input_devices_detailed()
        .into_iter()
        .map(|entry| entry.label)
        .collect()
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

    let host = cached_default_host();
    let host_id = host.id();
    tracing::debug!(host_id = ?host_id, "cpal host for categorized device listing");
    let is_pipewire = is_pipewire_host(host_id);
    let default_name = host
        .default_input_device()
        .and_then(|d| d.description().ok().map(|desc| desc.name().to_string()))
        .unwrap_or_default();

    let mut devices = Vec::new();

    if let Ok(input_devices) = host.input_devices() {
        for device in input_devices {
            let Ok(desc) = device.description() else {
                continue;
            };
            let name = desc.name().to_string();
            let (sample_rate, channels) = device
                .default_input_config()
                .map(|c| (c.sample_rate(), c.channels()))
                .unwrap_or((0, 0));

            // host.input_devices() already filters by supports_input, but we re-check
            // supports_output here to detect Duplex devices (PipeWire sinks-as-inputs).
            let supports_output = device.supports_output();
            let category = categorize_device(&name, true, supports_output, is_pipewire);

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
    fn strip_device_format_suffix_strips_decorated_label() {
        assert_eq!(
            strip_device_format_suffix("Ground Control (16000Hz, 1 ch)"),
            "Ground Control"
        );
        assert_eq!(
            strip_device_format_suffix("MacBook Pro Microphone (48000Hz, 2 ch)"),
            "MacBook Pro Microphone"
        );
    }

    #[test]
    fn strip_device_format_suffix_passes_through_bare_names() {
        assert_eq!(
            strip_device_format_suffix("Ground Control"),
            "Ground Control"
        );
        assert_eq!(strip_device_format_suffix(""), "");
    }

    #[test]
    fn strip_device_format_suffix_ignores_non_matching_parens() {
        // Device names can legitimately contain parentheses; only the exact
        // "(NHz, N ch)" format produced by list_input_devices should be
        // stripped.
        assert_eq!(
            strip_device_format_suffix("USB Mic (rev 2)"),
            "USB Mic (rev 2)"
        );
        assert_eq!(
            strip_device_format_suffix("Mic (16000Hz, two ch)"),
            "Mic (16000Hz, two ch)"
        );
        assert_eq!(
            strip_device_format_suffix("Mic (abcHz, 1 ch)"),
            "Mic (abcHz, 1 ch)"
        );
    }

    #[test]
    fn strip_device_format_suffix_roundtrips_list_output() {
        // Anything list_input_devices_detailed produces as a label must
        // strip back to the canonical name.
        let entry = InputDeviceEntry {
            name: "Ground Control".into(),
            label: "Ground Control (16000Hz, 1 ch)".into(),
        };
        assert_eq!(strip_device_format_suffix(&entry.label), entry.name);
    }

    #[test]
    fn canonicalize_input_device_setting_strips_picker_decoration() {
        assert_eq!(
            canonicalize_input_device_setting(" Ground Control (16000Hz, 1 ch) "),
            Some("Ground Control".into())
        );
        assert_eq!(
            canonicalize_input_device_setting("Ground Control"),
            Some("Ground Control".into())
        );
        assert_eq!(canonicalize_input_device_setting("   "), None);
    }

    #[test]
    fn meeting_audio_artifact_paths_include_stems_and_embeddings_sidecar() {
        let markdown = Path::new("/tmp/meetings/2026-04-01-standup.md");
        let artifacts = meeting_audio_artifact_paths(markdown);
        let audio_path = markdown.with_extension("wav");
        let stems = stem_paths_for(&audio_path).expect("expected stem paths for meeting audio");

        assert_eq!(
            artifacts,
            vec![
                audio_path,
                stems.voice,
                stems.system,
                crate::voice::meeting_embeddings_sidecar_path(markdown),
            ]
        );
    }

    #[test]
    fn categorize_pipewire_sink_returns_system_audio() {
        // PipeWire host: a sink (output device) appears in input_devices() with
        // supports_output() == true. This is a "monitor source" and should be
        // categorized as SystemAudio so users know they can record system audio
        // from it.
        let category = categorize_device(
            "Built-in Audio Analog Stereo",
            true, // supports_input (cpal already filtered)
            true, // supports_output (the Duplex signal)
            true, // is_pipewire
        );
        assert_eq!(category, DeviceCategory::SystemAudio);
    }

    #[test]
    fn categorize_pipewire_real_microphone_returns_microphone() {
        // PipeWire host: a real microphone (Audio/Source) only supports input.
        let category = categorize_device(
            "Built-in Audio Analog Mono",
            true,
            false, // input-only
            true,
        );
        assert_eq!(category, DeviceCategory::Microphone);
    }

    #[test]
    fn categorize_alsa_duplex_does_not_become_system_audio() {
        // ALSA host: a USB headset that genuinely supports both directions on
        // the same node. Without the host gate, this would be wrongly categorized
        // as SystemAudio. This test pins the gate behavior — if it ever fails,
        // someone removed the is_pipewire check and broke ALSA users.
        let category = categorize_device(
            "USB Headset Mono",
            true,
            true,
            false, // NOT pipewire — this is the ALSA path
        );
        assert_eq!(category, DeviceCategory::Microphone);
    }

    #[test]
    fn categorize_macos_camo_microphone_does_not_become_system_audio() {
        // macOS-style false positive: Camo Microphone is a real mic that bridges
        // to an iPhone, but CoreAudio reports it as Duplex (verified empirically
        // on Mat's machine, see PLAN.md.cpal-pipewire-fix P2 results). The
        // PipeWire-only gate prevents this false positive.
        let category = categorize_device("Camo Microphone", true, true, false);
        assert_eq!(category, DeviceCategory::Microphone);
    }

    #[test]
    fn categorize_loopback_device_by_name_still_works() {
        // Existing name-based heuristic still applies as a fallback.
        let category = categorize_device("Descript Loopback Recorder", true, true, false);
        assert_eq!(category, DeviceCategory::SystemAudio);
    }

    #[test]
    fn categorize_virtual_device_by_name_still_works() {
        let category = categorize_device("VirtualMicSomething", true, false, false);
        assert_eq!(category, DeviceCategory::Virtual);
    }

    #[test]
    fn categorize_blackhole_still_works() {
        // BlackHole is an output-only loopback driver on macOS — supports_output
        // is false from the input_devices() perspective in our existing setup,
        // but the name heuristic catches it.
        let category = categorize_device("BlackHole 2ch", true, false, false);
        assert_eq!(category, DeviceCategory::SystemAudio);
    }

    #[test]
    fn categorize_mmaudio_returns_system_audio() {
        // MMAudio is a virtual loopback driver on macOS. Both the main device
        // and the "(UI Sounds)" variant should be SystemAudio.
        assert_eq!(
            categorize_device("MMAudio Device", true, true, false),
            DeviceCategory::SystemAudio
        );
        assert_eq!(
            categorize_device("MMAudio Device (UI Sounds)", true, true, false),
            DeviceCategory::SystemAudio
        );
    }

    #[test]
    fn categorize_loom_audio_device_returns_system_audio() {
        // Loom installs LoomAudioDevice for screen-recording audio capture.
        let category = categorize_device("LoomAudioDevice", true, true, false);
        assert_eq!(category, DeviceCategory::SystemAudio);
    }

    #[test]
    fn categorize_zoom_audio_device_returns_system_audio() {
        // Zoom installs ZoomAudioDevice for screen-share audio capture.
        let category = categorize_device("ZoomAudioDevice", true, true, false);
        assert_eq!(category, DeviceCategory::SystemAudio);
    }

    #[test]
    fn categorize_teams_audio_returns_system_audio() {
        // Microsoft Teams installs "Microsoft Teams Audio" for call audio routing.
        let category = categorize_device("Microsoft Teams Audio", true, true, false);
        assert_eq!(category, DeviceCategory::SystemAudio);
    }

    #[test]
    fn categorize_pulseaudio_monitor_source_returns_system_audio() {
        // PulseAudio exposes monitor sources as separate Source devices with
        // names ending in `.monitor`. They appear in input_devices() like real
        // mics and were previously lumped into Microphone.
        let category = categorize_device(
            "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor",
            true,
            false, // PulseAudio Source has direction = Input, not Duplex
            false, // not pipewire
        );
        assert_eq!(category, DeviceCategory::SystemAudio);
    }

    #[test]
    fn categorize_camo_microphone_is_not_false_positive() {
        // Critical regression test: Camo Microphone is a REAL microphone
        // (via a 2-way iPhone bridge) that also reports as Duplex on macOS.
        // None of the new hints should accidentally match it. If this test
        // ever fails, someone added a hint that's too broad.
        let category = categorize_device("Camo Microphone", true, true, false);
        assert_eq!(category, DeviceCategory::Microphone);
    }

    #[test]
    fn categorize_real_mic_named_with_monitor_substring_does_not_false_positive() {
        // Edge case: a microphone named "Studio Monitor Microphone" or similar
        // contains "monitor" but should NOT be SystemAudio. The PulseAudio
        // pattern uses ".monitor" specifically (with the leading dot), so this
        // should fall through to Microphone.
        let category = categorize_device("Studio Monitor Microphone", true, false, false);
        assert_eq!(category, DeviceCategory::Microphone);
    }

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

    #[test]
    fn native_call_capture_bypass_only_applies_to_call_auto_loopback_failure() {
        let mut config = Config::default();
        config.recording.sources = Some(crate::config::SourcesConfig {
            voice: Some("default".into()),
            call: Some("auto".into()),
        });

        let loopback_error =
            CaptureError::Io(std::io::Error::other(MISSING_DUAL_SOURCE_LOOPBACK_MESSAGE));
        assert!(should_bypass_loopback_preflight_for_native_call_capture(
            RecordingIntent::Call,
            true,
            &config,
            &loopback_error,
        ));

        assert!(!should_bypass_loopback_preflight_for_native_call_capture(
            RecordingIntent::Room,
            true,
            &config,
            &loopback_error,
        ));

        assert!(!should_bypass_loopback_preflight_for_native_call_capture(
            RecordingIntent::Call,
            false,
            &config,
            &loopback_error,
        ));

        config.recording.sources = Some(crate::config::SourcesConfig {
            voice: Some("default".into()),
            call: Some("BlackHole 2ch".into()),
        });
        assert!(!should_bypass_loopback_preflight_for_native_call_capture(
            RecordingIntent::Call,
            true,
            &config,
            &loopback_error,
        ));

        let different_error = CaptureError::Io(std::io::Error::other("different error"));
        config.recording.sources = Some(crate::config::SourcesConfig {
            voice: Some("default".into()),
            call: Some("auto".into()),
        });
        assert!(!should_bypass_loopback_preflight_for_native_call_capture(
            RecordingIntent::Call,
            true,
            &config,
            &different_error,
        ));
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

    /// Round-trip: `set_preferred_host_id` is observable via `preferred_host_id`.
    /// Uses a serial guard because `PREFERRED_HOST` is process-wide static and
    /// other parallel tests could otherwise race the read/write.
    #[test]
    fn preferred_host_cache_round_trips() {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = LOCK.lock().unwrap();

        let prior = preferred_host_id();
        let id = cached_default_host().id();
        set_preferred_host_id(id);
        assert_eq!(preferred_host_id(), Some(id));

        // Restore prior state so other tests see what they expect.
        if let Some(p) = prior {
            set_preferred_host_id(p);
        } else {
            // No public clear; overwrite with current default is fine for tests.
            set_preferred_host_id(id);
        }
    }
}
