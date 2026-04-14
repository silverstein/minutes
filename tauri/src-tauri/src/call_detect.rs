//! Auto-detect video/voice calls and prompt the user to start recording.
//!
//! Detection strategy: poll for known call-app processes that are actively
//! using the microphone. Two signals together (process running + mic active)
//! give high confidence with minimal false positives.
//!
//! Currently macOS-only. The detection functions (`running_process_names`,
//! `is_mic_in_use`) use CoreAudio and `ps`. Windows/Linux would need
//! alternative implementations behind `cfg(target_os)` gates.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use minutes_core::config::{CallDetectionConfig, Config};
use tauri::Emitter;

fn log_call_detect_event(
    level: &str,
    action: &str,
    app_name: Option<&str>,
    process_name: Option<&str>,
    extra: serde_json::Value,
) {
    minutes_core::logging::append_log(&serde_json::json!({
        "ts": chrono::Local::now().to_rfc3339(),
        "level": level,
        "step": "call_detect",
        "file": "",
        "extra": {
            "action": action,
            "app_name": app_name,
            "process_name": process_name,
            "details": extra,
        }
    }))
    .ok();
}

/// State for the call detection background loop.
pub struct CallDetector {
    config: Mutex<CallDetectionConfig>,
    /// Last observed active call session. We still re-arm on call end/start,
    /// but we also re-notify the same active app after a short interval so
    /// back-to-back meetings and sticky Zoom states don't go silent forever.
    active_call: Mutex<Option<ActiveCallState>>,
    /// Browser tab probing is slower and lower-confidence than native app
    /// detection, so keep it on its own cadence instead of every call-detect
    /// poll when the mic is hot.
    browser_probe_next_allowed_at: Mutex<Option<Instant>>,
    /// Back off individual browsers after Apple Events / automation failures so
    /// one denied path does not get retried every poll or suppress other browsers.
    browser_probe_backoff_until: Mutex<HashMap<String, Instant>>,
    /// Recent successful browser-based Meet detection. Prevents fast native-app
    /// polling from immediately relabeling the same active session as Slack.
    recent_google_meet_until: Mutex<Option<Instant>>,
    /// Log mic-gate transitions once instead of spamming every poll.
    last_mic_live: Mutex<Option<bool>>,
}

/// Payload emitted to the frontend when a call is detected.
#[derive(Clone, serde::Serialize)]
pub struct CallDetectedPayload {
    pub app_name: String,
    pub process_name: String,
    /// `true` for follow-up reminders about the same ongoing call.
    /// The frontend should NOT steal focus on reminders.
    pub is_reminder: bool,
}

#[derive(Clone)]
struct ActiveCallState {
    process_name: String,
    last_notified_at: Instant,
}

enum DetectionTransition {
    NewSession,
    Reminder,
    Noop,
}

enum DetectActiveCallResult {
    Detected {
        display_name: String,
        process_name: String,
    },
    PermissionWarning {
        browser_app: String,
    },
    None,
}

enum BrowserMeetProbe {
    Detected,
    PermissionDenied { browser_app: String },
    Error,
    NoBrowserProcesses,
    NoMatch,
}

const SAME_APP_REMINDER_SECS: u64 = 20;
const BROWSER_PROBE_INTERVAL_SECS: u64 = 15;
const BROWSER_PROBE_BACKOFF_SECS: u64 = 300;
const GOOGLE_MEET_STICKY_SECS: u64 = 20;

impl CallDetector {
    pub fn new(config: CallDetectionConfig) -> Self {
        Self {
            config: Mutex::new(config),
            active_call: Mutex::new(None),
            browser_probe_next_allowed_at: Mutex::new(None),
            browser_probe_backoff_until: Mutex::new(HashMap::new()),
            recent_google_meet_until: Mutex::new(None),
            last_mic_live: Mutex::new(None),
        }
    }

    fn current_config(&self) -> CallDetectionConfig {
        self.config.lock().unwrap().clone()
    }

    fn reload_config(&self) -> CallDetectionConfig {
        let latest = Config::load().call_detection;
        let mut guard = self.config.lock().unwrap();
        *guard = latest.clone();
        latest
    }

    /// Start the background detection loop. Runs in its own thread.
    pub fn start(
        self: Arc<Self>,
        app: tauri::AppHandle,
        recording: Arc<AtomicBool>,
        _processing: Arc<AtomicBool>,
    ) {
        let startup_config = self.current_config();
        let interval = Duration::from_secs(startup_config.poll_interval_secs.max(1));
        eprintln!(
            "[call-detect] started — polling every {}s for {:?}",
            interval.as_secs(),
            startup_config.apps
        );
        log_call_detect_event(
            "info",
            "started",
            None,
            None,
            serde_json::json!({
                "poll_interval_secs": interval.as_secs(),
                "apps": startup_config.apps,
            }),
        );

        std::thread::spawn(move || {
            // Initial delay to let the app finish launching
            std::thread::sleep(Duration::from_secs(5));

            loop {
                let config = self.reload_config();
                if !config.enabled {
                    if let Some(previous) = self.clear_active_call() {
                        log_call_detect_event(
                            "info",
                            "cleared",
                            None,
                            Some(&previous),
                            serde_json::json!({
                                "reason": "call detection disabled"
                            }),
                        );
                    }
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }

                let interval = Duration::from_secs(config.poll_interval_secs.max(1));
                std::thread::sleep(interval);

                // Skip only while the mic is already in use.
                if recording.load(Ordering::Relaxed) {
                    continue;
                }

                match self.detect_active_call(&config) {
                    DetectActiveCallResult::Detected {
                        display_name,
                        process_name,
                    } => {
                        match self.note_active_call(&process_name) {
                            DetectionTransition::Noop => {}
                            transition => {
                                let is_reminder =
                                    matches!(transition, DetectionTransition::Reminder);
                                let action = if is_reminder { "reminder" } else { "detected" };
                                eprintln!(
                                    "[call-detect] {}: {} ({})",
                                    action, display_name, process_name
                                );
                                log_call_detect_event(
                                    "info",
                                    action,
                                    Some(&display_name),
                                    Some(&process_name),
                                    serde_json::json!({
                                        "recording_active": recording.load(Ordering::Relaxed),
                                        "reminder_interval_secs": SAME_APP_REMINDER_SECS,
                                    }),
                                );

                                // Only show a macOS notification on first detection,
                                // not on periodic reminders — those are too noisy.
                                if !is_reminder {
                                    crate::commands::show_user_notification(
                                        &app,
                                        &format!("{} call detected", display_name),
                                        "Open Minutes to start recording",
                                    );
                                }

                                app.emit(
                                    "call:detected",
                                    CallDetectedPayload {
                                        app_name: display_name,
                                        process_name,
                                        is_reminder,
                                    },
                                )
                                .ok();
                            }
                        }
                    }
                    DetectActiveCallResult::PermissionWarning { browser_app } => {
                        if let Some(previous) = self.clear_active_call() {
                            log_call_detect_event(
                                "info",
                                "cleared",
                                None,
                                Some(&previous),
                                serde_json::json!({
                                    "reason": "browser automation permission required"
                                }),
                            );
                        }
                        crate::commands::show_user_notification(
                            &app,
                            "Google Meet detection needs browser access",
                            &format!(
                                "Allow Minutes to control {} in System Settings > Privacy & Security > Automation so Meet detection can see browser tabs.",
                                browser_app
                            ),
                        );
                    }
                    DetectActiveCallResult::None => {
                        if let Some(previous) = self.clear_active_call() {
                            log_call_detect_event(
                                "info",
                                "cleared",
                                None,
                                Some(&previous),
                                serde_json::json!({
                                    "reason": "no active call detected on current poll"
                                }),
                            );
                        }
                    }
                }
            }
        });
    }

    fn note_mic_state(&self, mic_live: bool) {
        let mut last = self.last_mic_live.lock().unwrap();
        if last.is_some() && *last == Some(mic_live) {
            return;
        }
        *last = Some(mic_live);
        log_call_detect_event(
            "info",
            if mic_live {
                "mic_gate_active"
            } else {
                "mic_gate_inactive"
            },
            None,
            None,
            serde_json::json!({ "mic_live": mic_live }),
        );
    }

    /// Check if any configured call app is active.
    fn detect_active_call(&self, config: &CallDetectionConfig) -> DetectActiveCallResult {
        let mic_live = is_mic_in_use();
        self.note_mic_state(mic_live);

        let has_google_meet = config.apps.iter().any(|app| app == "google-meet");
        let native_apps: Vec<&String> = config
            .apps
            .iter()
            .filter(|app| app.as_str() != "google-meet")
            .collect();
        let running = running_process_names();

        if has_google_meet && mic_live && self.google_meet_detection_is_sticky() {
            return DetectActiveCallResult::Detected {
                display_name: "Google Meet".into(),
                process_name: "google-meet".into(),
            };
        }

        if !mic_live {
            return DetectActiveCallResult::None;
        }

        for config_app in native_apps {
            let config_lower = config_app.to_lowercase();
            // Match the actual app binary name, not background daemons.
            // e.g. "FaceTime" should match the "FaceTime" binary, NOT
            // "com.apple.FaceTime.FTConversationService" (a system daemon
            // that runs permanently and caused false positives).
            if running.iter().any(|p| {
                let p_lower = p.to_lowercase();
                // Exact match (most common) or the config name is a
                // prefix/suffix of the binary name (e.g. "zoom.us" matches
                // "zoom.us"), but NOT a mere substring of a longer daemon name.
                p_lower == config_lower
                    || p_lower.starts_with(&format!("{}.", config_lower))
                    || p_lower.starts_with(&format!("{} ", config_lower))
            }) {
                let display = display_name_for(config_app);
                return DetectActiveCallResult::Detected {
                    display_name: display,
                    process_name: config_app.clone(),
                };
            }
        }

        if has_google_meet && self.browser_probe_due() {
            self.schedule_next_browser_probe();
            match self.detect_google_meet_in_browsers(&running) {
                BrowserMeetProbe::Detected => {
                    self.remember_google_meet_detection();
                    return DetectActiveCallResult::Detected {
                        display_name: "Google Meet".into(),
                        process_name: "google-meet".into(),
                    };
                }
                BrowserMeetProbe::PermissionDenied { browser_app } => {
                    return DetectActiveCallResult::PermissionWarning { browser_app };
                }
                BrowserMeetProbe::Error
                | BrowserMeetProbe::NoBrowserProcesses
                | BrowserMeetProbe::NoMatch => {}
            }
        }

        DetectActiveCallResult::None
    }

    fn note_active_call(&self, process_name: &str) -> DetectionTransition {
        let mut active = self.active_call.lock().unwrap();
        let now = Instant::now();
        match active.as_mut() {
            None => {
                *active = Some(ActiveCallState {
                    process_name: process_name.to_string(),
                    last_notified_at: now,
                });
                DetectionTransition::NewSession
            }
            Some(state) if state.process_name != process_name => {
                *state = ActiveCallState {
                    process_name: process_name.to_string(),
                    last_notified_at: now,
                };
                DetectionTransition::NewSession
            }
            Some(state) => {
                if now.duration_since(state.last_notified_at)
                    >= Duration::from_secs(SAME_APP_REMINDER_SECS)
                {
                    state.last_notified_at = now;
                    DetectionTransition::Reminder
                } else {
                    DetectionTransition::Noop
                }
            }
        }
    }

    fn clear_active_call(&self) -> Option<String> {
        let mut active = self.active_call.lock().unwrap();
        active.take().map(|state| state.process_name)
    }

    fn browser_probe_due(&self) -> bool {
        let mut next_probe = self.browser_probe_next_allowed_at.lock().unwrap();
        match *next_probe {
            Some(until) if Instant::now() < until => false,
            Some(_) => {
                *next_probe = None;
                true
            }
            None => true,
        }
    }

    fn schedule_next_browser_probe(&self) {
        let mut next_probe = self.browser_probe_next_allowed_at.lock().unwrap();
        *next_probe = Some(Instant::now() + Duration::from_secs(BROWSER_PROBE_INTERVAL_SECS));
    }

    fn remember_google_meet_detection(&self) {
        let mut sticky = self.recent_google_meet_until.lock().unwrap();
        *sticky = Some(Instant::now() + Duration::from_secs(GOOGLE_MEET_STICKY_SECS));
    }

    fn google_meet_detection_is_sticky(&self) -> bool {
        let mut sticky = self.recent_google_meet_until.lock().unwrap();
        match *sticky {
            Some(until) if Instant::now() < until => true,
            Some(_) => {
                *sticky = None;
                false
            }
            None => false,
        }
    }

    fn browser_probe_allowed_for(&self, browser_app: &str) -> bool {
        let mut backoff = self.browser_probe_backoff_until.lock().unwrap();
        match backoff.get(browser_app).copied() {
            Some(until) if Instant::now() < until => false,
            Some(_) => {
                backoff.remove(browser_app);
                true
            }
            None => true,
        }
    }

    fn defer_browser_probe_for(&self, browser_app: &str, reason: &str) {
        let mut backoff = self.browser_probe_backoff_until.lock().unwrap();
        backoff.insert(
            browser_app.to_string(),
            Instant::now() + Duration::from_secs(BROWSER_PROBE_BACKOFF_SECS),
        );
        log_call_detect_event(
            "warn",
            "browser_probe_backoff",
            Some("Google Meet"),
            Some(browser_app),
            serde_json::json!({
                "reason": reason,
                "backoff_secs": BROWSER_PROBE_BACKOFF_SECS,
            }),
        );
    }

    fn detect_google_meet_in_browsers(&self, running: &[String]) -> BrowserMeetProbe {
        let running_lower: Vec<String> = running.iter().map(|s| s.to_lowercase()).collect();
        let mut saw_browser = false;

        for (proc_fragment, app_name, kind) in &[
            ("google chrome", "Google Chrome", BrowserKind::ChromeLike),
            (
                "chrome canary",
                "Google Chrome Canary",
                BrowserKind::ChromeLike,
            ),
            ("chromium", "Chromium", BrowserKind::ChromeLike),
            ("arc", "Arc", BrowserKind::ChromeLike),
            ("safari", "Safari", BrowserKind::Safari),
        ] {
            if !running_lower.iter().any(|p| p.contains(proc_fragment)) {
                continue;
            }
            saw_browser = true;
            if !self.browser_probe_allowed_for(app_name) {
                continue;
            }

            match query_browser_urls(app_name, *kind) {
                AppleScriptProbe::Urls(urls) => {
                    if urls
                        .iter()
                        .any(|url| looks_like_google_meet_meeting_url(url))
                    {
                        return BrowserMeetProbe::Detected;
                    }
                }
                AppleScriptProbe::PermissionDenied => {
                    self.defer_browser_probe_for(app_name, "apple_events_permission_denied");
                    return BrowserMeetProbe::PermissionDenied {
                        browser_app: (*app_name).to_string(),
                    };
                }
                AppleScriptProbe::Error => {
                    self.defer_browser_probe_for(app_name, "browser_probe_error");
                    return BrowserMeetProbe::Error;
                }
            }
        }

        if saw_browser {
            BrowserMeetProbe::NoMatch
        } else {
            BrowserMeetProbe::NoBrowserProcesses
        }
    }
}

/// Friendly display name for a process name or browser sentinel.
fn display_name_for(process: &str) -> String {
    match process {
        "zoom.us" => "Zoom".into(),
        "Microsoft Teams" | "Microsoft Teams (work or school)" => "Teams".into(),
        "FaceTime" => "FaceTime".into(),
        "Webex" => "Webex".into(),
        "Slack" => "Slack".into(),
        "google-meet" => "Google Meet".into(),
        other => other.into(),
    }
}

#[derive(Debug, Clone, Copy)]
enum BrowserKind {
    ChromeLike,
    Safari,
}

enum AppleScriptProbe {
    Urls(Vec<String>),
    PermissionDenied,
    Error,
}

fn query_browser_urls(app_name: &str, kind: BrowserKind) -> AppleScriptProbe {
    let script = match kind {
        BrowserKind::ChromeLike => format!(
            r#"tell application "{app_name}"
set output to ""
repeat with w in windows
  repeat with t in tabs of w
    set output to output & (URL of t as text) & linefeed
  end repeat
end repeat
return output
end tell"#
        ),
        BrowserKind::Safari => format!(
            r#"tell application "{app_name}"
set output to ""
repeat with w in windows
  repeat with t in tabs of w
    set output to output & (URL of t as text) & linefeed
  end repeat
end repeat
return output
end tell"#
        ),
    };
    run_applescript_urls(&script)
}

fn run_applescript_urls(script: &str) -> AppleScriptProbe {
    let output = match std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
    {
        Ok(output) => output,
        Err(_) => return AppleScriptProbe::Error,
    };

    if output.status.success() {
        let urls = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        return AppleScriptProbe::Urls(urls);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    if stderr.contains("not authorized")
        || stderr.contains("not permitted")
        || stderr.contains("(-1743)")
    {
        AppleScriptProbe::PermissionDenied
    } else {
        AppleScriptProbe::Error
    }
}

fn looks_like_google_meet_meeting_url(url: &str) -> bool {
    let lower = url.trim().to_lowercase();
    let without_scheme = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);

    let Some(rest) = without_scheme.strip_prefix("meet.google.com/") else {
        return false;
    };

    let first_segment = rest
        .split(['?', '#', '/'])
        .next()
        .unwrap_or_default()
        .trim();

    looks_like_google_meet_meeting_code(first_segment)
}

fn looks_like_google_meet_meeting_code(segment: &str) -> bool {
    let parts: Vec<&str> = segment.split('-').collect();
    if parts.len() != 3 {
        return false;
    }

    let expected_lengths = [3, 4, 3];
    parts
        .iter()
        .zip(expected_lengths)
        .all(|(part, expected_len)| {
            part.len() == expected_len && part.chars().all(|ch| ch.is_ascii_lowercase())
        })
}

// ── macOS-specific detection ──────────────────────────────────

/// Get list of running process names via `ps`. Fast (~2ms), no permissions
/// needed, no osascript overhead.
fn running_process_names() -> Vec<String> {
    let output = std::process::Command::new("ps")
        .args(["-eo", "comm="])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines()
                .filter_map(|line| {
                    // ps returns full paths like /Applications/zoom.us.app/Contents/MacOS/zoom.us
                    // Extract just the binary name
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        return None;
                    }
                    Some(trimmed.rsplit('/').next().unwrap_or(trimmed).to_string())
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Check if the default audio input device is currently being used.
///
/// Uses a pre-compiled Swift helper that calls CoreAudio
/// `kAudioDevicePropertyDeviceIsRunningSomewhere` on the default input device.
/// Works on both Intel and Apple Silicon Macs.
///
/// Falls back to an inline `swift` invocation if the helper binary is missing.
fn is_mic_in_use() -> bool {
    // Try the pre-compiled helper first (fast: ~5ms)
    let helper = find_mic_check_binary();
    if let Some(path) = &helper {
        if let Ok(out) = std::process::Command::new(path).output() {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                return text == "1";
            }
        }
    }

    // Fallback: inline swift (slower: ~200ms, but always works)
    let script = r#"
import CoreAudio
var id = AudioObjectID(kAudioObjectSystemObject)
var pa = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyDefaultInputDevice, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
var sz = UInt32(MemoryLayout<AudioObjectID>.size)
guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &pa, 0, nil, &sz, &id) == noErr else { print("0"); exit(0) }
var r: UInt32 = 0
var ra = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyDeviceIsRunningSomewhere, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
sz = UInt32(MemoryLayout<UInt32>.size)
guard AudioObjectGetPropertyData(id, &ra, 0, nil, &sz, &r) == noErr else { print("0"); exit(0) }
print(r > 0 ? "1" : "0")
"#;

    let output = std::process::Command::new("swift")
        .arg("-e")
        .arg(script)
        .output();

    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim() == "1",
        _ => false,
    }
}

/// Find the pre-compiled mic_check binary.
/// Checks next to the app binary first, then the source tree location.
fn find_mic_check_binary() -> Option<std::path::PathBuf> {
    // In the bundled app: same directory as the main binary
    if let Ok(exe) = std::env::current_exe() {
        let beside_exe = exe.parent().unwrap_or(exe.as_ref()).join("mic_check");
        if beside_exe.exists() {
            return Some(beside_exe);
        }
    }

    // In development: check the source tree
    let dev_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bin/mic_check");
    if dev_path.exists() {
        return Some(dev_path);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_session_rearms_when_process_changes_or_ends() {
        let detector = CallDetector::new(CallDetectionConfig {
            enabled: true,
            poll_interval_secs: 1,
            cooldown_minutes: 5,
            apps: vec!["zoom.us".into()],
        });

        assert!(matches!(
            detector.note_active_call("zoom.us"),
            DetectionTransition::NewSession
        ));
        assert!(matches!(
            detector.note_active_call("zoom.us"),
            DetectionTransition::Noop
        ));
        detector.clear_active_call();
        assert!(matches!(
            detector.note_active_call("zoom.us"),
            DetectionTransition::NewSession
        ));
        assert!(matches!(
            detector.note_active_call("face.time"),
            DetectionTransition::NewSession
        ));
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name_for("zoom.us"), "Zoom");
        assert_eq!(display_name_for("Microsoft Teams"), "Teams");
        assert_eq!(display_name_for("FaceTime"), "FaceTime");
        assert_eq!(display_name_for("google-meet"), "Google Meet");
        assert_eq!(display_name_for("SomeOtherApp"), "SomeOtherApp");
    }

    #[test]
    fn google_meet_detection_is_opt_in_via_sentinel() {
        let detector = CallDetector::new(CallDetectionConfig {
            enabled: true,
            poll_interval_secs: 1,
            cooldown_minutes: 5,
            apps: vec!["zoom.us".into(), "google-meet".into()],
        });

        assert!(detector
            .current_config()
            .apps
            .iter()
            .any(|app| app == "google-meet"));
    }

    #[test]
    fn browser_probe_is_skipped_when_no_browser_processes_exist() {
        let detector = CallDetector::new(CallDetectionConfig {
            enabled: true,
            poll_interval_secs: 1,
            cooldown_minutes: 5,
            apps: vec!["google-meet".into()],
        });
        let running: Vec<String> = vec!["Finder".into(), "launchd".into()];
        assert!(matches!(
            detector.detect_google_meet_in_browsers(&running),
            BrowserMeetProbe::NoBrowserProcesses
        ));
    }

    #[test]
    fn meet_url_requires_real_meeting_code() {
        assert!(looks_like_google_meet_meeting_url(
            "https://meet.google.com/abc-defg-hij"
        ));
        assert!(looks_like_google_meet_meeting_url(
            "https://meet.google.com/abc-defg-hij?authuser=1"
        ));
        assert!(!looks_like_google_meet_meeting_url(
            "https://meet.google.com/"
        ));
        assert!(!looks_like_google_meet_meeting_url(
            "https://meet.google.com/new"
        ));
        assert!(!looks_like_google_meet_meeting_url(
            "https://meet.google.com/landing"
        ));
        assert!(!looks_like_google_meet_meeting_url(
            "https://example.com/abc-defg-hij"
        ));
    }

    #[test]
    fn malformed_applescript_fails_gracefully() {
        assert!(matches!(
            run_applescript_urls("this is not valid applescript @@@@"),
            AppleScriptProbe::Error
        ));
    }

    #[test]
    fn browser_probe_backoff_resets_after_expiry() {
        let detector = CallDetector::new(CallDetectionConfig {
            enabled: true,
            poll_interval_secs: 1,
            cooldown_minutes: 5,
            apps: vec!["google-meet".into()],
        });

        detector.defer_browser_probe_for("Google Chrome", "test");
        assert!(!detector.browser_probe_allowed_for("Google Chrome"));
        assert!(detector.browser_probe_allowed_for("Safari"));

        {
            let mut backoff = detector.browser_probe_backoff_until.lock().unwrap();
            backoff.insert(
                "Google Chrome".into(),
                Instant::now() - Duration::from_secs(1),
            );
        }

        assert!(detector.browser_probe_allowed_for("Google Chrome"));
    }

    #[test]
    fn browser_probe_global_interval_resets_after_expiry() {
        let detector = CallDetector::new(CallDetectionConfig {
            enabled: true,
            poll_interval_secs: 1,
            cooldown_minutes: 5,
            apps: vec!["google-meet".into()],
        });

        detector.schedule_next_browser_probe();
        assert!(!detector.browser_probe_due());

        {
            let mut next_probe = detector.browser_probe_next_allowed_at.lock().unwrap();
            *next_probe = Some(Instant::now() - Duration::from_secs(1));
        }

        assert!(detector.browser_probe_due());
    }

    #[test]
    fn sticky_google_meet_detection_survives_between_browser_probes() {
        let detector = CallDetector::new(CallDetectionConfig {
            enabled: true,
            poll_interval_secs: 1,
            cooldown_minutes: 5,
            apps: vec!["Slack".into(), "google-meet".into()],
        });

        detector.remember_google_meet_detection();
        assert!(detector.google_meet_detection_is_sticky());

        {
            let mut sticky = detector.recent_google_meet_until.lock().unwrap();
            *sticky = Some(Instant::now() - Duration::from_secs(1));
        }

        assert!(!detector.google_meet_detection_is_sticky());
    }

    #[test]
    fn native_app_detection_wins_before_browser_meet_probe() {
        let detector = CallDetector::new(CallDetectionConfig {
            enabled: true,
            poll_interval_secs: 1,
            cooldown_minutes: 5,
            apps: vec!["zoom.us".into(), "google-meet".into()],
        });

        let running = ["zoom.us".into(), "Safari".into()];
        let mic_live = true;

        // Reproduce the decision ordering from detect_active_call without relying
        // on live browser automation in tests.
        let config = detector.current_config();
        let native_apps: Vec<&String> = config
            .apps
            .iter()
            .filter(|app| app.as_str() != "google-meet")
            .collect();

        let mut detected = None;
        if mic_live {
            for config_app in native_apps {
                let config_lower = config_app.to_lowercase();
                if running.iter().any(|p: &String| {
                    let p_lower = p.to_lowercase();
                    p_lower == config_lower
                        || p_lower.starts_with(&format!("{}.", config_lower))
                        || p_lower.starts_with(&format!("{} ", config_lower))
                }) {
                    detected = Some(config_app.clone());
                    break;
                }
            }
        }

        assert_eq!(detected.as_deref(), Some("zoom.us"));
    }

    #[test]
    fn sticky_google_meet_still_wins_when_no_native_app_is_active() {
        let detector = CallDetector::new(CallDetectionConfig {
            enabled: true,
            poll_interval_secs: 1,
            cooldown_minutes: 5,
            apps: vec!["zoom.us".into(), "google-meet".into()],
        });

        detector.remember_google_meet_detection();
        let running = ["Safari".into()];
        let config = detector.current_config();
        let native_apps: Vec<&String> = config
            .apps
            .iter()
            .filter(|app| app.as_str() != "google-meet")
            .collect();

        let native_detected = native_apps.iter().any(|config_app| {
            let config_lower = config_app.to_lowercase();
            running.iter().any(|p: &String| {
                let p_lower = p.to_lowercase();
                p_lower == config_lower
                    || p_lower.starts_with(&format!("{}.", config_lower))
                    || p_lower.starts_with(&format!("{} ", config_lower))
            })
        });

        assert!(!native_detected);
        assert!(detector.google_meet_detection_is_sticky());
    }

    #[test]
    fn process_list_returns_real_results() {
        let procs = running_process_names();
        // ps should always return at least a few processes
        assert!(!procs.is_empty(), "process list should not be empty");
    }

    #[test]
    fn mic_check_does_not_panic() {
        // Just verify the function returns without crashing.
        // Will return false unless something is using the mic right now.
        let _result = is_mic_in_use();
    }
}
