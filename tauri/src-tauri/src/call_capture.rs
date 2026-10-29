use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallCaptureAvailability {
    Available { backend: String },
    PermissionRequired { detail: String },
    Unavailable { detail: String },
    Unsupported { detail: String },
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CallSourceHealth {
    pub backend: String,
    pub mic_live: bool,
    pub call_audio_live: bool,
    pub mic_level: u32,
    pub call_audio_level: u32,
    pub mic_frames_silenced: u64,
    pub system_frames_silenced: u64,
    pub last_update: String,
    /// Set by the recording loop while the live remote-silence alert is
    /// outstanding (#1266): the system source may be "live" (delivering
    /// buffers) yet carry no remote audio, which `call_audio_live` alone
    /// cannot show.
    pub remote_audio_silent: bool,
}

/// Map one helper health snapshot to a remote-silence detector sample.
///
/// `None` when the microphone is not delivering (including stale telemetry,
/// which `source_health` reports as not live): without the local side the
/// detector cannot tell "the remote is silent" from "nobody is talking". A
/// system source that is attached but not delivering buffers reads as level 0,
/// because that is still "not hearing the other side".
pub(crate) fn remote_silence_sample(
    health: &CallSourceHealth,
) -> Option<minutes_core::remote_silence::LevelSample> {
    if !health.mic_live {
        return None;
    }
    Some(minutes_core::remote_silence::LevelSample {
        mic_level: health.mic_level,
        system_level: if health.call_audio_live {
            health.call_audio_level
        } else {
            0
        },
    })
}

const HELPER_STDERR_INITIAL_LOG_LIMIT: usize = 5;
const HELPER_STDERR_LOG_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct HelperStderrRateLimiter {
    initial_lines_logged: usize,
    suppressed_lines: u64,
    last_log_at: Instant,
}

impl HelperStderrRateLimiter {
    fn new(now: Instant) -> Self {
        Self {
            initial_lines_logged: 0,
            suppressed_lines: 0,
            last_log_at: now,
        }
    }

    /// Returns the number of lines suppressed before the current line when
    /// the current line should be logged, or `None` when it should be hidden.
    fn observe(&mut self, now: Instant) -> Option<u64> {
        if self.initial_lines_logged < HELPER_STDERR_INITIAL_LOG_LIMIT {
            self.initial_lines_logged += 1;
            self.last_log_at = now;
            return Some(0);
        }
        if now.duration_since(self.last_log_at) >= HELPER_STDERR_LOG_INTERVAL {
            let suppressed = std::mem::take(&mut self.suppressed_lines);
            self.last_log_at = now;
            return Some(suppressed);
        }
        self.suppressed_lines += 1;
        None
    }
}

fn spawn_helper_stderr_drain(stderr: std::process::ChildStderr) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        let mut limiter = HelperStderrRateLimiter::new(Instant::now());
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if let Some(suppressed) = limiter.observe(Instant::now()) {
                        tracing::warn!(
                            helper_stderr = line,
                            suppressed_since_last = suppressed,
                            "native call helper stderr"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(error = %error, "failed to drain native call helper stderr");
                    break;
                }
            }
        }
        if limiter.suppressed_lines > 0 {
            tracing::warn!(
                suppressed = limiter.suppressed_lines,
                "native call helper stderr lines suppressed before stream closed"
            );
        }
    });
}

pub(crate) fn capture_warning_for_audio_format_unsupported(value: &serde_json::Value) -> String {
    let source = value
        .get("source")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown source");
    let field = |name: &str| {
        value
            .get(name)
            .map(serde_json::Value::to_string)
            .unwrap_or_else(|| "unknown".into())
    };
    format!(
        "Native call capture silenced undecodable {source} audio (ASBD: sample_rate={}, format_id={}, format_flags={}, bits_per_channel={}, bytes_per_packet={}, bytes_per_frame={}, frames_per_packet={}, channels={}).",
        field("sample_rate"),
        field("format_id"),
        field("format_flags"),
        field("bits_per_channel"),
        field("bytes_per_packet"),
        field("bytes_per_frame"),
        field("frames_per_packet"),
        field("channels"),
    )
}

/// Paths to per-source audio stems written by the native call helper.
#[derive(Debug, Clone)]
pub struct StemPaths {
    pub voice: PathBuf,
    pub system: PathBuf,
}

// Successful writes include digital silence. Health heartbeats alone do not
// reset the audio clock, and missing counters are not interpreted as zero.
const CAPTURE_PROGRESS_WARNING: Duration = Duration::from_secs(30);
const CAPTURE_PROGRESS_STOP: Duration = Duration::from_secs(120);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CaptureProgressAction {
    None,
    Warning(String),
    Interrupted(String),
    Recovered(String),
    Stop(String),
}

#[derive(Debug, Clone, serde::Serialize)]
struct CaptureInterruption {
    reason: &'static str,
    error_domain: Option<&'static str>,
    error_code: Option<i64>,
    observed_at: Option<String>,
    will_retry: bool,
    attempt: u64,
}

impl CaptureInterruption {
    fn from_helper_event(value: &serde_json::Value) -> Option<Self> {
        if value.get("event")?.as_str()? != "capture_interrupted" {
            return None;
        }
        let reason = match value.get("reason")?.as_str()? {
            "stream_error" => "stream_error",
            "microphone_lost" => "microphone_lost",
            "unexpected_output_finish" => "unexpected_output_finish",
            _ => return None,
        };
        Some(Self {
            reason,
            error_domain: match value.get("error_domain").and_then(|value| value.as_str()) {
                Some("com.apple.ScreenCaptureKit.SCStreamErrorDomain") => {
                    Some("com.apple.ScreenCaptureKit.SCStreamErrorDomain")
                }
                _ => None,
            },
            error_code: value.get("error_code").and_then(|value| value.as_i64()),
            observed_at: value
                .get("observed_at")
                .and_then(|value| value.as_str())
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.to_rfc3339()),
            will_retry: value
                .get("will_retry")
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
            attempt: value
                .get("attempt")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
        })
    }

    fn message(&self) -> String {
        let detail = match (self.reason, self.error_code) {
            ("microphone_lost", _) => "Microphone capture stopped delivering audio.".into(),
            ("recovery_failed", _) => "Automatic call audio recovery did not complete.".into(),
            (_, Some(code)) => {
                format!("macOS reported a call capture interruption (error {code}).")
            }
            (_, None) => "Call capture finished without a stop request.".into(),
        };
        if self.will_retry {
            format!("{detail} Minutes is reconnecting audio automatically and keeping this meeting open. Any missing interval will be marked in the saved recording.")
        } else {
            format!("{detail} Minutes could not keep audio capture running and is saving the captured segment. Start recording again to capture the rest of the call.")
        }
    }
}

pub(crate) fn native_call_exit_is_unexpected(success: bool, stop_requested: bool) -> bool {
    !success || !stop_requested
}

#[derive(Debug)]
struct CaptureProgressMonitor {
    last_health: Instant,
    last_audio: Instant,
    frames: Option<(u64, u64)>,
    warned: bool,
    last_file_progress: Instant,
    last_file_observation: Option<Instant>,
    file_sizes: (u64, u64),
    interruption: Option<CaptureInterruption>,
    interruption_reported: bool,
    recovery_notice: Option<String>,
    last_source_audio: [Instant; 2],
    source_warned: [bool; 2],
    source_notice: Option<String>,
}

impl CaptureProgressMonitor {
    fn new(now: Instant) -> Self {
        Self {
            last_health: now,
            last_audio: now,
            frames: None,
            warned: false,
            last_file_progress: now,
            last_file_observation: None,
            file_sizes: (0, 0),
            interruption: None,
            interruption_reported: false,
            recovery_notice: None,
            last_source_audio: [now; 2],
            source_warned: [false; 2],
            source_notice: None,
        }
    }

    fn observe(&mut self, now: Instant, value: &serde_json::Value) {
        self.last_health = now;
        let frames = value
            .get("mic_frames_written")
            .and_then(|v| v.as_u64())
            .zip(value.get("system_frames_written").and_then(|v| v.as_u64()));
        if let Some(frames) = frames {
            let previous = self.frames.unwrap_or((0, 0));
            for (index, (current, prior)) in [(frames.0, previous.0), (frames.1, previous.1)]
                .into_iter()
                .enumerate()
            {
                if current > prior {
                    self.last_source_audio[index] = now;
                    self.source_warned[index] = false;
                }
            }
            if frames.0 > previous.0 || frames.1 > previous.1 {
                self.last_audio = now;
                self.warned = false;
            }
            self.frames = Some(frames);
        }
    }

    fn observe_files(&mut self, now: Instant, sizes: (u64, u64)) {
        if sizes.0 > self.file_sizes.0 || sizes.1 > self.file_sizes.1 {
            self.last_file_progress = now;
        }
        self.file_sizes = sizes;
        self.last_file_observation = Some(now);
    }

    fn check(&mut self, now: Instant) -> CaptureProgressAction {
        if let Some(message) = self.source_notice.take() {
            return CaptureProgressAction::Warning(message);
        }
        if let Some(message) = self.recovery_notice.take() {
            return CaptureProgressAction::Recovered(message);
        }
        if let Some(interruption) = &self.interruption {
            if !self.interruption_reported {
                self.interruption_reported = true;
                return CaptureProgressAction::Interrupted(interruption.message());
            }
            // Keep the file-progress safety net while the output finalizes.
        }
        let health_age = now.saturating_duration_since(self.last_health);
        let audio_age = now.saturating_duration_since(self.last_audio);
        let (age, detail) = if health_age >= CAPTURE_PROGRESS_WARNING {
            (
                health_age,
                "The call capture helper stopped reporting audio progress.",
            )
        } else if self.frames.is_none() {
            (
                audio_age,
                "The call capture helper is not providing successful audio-write counters.",
            )
        } else {
            (
                audio_age,
                "The helper has not reported new call-audio or microphone writes recently.",
            )
        };
        // A telemetry reader failure must not stop healthy capture. Require
        // fresh, independent evidence that BOTH stem files stopped growing.
        let file_stall_confirmed = self.last_file_observation.is_some_and(|observed| {
            now.saturating_duration_since(observed) < Duration::from_secs(3)
        }) && now.saturating_duration_since(self.last_file_progress)
            >= CAPTURE_PROGRESS_STOP;
        if audio_age >= CAPTURE_PROGRESS_STOP && file_stall_confirmed {
            return CaptureProgressAction::Stop(format!(
                "Both audio stems stopped growing and successful-write telemetry has not advanced. Minutes stopped capture after {} seconds without verified progress and is saving the captured audio.",
                audio_age.as_secs()
            ));
        }
        if age >= CAPTURE_PROGRESS_WARNING && !self.warned && self.interruption.is_none() {
            self.warned = true;
            return CaptureProgressAction::Warning(format!(
                "{detail} Minutes will stop capture if both stem files also remain stalled for two minutes."
            ));
        }
        // Preserve a surviving lane. Only a verified BOTH-source stall can
        // stop capture, but one progressing source must not hide another loss.
        if health_age < CAPTURE_PROGRESS_WARNING && audio_age < CAPTURE_PROGRESS_WARNING {
            if let Some(frames) = self.frames {
                for (index, source) in ["Microphone", "System audio"].into_iter().enumerate() {
                    // System audio may legitimately never start in a room meeting.
                    let expected = index == 0 || frames.1 > 0;
                    if expected
                        && !self.source_warned[index]
                        && now.saturating_duration_since(self.last_source_audio[index])
                            >= CAPTURE_PROGRESS_WARNING
                    {
                        self.source_warned[index] = true;
                        return CaptureProgressAction::Warning(format!(
                            "{source} capture has stopped delivering audio. Minutes is preserving the other input, but speech from this source may be missing. Check the recording inputs."
                        ));
                    }
                }
            }
        }
        CaptureProgressAction::None
    }

    fn resume(&mut self, attempt: u64, now: Instant, gap_ms: u64) -> Option<String> {
        let interruption = self.interruption.as_ref()?;
        // A buffered resume event from an older generation cannot clear a
        // newer failure or a terminal reconnect failure.
        if !interruption.will_retry || attempt <= interruption.attempt || attempt > 64 {
            return None;
        }
        self.interruption = None;
        self.interruption_reported = false;
        self.last_audio = now;
        self.last_health = now;
        self.warned = false;
        let message = format!(
            "Call audio reconnected automatically. The meeting remained open. Capture was interrupted for approximately {:.1} seconds; that interval may contain missing speech.",
            gap_ms.min(3_600_000) as f64 / 1000.0
        );
        self.recovery_notice = Some(message.clone());
        Some(message)
    }
}

fn spawn_stem_progress_monitor(
    output_path: &Path,
    monitor: Arc<Mutex<CaptureProgressMonitor>>,
) -> Arc<std::sync::atomic::AtomicBool> {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let stem = output_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let voice = output_path.with_file_name(format!("{stem}.voice.wav"));
    let system = output_path.with_file_name(format!("{stem}.system.wav"));
    std::thread::spawn(move || {
        let size = |path: &Path| -> Option<u64> {
            match std::fs::metadata(path) {
                Ok(meta) if meta.is_file() => Some(meta.len()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(0),
                _ => None,
            }
        };
        while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
            let observation_started = Instant::now();
            if let Some(sizes) = size(&voice).zip(size(&system)) {
                if let Ok(mut monitor) = monitor.lock() {
                    monitor.observe_files(observation_started, sizes);
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    stop
}

pub struct NativeCallCaptureSession {
    child: Child,
    output_path: PathBuf,
    health: Arc<Mutex<CallSourceHealth>>,
    #[allow(dead_code)] // used once pipeline stem attribution is wired up
    stem_paths: Arc<Mutex<Option<StemPaths>>>,
    /// Bumped by the stdout reader thread whenever the helper emits any line
    /// (health, stems, or `finalizing` heartbeats during stop). Used by `stop()`
    /// to distinguish a helper that is genuinely working through finalize from
    /// one that is hung, instead of relying on a fixed wall-clock timeout. The
    /// fixed 15s ceiling caused issue #216: long captures' moov-atom finalize
    /// takes longer than 15s, the helper was SIGKILLed, and the .mov was
    /// truncated with no `moov` box.
    last_progress: Arc<Mutex<Instant>>,
    capture_warnings: Arc<Mutex<Vec<String>>>,
    capture_progress: Arc<Mutex<CaptureProgressMonitor>>,
    stem_monitor_stop: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MicrophoneSelection {
    Configured(String),
    ConfiguredMissing(String),
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MicrophoneStartupStatus {
    Selected(String),
    Fallback(String),
}

const MICROPHONE_STARTUP_STATUS_TIMEOUT: Duration = Duration::from_secs(2);

fn finish_microphone_startup_handshake(
    status_expected: bool,
    status_rx: &mpsc::Receiver<MicrophoneStartupStatus>,
    capture_warnings: &Arc<Mutex<Vec<String>>>,
    timeout: Duration,
) {
    if !status_expected {
        return;
    }

    match status_rx.recv_timeout(timeout) {
        Ok(MicrophoneStartupStatus::Selected(name)) => {
            tracing::info!(
                microphone = name,
                "native call capture is using configured microphone"
            );
        }
        Ok(MicrophoneStartupStatus::Fallback(name)) => {
            let message = format!("configured mic not found, using default: '{}'.", name);
            tracing::warn!(
                microphone = name,
                "configured mic disappeared before native call capture started; using default"
            );
            if let Ok(mut warnings) = capture_warnings.lock() {
                if !warnings.contains(&message) {
                    warnings.push(message);
                }
            }
        }
        Err(error) => {
            let message = format!(
                "Configured microphone selection was not acknowledged before capture start ({error}); verify the selected input."
            );
            tracing::warn!(error = %error, "native call microphone startup status timed out");
            if let Ok(mut warnings) = capture_warnings.lock() {
                warnings.push(message);
            }
        }
    }
}

fn resolve_microphone_selection(
    configured: Option<&str>,
    available_names: &[String],
) -> MicrophoneSelection {
    let configured = configured.map(str::trim).filter(|name| !name.is_empty());
    match configured {
        Some(name) if available_names.iter().any(|candidate| candidate == name) => {
            MicrophoneSelection::Configured(name.to_string())
        }
        Some(name) => MicrophoneSelection::ConfiguredMissing(name.to_string()),
        None => MicrophoneSelection::Default,
    }
}

fn native_call_helper_args(output_path: &Path, selection: &MicrophoneSelection) -> Vec<OsString> {
    let mut args = vec![output_path.as_os_str().to_os_string()];
    if let MicrophoneSelection::Configured(name) = selection {
        args.push(OsString::from("--microphone-name"));
        args.push(OsString::from(name));
    }
    args
}

#[derive(Debug, serde::Deserialize)]
struct MicrophoneInventory {
    devices: Vec<String>,
}

/// Absolute ceiling on `stop()` after SIGTERM is sent. The helper has to be
/// genuinely making progress (see `last_progress`) within this window or it
/// gets SIGKILLed. Sized for very long captures: a multi-hour recording can
/// take tens of seconds to write the moov atom, but a 10-minute hang is
/// indistinguishable from "wedged" and we'd rather surface a recoverable
/// failure than wait forever.
const STOP_MAX_FINALIZE: Duration = Duration::from_secs(600);

/// Maximum time without any stdout activity from the helper before we treat
/// it as hung and SIGKILL. Health events arrive every ~150ms while capturing
/// and finalizing heartbeats every ~1s during `stream.stopCapture()`, so 30s
/// of silence is unambiguous.
const STOP_PROGRESS_TIMEOUT: Duration = Duration::from_secs(30);

fn parse_macos_major_version(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse().ok()
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_major_version() -> Option<u32> {
    let output = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_macos_major_version(&String::from_utf8_lossy(&output.stdout))
}

impl Drop for NativeCallCaptureSession {
    fn drop(&mut self) {
        self.stem_monitor_stop
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl NativeCallCaptureSession {
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }

    /// Return per-source stem paths if the helper reported them and the files exist.
    #[allow(dead_code)] // used once pipeline stem attribution is wired up
    pub fn stem_paths(&self) -> Option<StemPaths> {
        self.stem_paths
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .filter(|stems| stems.voice.exists() && stems.system.exists())
    }

    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, String> {
        self.child.try_wait().map_err(|error| error.to_string())
    }

    pub(crate) fn check_capture_progress(&self) -> CaptureProgressAction {
        let action = self
            .capture_progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .check(Instant::now());
        if let CaptureProgressAction::Warning(message) = &action {
            if let Ok(mut warnings) = self.capture_warnings.lock() {
                if !warnings.contains(message) {
                    warnings.push(message.clone());
                }
            }
        }
        action
    }

    pub fn source_health(&self) -> CallSourceHealth {
        let stale = self
            .capture_progress
            .lock()
            .map(|monitor| {
                monitor.interruption.is_some()
                    || monitor.last_health.elapsed() >= Duration::from_secs(3)
            })
            .unwrap_or(true);
        self.health
            .lock()
            .map(|health| {
                let mut health = health.clone();
                if stale {
                    health.mic_live = false;
                    health.call_audio_live = false;
                    health.mic_level = 0;
                    health.call_audio_level = 0;
                }
                health
            })
            .unwrap_or_else(|_| CallSourceHealth {
                backend: "screencapturekit-helper".into(),
                mic_live: false,
                call_audio_live: false,
                mic_level: 0,
                call_audio_level: 0,
                mic_frames_silenced: 0,
                system_frames_silenced: 0,
                last_update: chrono::Local::now().to_rfc3339(),
                remote_audio_silent: false,
            })
    }

    pub fn capture_warning(&self) -> Option<String> {
        self.capture_warnings
            .lock()
            .ok()
            .and_then(|warnings| (!warnings.is_empty()).then(|| warnings.join(" ")))
    }

    /// Send SIGKILL after a timeout, but reap once more first so a helper
    /// that exited successfully between the loop's last `try_wait` and now
    /// is reported as a clean stop rather than a kill failure. Without this,
    /// the helper-exits-during-the-sleep race surfaced a spurious error to
    /// the caller and stranded the .mov in `failed-captures/` even though
    /// the child wrote the moov atom before exiting.
    #[cfg(target_os = "macos")]
    fn giveup_with_kill(&mut self, kill_reason: String) -> Result<(), String> {
        if let Ok(Some(status)) = self.child.try_wait() {
            if status.success() {
                return Ok(());
            }
            return Err(format!("native call helper exited with status {}", status));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        Err(kill_reason)
    }

    pub fn stop(&mut self) -> Result<(), String> {
        #[cfg(not(target_os = "macos"))]
        {
            return Err("native call capture is unsupported on this platform".into());
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                if status.success() {
                    return Ok(());
                }
                return Err(format!("native call helper exited with status {}", status));
            }

            let pid = self.child.id();
            let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            if rc != 0 {
                let error = std::io::Error::last_os_error();
                let _ = self.child.kill();
                return Err(format!(
                    "failed to stop native call helper (PID {}): {}",
                    pid, error
                ));
            }

            // Bump last_progress to "now" so the stdout-activity check doesn't
            // mis-fire on a stale timestamp from before SIGTERM (the loop's
            // grace period restarts from when we asked the helper to stop).
            if let Ok(mut t) = self.last_progress.lock() {
                *t = Instant::now();
            }

            let start = Instant::now();
            loop {
                if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                    if status.success() {
                        return Ok(());
                    }
                    return Err(format!("native call helper exited with status {}", status));
                }

                if start.elapsed() >= STOP_MAX_FINALIZE {
                    return self.giveup_with_kill(format!(
                        "native call helper did not finalize within {}s (absolute ceiling); SIGKILLed. Per-source stems may still be recoverable in ~/.minutes/native-captures/.",
                        STOP_MAX_FINALIZE.as_secs()
                    ));
                }

                // Poisoned mutex = stdout reader thread panicked. Treat that
                // as "no progress possible" and short-circuit to SIGKILL via
                // the progress-timeout branch. The previous fallback of ZERO
                // would have kept the loop spinning until the 600s ceiling.
                let since_progress = self
                    .last_progress
                    .lock()
                    .map(|t| t.elapsed())
                    .unwrap_or(Duration::MAX);
                if since_progress >= STOP_PROGRESS_TIMEOUT {
                    return self.giveup_with_kill(format!(
                        "native call helper went silent for {}s during finalize; SIGKILLed. Per-source stems may still be recoverable in ~/.minutes/native-captures/.",
                        STOP_PROGRESS_TIMEOUT.as_secs()
                    ));
                }

                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

pub fn availability() -> CallCaptureAvailability {
    #[cfg(not(target_os = "macos"))]
    {
        return CallCaptureAvailability::Unsupported {
            detail: "Native call capture is currently implemented on macOS only.".into(),
        };
    }

    #[cfg(target_os = "macos")]
    {
        match macos_major_version() {
            Some(major) if major < 15 => {
                return CallCaptureAvailability::Unsupported {
                    detail: format!(
                        "Native call capture requires macOS 15 or newer. This Mac reports macOS {}.",
                        major
                    ),
                };
            }
            None => {
                return CallCaptureAvailability::Unavailable {
                    detail: "Could not determine the macOS version for native call capture.".into(),
                };
            }
            _ => {}
        }

        match find_native_call_helper_binary() {
            Some(_) => CallCaptureAvailability::Available {
                backend: "screencapturekit-helper".into(),
            },
            None => CallCaptureAvailability::Unavailable {
                detail: "Bundled native call helper is missing from the app bundle.".into(),
            },
        }
    }
}

#[cfg(target_os = "macos")]
pub fn start_native_call_capture(
    configured_microphone: Option<&str>,
    enable_recovery_test_signal: bool,
) -> Result<NativeCallCaptureSession, String> {
    if let Some(major) = macos_major_version() {
        if major < 15 {
            return Err(format!(
                "native call capture requires macOS 15 or newer (found macOS {})",
                major
            ));
        }
    }

    let helper = find_native_call_helper_binary()
        .ok_or_else(|| "native call helper binary is unavailable".to_string())?;
    let mut capture_warnings = Vec::new();
    let microphone_selection = if configured_microphone
        .map(str::trim)
        .is_some_and(|name| !name.is_empty())
    {
        match list_native_call_microphones(&helper) {
            Ok(inventory) => {
                resolve_microphone_selection(configured_microphone, &inventory.devices)
            }
            Err(error) => {
                let name = configured_microphone.unwrap_or_default().trim();
                tracing::warn!(
                    configured_microphone = name,
                    error = %error,
                    "could not enumerate native-call microphones; using default"
                );
                MicrophoneSelection::ConfiguredMissing(name.to_string())
            }
        }
    } else {
        MicrophoneSelection::Default
    };
    if let MicrophoneSelection::ConfiguredMissing(name) = &microphone_selection {
        let message = format!("configured mic not found, using default: '{}'.", name);
        tracing::warn!(
            configured_microphone = name,
            "configured mic not found, using default"
        );
        capture_warnings.push(message);
    }
    let output_path = native_call_output_path()?;
    let health = Arc::new(Mutex::new(CallSourceHealth {
        backend: "screencapturekit-helper".into(),
        mic_live: false,
        call_audio_live: false,
        mic_level: 0,
        call_audio_level: 0,
        mic_frames_silenced: 0,
        system_frames_silenced: 0,
        last_update: chrono::Local::now().to_rfc3339(),
        remote_audio_silent: false,
    }));
    let mut command = Command::new(&helper);
    command.args(native_call_helper_args(&output_path, &microphone_selection));
    if enable_recovery_test_signal {
        command.arg("--enable-recovery-test-signal");
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start native call helper: {}", error))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "native call helper did not expose stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "native call helper did not expose stderr".to_string())?;
    spawn_helper_stderr_drain(stderr);
    let (tx, rx) = mpsc::channel();
    let (microphone_status_tx, microphone_status_rx) = mpsc::channel();
    let microphone_status_expected =
        matches!(microphone_selection, MicrophoneSelection::Configured(_));
    let stem_paths: Arc<Mutex<Option<StemPaths>>> = Arc::new(Mutex::new(None));
    let last_progress: Arc<Mutex<Instant>> = Arc::new(Mutex::new(Instant::now()));
    let capture_progress = Arc::new(Mutex::new(CaptureProgressMonitor::new(Instant::now())));
    let capture_progress_for_thread = Arc::clone(&capture_progress);
    let health_for_thread = Arc::clone(&health);
    let stems_for_thread = Arc::clone(&stem_paths);
    let progress_for_thread = Arc::clone(&last_progress);
    let capture_warnings = Arc::new(Mutex::new(capture_warnings));
    let warnings_for_thread = Arc::clone(&capture_warnings);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let mut ready_sent = false;

        loop {
            line.clear();
            let read = match reader.read_line(&mut line) {
                Ok(read) => read,
                Err(error) => {
                    if !ready_sent {
                        let _ = tx.send(Err(format!(
                            "failed to read native call helper output: {}",
                            error
                        )));
                    }
                    break;
                }
            };

            if read == 0 {
                if !ready_sent {
                    let _ = tx.send(Err(
                        "native call helper exited before signaling readiness".into()
                    ));
                }
                break;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Any line from the helper counts as "still alive" for the stop()
            // progress check. This includes `finalizing` heartbeats emitted
            // every ~1s during stream.stopCapture(), which is what lets long
            // (multi-hour) captures finalize without being SIGKILLed. See #216.
            if let Ok(mut t) = progress_for_thread.lock() {
                *t = Instant::now();
            }

            if !ready_sent {
                ready_sent = true;
                let _ = tx.send(Ok(trimmed.to_string()));
                continue;
            }

            if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
                match value.get("event").and_then(|v| v.as_str()) {
                    Some("capture_test_interruption") => {
                        minutes_core::logging::log_step(
                            "native_call_test_interruption",
                            "native-call",
                            0,
                            serde_json::json!({"injected": true}),
                        );
                    }
                    Some("capture_interrupted") => {
                        if let Some(interruption) = CaptureInterruption::from_helper_event(&value) {
                            let first = capture_progress_for_thread
                                .lock()
                                .map(|mut monitor| {
                                    if monitor.interruption.as_ref().is_some_and(|previous| {
                                        previous.attempt >= interruption.attempt
                                    }) {
                                        return false;
                                    }
                                    monitor.interruption = Some(interruption.clone());
                                    monitor.interruption_reported = false;
                                    monitor.recovery_notice = None;
                                    true
                                })
                                .unwrap_or(false);
                            if first {
                                minutes_core::logging::log_step(
                                    "native_call_interrupted",
                                    "native-call",
                                    0,
                                    serde_json::json!(interruption),
                                );
                                if let Ok(mut warnings) = warnings_for_thread.lock() {
                                    warnings.push(interruption.message());
                                }
                            }
                        }
                    }
                    Some("capture_resumed") => {
                        let attempt = value
                            .get("attempt")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(0);
                        let gap_ms = value
                            .get("gap_ms")
                            .and_then(|value| value.as_u64())
                            .unwrap_or(0)
                            .min(3_600_000);
                        let message =
                            capture_progress_for_thread
                                .lock()
                                .ok()
                                .and_then(|mut monitor| {
                                    monitor.resume(attempt, Instant::now(), gap_ms)
                                });
                        if let Some(message) = message {
                            minutes_core::logging::log_step(
                                "native_call_resumed",
                                "native-call",
                                0,
                                serde_json::json!({"attempt": attempt, "gap_ms": gap_ms}),
                            );
                            if let Ok(mut warnings) = warnings_for_thread.lock() {
                                warnings.push(message);
                            }
                        }
                    }
                    Some("capture_recovery_failed") => {
                        if let Ok(mut monitor) = capture_progress_for_thread.lock() {
                            let mut failure =
                                monitor.interruption.clone().unwrap_or(CaptureInterruption {
                                    reason: "recovery_failed",
                                    error_domain: None,
                                    error_code: None,
                                    observed_at: None,
                                    will_retry: false,
                                    attempt: 0,
                                });
                            failure.reason = "recovery_failed";
                            failure.will_retry = false;
                            let message = failure.message();
                            monitor.interruption = Some(failure);
                            monitor.interruption_reported = false;
                            monitor.recovery_notice = None;
                            if let Ok(mut warnings) = warnings_for_thread.lock() {
                                warnings.push(message);
                            }
                        }
                        minutes_core::logging::log_step(
                            "native_call_recovery_failed",
                            "native-call",
                            0,
                            serde_json::json!({}),
                        );
                    }
                    Some("health") => {
                        if let Ok(mut monitor) = capture_progress_for_thread.lock() {
                            monitor.observe(Instant::now(), &value);
                        }
                        if let Ok(mut current) = health_for_thread.lock() {
                            current.mic_live = value
                                .get("mic_live")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            current.call_audio_live = value
                                .get("call_audio_live")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            current.mic_level = value
                                .get("mic_level")
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32)
                                .unwrap_or(0);
                            current.call_audio_level = value
                                .get("call_audio_level")
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32)
                                .unwrap_or(0);
                            current.mic_frames_silenced = value
                                .get("mic_frames_silenced")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            current.system_frames_silenced = value
                                .get("system_frames_silenced")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            current.last_update = chrono::Local::now().to_rfc3339();
                        }
                    }
                    Some("stems") => {
                        let voice = value
                            .get("voice_stem")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let system = value
                            .get("system_stem")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !voice.is_empty() && !system.is_empty() {
                            if let Ok(mut guard) = stems_for_thread.lock() {
                                *guard = Some(StemPaths {
                                    voice: PathBuf::from(voice),
                                    system: PathBuf::from(system),
                                });
                            }
                        }
                    }
                    Some("microphone_selected") => {
                        if let Some(name) = value.get("name").and_then(|value| value.as_str()) {
                            let _ = microphone_status_tx
                                .send(MicrophoneStartupStatus::Selected(name.to_string()));
                        }
                    }
                    Some(
                        "microphone_reconnecting"
                        | "microphone_recovery_failed"
                        | "microphone_resumed",
                    ) => {
                        let message = match value.get("event").and_then(|v| v.as_str()) {
                            Some("microphone_resumed") => {
                                let gap = value.get("gap_ms").and_then(|v| v.as_u64()).unwrap_or(0).min(3_600_000);
                                format!("Microphone capture reconnected. Approximately {:.1} seconds may contain missing speech. Other captured audio was preserved.", gap as f64 / 1000.0)
                            }
                            Some("microphone_recovery_failed") => "Microphone capture is unavailable. Minutes is preserving system audio and will retry the microphone. In-room speech may be missing.".into(),
                            _ => "Microphone capture was interrupted. Minutes is reconnecting an available input and preserving the captured segments. Some speech may be missing.".into(),
                        };
                        if let Ok(mut warnings) = warnings_for_thread.lock() {
                            if !warnings.contains(&message) {
                                warnings.push(message.clone());
                            }
                        }
                        if let Ok(mut monitor) = capture_progress_for_thread.lock() {
                            monitor.source_notice = Some(message);
                        }
                        minutes_core::logging::log_step(
                            "native_microphone_recovery",
                            "native-call",
                            0,
                            value.clone(),
                        );
                    }
                    Some("microphone_disconnected") => {
                        let name = value
                            .get("name")
                            .and_then(|value| value.as_str())
                            .unwrap_or("configured microphone");
                        let message = format!(
                            "Microphone '{}' disconnected during capture. Minutes is reconnecting an available input; speech during the gap may be missing.",
                            name
                        );
                        tracing::warn!(
                            microphone = name,
                            "configured microphone disconnected during native call capture"
                        );
                        if let Ok(mut warnings) = warnings_for_thread.lock() {
                            warnings.push(message);
                        }
                    }
                    Some("microphone_fallback") => {
                        let name = value
                            .get("name")
                            .and_then(|value| value.as_str())
                            .unwrap_or("configured microphone");
                        let _ = microphone_status_tx
                            .send(MicrophoneStartupStatus::Fallback(name.to_string()));
                    }
                    Some("audio_format") => {
                        tracing::info!(
                            audio_format = %value,
                            "native call source audio format negotiated"
                        );
                    }
                    Some("audio_format_unsupported") => {
                        let message = capture_warning_for_audio_format_unsupported(&value);
                        tracing::warn!(
                            audio_format = %value,
                            "native call source PCM layout is unsupported; stem buffer silenced"
                        );
                        if let Ok(mut warnings) = warnings_for_thread.lock() {
                            if !warnings.contains(&message) {
                                warnings.push(message);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    });

    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(line)) if line == "ready" => {
            // A configured-device helper emits exactly one selected/fallback
            // status immediately after `ready`. Wait for that acknowledged
            // second phase before returning so the capture-start notice cannot
            // race the stdout reader (#463).
            finish_microphone_startup_handshake(
                microphone_status_expected,
                &microphone_status_rx,
                &capture_warnings,
                MICROPHONE_STARTUP_STATUS_TIMEOUT,
            );
            let stem_monitor_stop =
                spawn_stem_progress_monitor(&output_path, Arc::clone(&capture_progress));
            Ok(NativeCallCaptureSession {
                child,
                output_path,
                health,
                stem_paths,
                last_progress,
                capture_warnings,
                capture_progress,
                stem_monitor_stop,
            })
        }
        Ok(Ok(line)) => {
            let _ = child.kill();
            Err(format!(
                "native call helper returned unexpected readiness output: {}",
                line
            ))
        }
        Ok(Err(error)) => {
            let _ = child.kill();
            Err(error)
        }
        Err(_) => {
            let _ = child.kill();
            Err("native call helper timed out waiting for ScreenCaptureKit readiness".into())
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn start_native_call_capture(
    _configured_microphone: Option<&str>,
    _enable_recovery_test_signal: bool,
) -> Result<NativeCallCaptureSession, String> {
    Err("native call capture is unsupported on this platform".into())
}

#[cfg(target_os = "macos")]
fn list_native_call_microphones(helper: &Path) -> Result<MicrophoneInventory, String> {
    let output = Command::new(helper)
        .arg("--list-microphones")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("failed to enumerate native-call microphones: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "native call helper microphone enumeration exited with {}",
            output.status
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid native-call microphone inventory: {error}"))
}

#[cfg(target_os = "macos")]
fn native_call_output_path() -> Result<PathBuf, String> {
    let dir = minutes_core::Config::minutes_dir().join("native-captures");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join(format!(
        "{}-call.mov",
        chrono::Local::now().format("%Y-%m-%d-%H%M%S")
    )))
}

#[cfg(target_os = "macos")]
fn find_native_call_helper_binary() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        let beside_exe = exe
            .parent()
            .unwrap_or(exe.as_ref())
            .join("system_audio_record");
        if beside_exe.exists() {
            return Some(beside_exe);
        }
    }

    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bin/system_audio_record");
    if dev_path.exists() {
        return Some(dev_path);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{
        native_call_exit_is_unexpected, remote_silence_sample, CallSourceHealth,
        CaptureInterruption, CaptureProgressAction, CaptureProgressMonitor,
    };

    #[test]
    fn system_stop_is_reported_immediately_before_delayed_output_finalize() {
        let start = Instant::now();
        let mut monitor = CaptureProgressMonitor::new(start);
        monitor.interruption = CaptureInterruption::from_helper_event(&serde_json::json!({
            "event": "capture_interrupted",
            "reason": "stream_error",
            "error_domain": "com.apple.ScreenCaptureKit.SCStreamErrorDomain",
            "error_code": -3821,
            "observed_at": "2026-01-01T12:00:00Z",
        }));
        assert!(
            matches!(monitor.check(start), CaptureProgressAction::Interrupted(ref message)
            if message.contains("-3821") && message.contains("saving the captured segment"))
        );
        // Buffered health can arrive after the stream stopped. It must not
        // clear the latched failure, and the notice must only fire once.
        monitor.observe(start + Duration::from_secs(1), &written_frames(480, 480));
        assert!(monitor.interruption.is_some());
        assert_eq!(
            monitor.check(start + Duration::from_secs(60)),
            CaptureProgressAction::None
        );
        monitor.observe_files(start + Duration::from_secs(121), (0, 0));
        assert!(matches!(
            monitor.check(start + Duration::from_secs(121)),
            CaptureProgressAction::Stop(_)
        ));
    }

    #[test]
    fn interruption_diagnostics_never_copy_arbitrary_helper_text() {
        let event = CaptureInterruption::from_helper_event(&serde_json::json!({
            "event": "capture_interrupted",
            "reason": "stream_error",
            "error_domain": "private window title",
            "error_code": -3821,
            "observed_at": "private window title",
            "description": "private window title",
        }))
        .expect("known interruption reason");
        let diagnostic = serde_json::to_string(&event).unwrap();
        assert!(!diagnostic.contains("private window"));
        assert!(diagnostic.contains("-3821"));
        assert!(CaptureInterruption::from_helper_event(&serde_json::json!({
            "event": "health", "reason": "stream_error"
        }))
        .is_none());
    }

    #[test]
    fn successful_helper_exit_is_not_evidence_of_a_requested_stop() {
        assert!(native_call_exit_is_unexpected(true, false));
        assert!(native_call_exit_is_unexpected(false, false));
        assert!(native_call_exit_is_unexpected(false, true));
        assert!(!native_call_exit_is_unexpected(true, true));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_helper_preserves_interruptions_during_output_finalization() {
        let helper = find_native_call_helper_binary().expect("native call helper should be built");
        let output = std::process::Command::new(helper)
            .arg("--self-test-capture-lifecycle")
            .output()
            .expect("capture lifecycle self-test should launch without capture");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt["event"], "capture_lifecycle_self_test");
        assert_eq!(receipt["status"], "ok");
        assert_eq!(receipt["cases"], 4);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_helper_recovers_without_truncating_audio_or_compressing_gaps() {
        let helper = find_native_call_helper_binary().expect("native call helper should be built");
        let output = std::process::Command::new(helper)
            .arg("--self-test-capture-recovery")
            .output()
            .expect("capture-free recovery acceptance test should launch");
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt["event"], "capture_recovery_self_test");
        assert_eq!(receipt["status"], "ok");
        assert!(receipt["checks"].as_u64().unwrap() >= 45);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_microphone_recovery_ignores_silence_and_selects_an_available_fallback() {
        let output = std::process::Command::new(find_native_call_helper_binary().unwrap())
            .arg("--self-test-microphone-recovery")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt["status"], "ok");
        assert!(receipt["checks"].as_u64().unwrap() >= 130);
    }

    #[test]
    fn progressing_system_audio_cannot_hide_a_lost_microphone() {
        let start = Instant::now();
        let mut monitor = CaptureProgressMonitor::new(start);
        monitor.observe(start, &written_frames(48_000, 48_000));
        for second in 1..=150 {
            let now = start + Duration::from_secs(second);
            monitor.observe(now, &written_frames(48_000, (second + 1) * 48_000));
            monitor.observe_files(now, (192_000, (second + 1) * 192_000));
            let action = monitor.check(now);
            if second == 30 {
                assert!(
                    matches!(action, CaptureProgressAction::Warning(ref message) if message.starts_with("Microphone"))
                );
            } else {
                assert_eq!(action, CaptureProgressAction::None);
            }
        }
        let resumed = start + Duration::from_secs(151);
        monitor.observe(resumed, &written_frames(96_000, 7_296_000));
        assert_eq!(monitor.check(resumed), CaptureProgressAction::None);
        let lost_again = resumed + Duration::from_secs(30);
        monitor.observe(lost_again, &written_frames(96_000, 8_736_000));
        assert!(
            matches!(monitor.check(lost_again), CaptureProgressAction::Warning(ref message) if message.starts_with("Microphone"))
        );
    }

    #[test]
    fn microphone_loss_uses_the_existing_gap_and_stop_recovery_contract() {
        let start = Instant::now();
        let mut monitor = CaptureProgressMonitor::new(start);
        monitor.interruption = CaptureInterruption::from_helper_event(&serde_json::json!({
            "event": "capture_interrupted", "reason": "microphone_lost",
            "will_retry": true, "attempt": 0,
        }));
        assert!(
            matches!(monitor.check(start), CaptureProgressAction::Interrupted(ref message)
            if message.starts_with("Microphone") && message.contains("keeping this meeting open"))
        );
        // An acknowledgement for the failed generation cannot clear the loss.
        assert!(monitor.resume(0, start, 0).is_none());
        let resumed = monitor
            .resume(1, start + Duration::from_secs(4), 4000)
            .unwrap();
        assert!(resumed.contains("4.0 seconds") && resumed.contains("missing speech"));
        assert!(monitor.interruption.is_none());
    }

    #[test]
    fn lost_system_source_warns_without_stopping_the_surviving_microphone() {
        let start = Instant::now();
        let mut monitor = CaptureProgressMonitor::new(start);
        monitor.observe(start, &written_frames(48_000, 48_000));
        let now = start + Duration::from_secs(150);
        monitor.observe(now, &written_frames(7_248_000, 48_000));
        monitor.observe_files(now, (28_992_000, 192_000));
        assert!(
            matches!(monitor.check(now), CaptureProgressAction::Warning(ref message) if message.starts_with("System audio"))
        );
        assert_eq!(monitor.check(now), CaptureProgressAction::None);
    }

    #[test]
    fn recovered_stream_keeps_the_same_monitor_and_rejects_late_resume() {
        let start = Instant::now();
        let mut monitor = CaptureProgressMonitor::new(start);
        monitor.interruption = CaptureInterruption::from_helper_event(&serde_json::json!({
            "event": "capture_interrupted", "reason": "stream_error", "error_code": -3821,
            "will_retry": true, "attempt": 0,
        }));
        assert!(
            matches!(monitor.check(start), CaptureProgressAction::Interrupted(ref message)
            if message.contains("keeping this meeting open"))
        );
        assert!(monitor.resume(0, start, 100).is_none());
        assert!(monitor
            .resume(1, start + Duration::from_secs(2), 2000)
            .is_some());
        assert!(
            matches!(monitor.check(start + Duration::from_secs(2)), CaptureProgressAction::Recovered(ref message)
            if message.contains("2.0 seconds"))
        );
        monitor.interruption = CaptureInterruption::from_helper_event(&serde_json::json!({
            "event": "capture_interrupted", "reason": "stream_error", "will_retry": true, "attempt": 1,
        }));
        assert!(monitor
            .resume(1, start + Duration::from_secs(3), 200)
            .is_none());
        assert!(monitor.interruption.is_some());
        assert!(monitor
            .resume(2, start + Duration::from_secs(4), 1000)
            .is_some());
        assert!(monitor.interruption.is_none());
    }

    #[test]
    fn terminal_recovery_failure_cannot_be_cleared_by_a_buffered_resume() {
        let start = Instant::now();
        let mut monitor = CaptureProgressMonitor::new(start);
        monitor.interruption = Some(CaptureInterruption {
            reason: "recovery_failed",
            error_domain: None,
            error_code: Some(-3821),
            observed_at: None,
            will_retry: false,
            attempt: 1,
        });
        assert!(monitor.resume(2, start, 1000).is_none());
        assert!(
            matches!(monitor.check(start), CaptureProgressAction::Interrupted(ref message)
            if message.contains("recovery did not complete"))
        );
    }

    fn written_frames(mic: u64, system: u64) -> serde_json::Value {
        serde_json::json!({"mic_frames_written": mic, "system_frames_written": system})
    }

    #[test]
    fn healthy_heartbeat_cannot_hide_stalled_audio_writes() {
        let start = Instant::now();
        let mut monitor = CaptureProgressMonitor::new(start);
        monitor.observe(start, &written_frames(480, 480));
        monitor.observe_files(start, (1920, 1920));
        for second in 1..=120 {
            let now = start + Duration::from_secs(second);
            monitor.observe(now, &written_frames(480, 480));
            monitor.observe_files(now, (1920, 1920));
            let action = monitor.check(now);
            if second == 30 {
                assert!(
                    matches!(action, CaptureProgressAction::Warning(ref s) if s.contains("has not reported new"))
                );
            } else if second == 120 {
                assert!(
                    matches!(action, CaptureProgressAction::Stop(ref s) if s.contains("saving the captured audio"))
                );
            } else {
                assert_eq!(action, CaptureProgressAction::None);
            }
        }
    }

    #[test]
    fn silent_buffers_and_a_single_active_source_keep_capture_running_with_missing_mic_warning() {
        let start = Instant::now();
        for system_only in [false, true] {
            let mut monitor = CaptureProgressMonitor::new(start);
            for second in 0..=300 {
                let now = start + Duration::from_secs(second);
                // Counter progress is independent of sample amplitude or liveness.
                let count = (second + 1) * 48_000;
                monitor.observe(
                    now,
                    &if system_only {
                        written_frames(0, count)
                    } else {
                        written_frames(count, 0)
                    },
                );
                let action = monitor.check(now);
                if system_only && second == 30 {
                    assert!(
                        matches!(action, CaptureProgressAction::Warning(ref message) if message.starts_with("Microphone"))
                    );
                } else {
                    assert_eq!(action, CaptureProgressAction::None);
                }
            }
        }
    }

    #[test]
    fn recovering_writes_reset_warning_and_stop_deadline() {
        let start = Instant::now();
        let mut monitor = CaptureProgressMonitor::new(start);
        monitor.observe(start, &written_frames(1, 0));
        monitor.observe_files(start, (4, 0));
        assert!(matches!(
            monitor.check(start + Duration::from_secs(30)),
            CaptureProgressAction::Warning(_)
        ));
        let resumed = start + Duration::from_secs(119);
        monitor.observe(resumed, &written_frames(2, 0));
        monitor.observe_files(resumed, (8, 0));
        assert_eq!(monitor.check(resumed), CaptureProgressAction::None);
        monitor.observe(resumed + Duration::from_secs(30), &written_frames(2, 0));
        assert!(matches!(
            monitor.check(resumed + Duration::from_secs(30)),
            CaptureProgressAction::Warning(_)
        ));
        monitor.observe(resumed + Duration::from_secs(120), &written_frames(2, 0));
        monitor.observe_files(resumed + Duration::from_secs(120), (8, 0));
        assert!(matches!(
            monitor.check(resumed + Duration::from_secs(120)),
            CaptureProgressAction::Stop(_)
        ));
    }

    #[test]
    fn missing_telemetry_is_named_separately_from_audio_stall() {
        let start = Instant::now();
        let mut monitor = CaptureProgressMonitor::new(start);
        assert!(
            matches!(monitor.check(start + Duration::from_secs(30)), CaptureProgressAction::Warning(ref s) if s.contains("stopped reporting"))
        );
        monitor.observe_files(start + Duration::from_secs(120), (0, 0));
        assert!(matches!(
            monitor.check(start + Duration::from_secs(120)),
            CaptureProgressAction::Stop(_)
        ));
        let mut legacy = CaptureProgressMonitor::new(start);
        legacy.observe(
            start + Duration::from_secs(120),
            &serde_json::json!({"mic_level": 0}),
        );
        assert!(
            matches!(legacy.check(start + Duration::from_secs(120)), CaptureProgressAction::Warning(ref s) if s.contains("not providing"))
        );
    }

    #[cfg(target_os = "macos")]
    use super::find_native_call_helper_binary;
    use super::{
        capture_warning_for_audio_format_unsupported, finish_microphone_startup_handshake,
        native_call_helper_args, parse_macos_major_version, resolve_microphone_selection,
        HelperStderrRateLimiter, MicrophoneSelection, MicrophoneStartupStatus,
        HELPER_STDERR_INITIAL_LOG_LIMIT, HELPER_STDERR_LOG_INTERVAL,
    };
    use std::{
        ffi::OsString,
        path::Path,
        sync::{mpsc, Arc, Mutex},
        time::{Duration, Instant},
    };

    #[test]
    fn telemetry_failure_cannot_stop_growing_stems_and_stat_failure_stays_unknown() {
        let start = Instant::now();
        let mut monitor = CaptureProgressMonitor::new(start);
        for second in 0..=300 {
            let now = start + Duration::from_secs(second);
            monitor.observe_files(now, (second * 192_000, 0));
            assert!(!matches!(
                monitor.check(now),
                CaptureProgressAction::Stop(_)
            ));
        }
        let mut unknown_files = CaptureProgressMonitor::new(start);
        unknown_files.observe(start, &written_frames(0, 0));
        unknown_files.observe(start + Duration::from_secs(300), &written_frames(0, 0));
        assert!(!matches!(
            unknown_files.check(start + Duration::from_secs(300)),
            CaptureProgressAction::Stop(_)
        ));
    }

    fn health(mic_live: bool, call_live: bool, mic: u32, call: u32) -> CallSourceHealth {
        CallSourceHealth {
            backend: "screencapturekit-helper".into(),
            mic_live,
            call_audio_live: call_live,
            mic_level: mic,
            call_audio_level: call,
            mic_frames_silenced: 0,
            system_frames_silenced: 0,
            last_update: chrono::Local::now().to_rfc3339(),
            remote_audio_silent: false,
        }
    }

    #[test]
    fn remote_silence_sample_maps_live_sources() {
        use minutes_core::remote_silence::LevelSample;
        assert_eq!(
            remote_silence_sample(&health(true, true, 40, 30)),
            Some(LevelSample {
                mic_level: 40,
                system_level: 30
            })
        );
        // System attached but not delivering buffers: still silent remote.
        assert_eq!(
            remote_silence_sample(&health(true, false, 40, 30)),
            Some(LevelSample {
                mic_level: 40,
                system_level: 0
            })
        );
        // Mic not delivering (or telemetry stale): no evidence either way.
        assert_eq!(remote_silence_sample(&health(false, true, 0, 30)), None);
    }

    #[test]
    fn remote_audio_silent_flag_reaches_the_status_payload() {
        let mut snapshot = health(true, true, 40, 0);
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap()["remote_audio_silent"],
            false
        );
        snapshot.remote_audio_silent = true;
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap()["remote_audio_silent"],
            true
        );
    }

    #[test]
    fn helper_stderr_rate_limiter_logs_initial_lines_then_periodically() {
        let start = Instant::now();
        let mut limiter = HelperStderrRateLimiter::new(start);

        for _ in 0..HELPER_STDERR_INITIAL_LOG_LIMIT {
            assert_eq!(limiter.observe(start), Some(0));
        }
        assert_eq!(limiter.observe(start), None);
        assert_eq!(limiter.observe(start + Duration::from_secs(9)), None);
        assert_eq!(limiter.observe(start + HELPER_STDERR_LOG_INTERVAL), Some(2));
        assert_eq!(limiter.observe(start + HELPER_STDERR_LOG_INTERVAL), None);
    }

    #[test]
    fn unsupported_audio_warning_names_source_and_asbd() {
        let warning = capture_warning_for_audio_format_unsupported(&serde_json::json!({
            "source": "microphone",
            "sample_rate": 16_000,
            "format_id": "lpcm",
            "format_flags": 4,
            "bits_per_channel": 16,
            "bytes_per_packet": 4,
            "bytes_per_frame": 4,
            "frames_per_packet": 1,
            "channels": 1,
        }));
        assert!(warning.contains("microphone"));
        assert!(warning.contains("ASBD"));
        assert!(warning.contains("sample_rate=16000"));
        assert!(warning.contains("bits_per_channel=16"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_helper_decodes_hfp_pcm_container_layouts() {
        let helper = find_native_call_helper_binary().expect("native call helper should be built");
        let output = std::process::Command::new(helper)
            .arg("--self-test-pcm-decoder")
            .output()
            .expect("PCM decoder self-test should launch");

        assert!(
            output.status.success(),
            "PCM decoder self-test failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let receipt: serde_json::Value = serde_json::from_slice(&output.stdout)
            .expect("PCM decoder self-test should emit one JSON receipt");
        assert_eq!(receipt["event"], "pcm_decoder_self_test");
        assert_eq!(receipt["status"], "ok");
        assert_eq!(receipt["cases"], 7);
        assert_eq!(receipt["frames_silenced"], 80);
    }

    #[test]
    fn parses_major_version_from_product_version() {
        assert_eq!(parse_macos_major_version("15.0.1"), Some(15));
        assert_eq!(parse_macos_major_version("14.7"), Some(14));
        assert_eq!(parse_macos_major_version(""), None);
        assert_eq!(parse_macos_major_version("not-a-version"), None);
    }

    #[test]
    fn selects_exact_configured_microphone_when_present() {
        let available = vec![
            "MacBook Pro Microphone".to_string(),
            "Studio Display Microphone".to_string(),
        ];
        assert_eq!(
            resolve_microphone_selection(Some("Studio Display Microphone"), &available),
            MicrophoneSelection::Configured("Studio Display Microphone".into())
        );
    }

    #[test]
    fn reports_configured_microphone_missing_before_defaulting() {
        let available = vec!["MacBook Pro Microphone".to_string()];
        assert_eq!(
            resolve_microphone_selection(Some("Studio Display Microphone"), &available),
            MicrophoneSelection::ConfiguredMissing("Studio Display Microphone".into())
        );
    }

    #[test]
    fn uses_default_when_no_microphone_is_configured() {
        let available = vec!["MacBook Pro Microphone".to_string()];
        assert_eq!(
            resolve_microphone_selection(None, &available),
            MicrophoneSelection::Default
        );
    }

    #[test]
    fn configured_microphone_is_passed_to_native_helper() {
        let args = native_call_helper_args(
            Path::new("/tmp/call.mov"),
            &MicrophoneSelection::Configured("Studio Display Microphone".into()),
        );
        assert_eq!(
            args,
            vec![
                OsString::from("/tmp/call.mov"),
                OsString::from("--microphone-name"),
                OsString::from("Studio Display Microphone"),
            ]
        );

        let default_args =
            native_call_helper_args(Path::new("/tmp/call.mov"), &MicrophoneSelection::Default);
        assert_eq!(default_args, vec![OsString::from("/tmp/call.mov")]);
    }

    #[test]
    fn actual_start_fallback_is_visible_before_session_return() {
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            tx.send(MicrophoneStartupStatus::Fallback(
                "Studio Display Microphone".into(),
            ))
            .unwrap();
        });

        finish_microphone_startup_handshake(true, &rx, &warnings, Duration::from_secs(1));

        assert_eq!(
            warnings.lock().unwrap().as_slice(),
            ["configured mic not found, using default: 'Studio Display Microphone'."]
        );
    }
}
