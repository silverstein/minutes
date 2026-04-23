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
    /// Recent successful browser-based Teams detection. Same role as the Meet
    /// sticky field but for Microsoft Teams in a browser tab.
    recent_teams_web_until: Mutex<Option<Instant>>,
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

/// Emitted when the call app that triggered the current recording is no
/// longer detected. The frontend shows a countdown banner with Stop now /
/// Keep recording, and the backend arms a cancellable auto-stop timer.
#[derive(Clone, serde::Serialize)]
pub struct CallEndedPayload {
    pub app_name: String,
    pub process_name: String,
    pub countdown_secs: u64,
}

/// Handles shared with the desktop app state so this module can observe and
/// arm the auto-stop countdown without having to reach into `commands::AppState`
/// directly.
#[derive(Clone)]
pub struct CallEndAutoStopHandles {
    pub recording_started_by_call_detect: Arc<AtomicBool>,
    pub countdown_cancel: Arc<AtomicBool>,
    pub countdown_active: Arc<AtomicBool>,
    pub stop_flag: Arc<AtomicBool>,
}

#[derive(Clone)]
struct ActiveCallState {
    process_name: String,
    display_name: String,
    last_notified_at: Instant,
    /// Set after `call:ended` has been emitted for the current session.
    /// Keeps repeated "no longer detected" polls from re-arming the
    /// countdown if the user hit "Keep recording".
    call_end_fired: bool,
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

/// Decision the detector's polling loop makes when `detect_active_call`
/// returns `None`. Extracted as an enum so the invariants can be exercised
/// by a unit test without spinning up the full poll thread.
#[derive(Debug, PartialEq, Eq)]
enum NoCallDecision {
    /// A countdown is already ticking — let its thread own the lifecycle.
    /// Skip all state mutation and logging for this poll.
    DeferToCountdown,
    /// Active call ended and nothing is armed yet — fire the countdown if
    /// an active session exists that hasn't already fired.
    ArmCountdown,
    /// We already emitted `call:ended`, but the active atomic is no longer
    /// set and no explicit cancellation was observed. Re-arm instead of
    /// clearing so an orphaned countdown cannot leave recording running.
    RearmCountdown,
    /// No recording is in scope for auto-stop — safe to clear any stale
    /// `active_call` state and emit the "cleared" log.
    ClearIfStale,
}

fn decide_no_call_action(
    is_recording: bool,
    started_by_call_detect: bool,
    stop_when_call_ends: bool,
    countdown_active: bool,
    call_end_fired: bool,
    countdown_cancelled: bool,
) -> NoCallDecision {
    if countdown_active {
        return NoCallDecision::DeferToCountdown;
    }
    if is_recording && started_by_call_detect && stop_when_call_ends {
        if call_end_fired {
            if countdown_cancelled {
                NoCallDecision::ClearIfStale
            } else {
                NoCallDecision::RearmCountdown
            }
        } else {
            NoCallDecision::ArmCountdown
        }
    } else {
        NoCallDecision::ClearIfStale
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeetingProvider {
    GoogleMeet,
    TeamsWeb,
}

impl MeetingProvider {
    /// Returns (display name, process sentinel) for the provider. The sentinel
    /// is the same opaque string stored in `CallDetectionConfig::apps`.
    fn names(self) -> (&'static str, &'static str) {
        match self {
            MeetingProvider::GoogleMeet => ("Google Meet", "google-meet"),
            MeetingProvider::TeamsWeb => ("Teams", "teams-web"),
        }
    }

    fn sticky_duration(self) -> Duration {
        match self {
            MeetingProvider::GoogleMeet => Duration::from_secs(GOOGLE_MEET_STICKY_SECS),
            MeetingProvider::TeamsWeb => Duration::from_secs(TEAMS_WEB_STICKY_SECS),
        }
    }
}

fn detected_for(provider: MeetingProvider) -> DetectActiveCallResult {
    let (display_name, process_name) = provider.names();
    DetectActiveCallResult::Detected {
        display_name: display_name.into(),
        process_name: process_name.into(),
    }
}

fn remember_sticky(sticky: &Mutex<Option<Instant>>, ttl: Duration) {
    *sticky.lock().unwrap() = Some(Instant::now() + ttl);
}

fn sticky_alive(sticky: &Mutex<Option<Instant>>) -> bool {
    let mut guard = sticky.lock().unwrap();
    match *guard {
        Some(until) if Instant::now() < until => true,
        Some(_) => {
            *guard = None;
            false
        }
        None => false,
    }
}

enum BrowserMeetProbe {
    Detected { provider: MeetingProvider },
    PermissionDenied { browser_app: String },
    Error,
    NoBrowserProcesses,
    NoMatch,
}

const SAME_APP_REMINDER_SECS: u64 = 20;
const BROWSER_PROBE_INTERVAL_SECS: u64 = 15;
const BROWSER_PROBE_BACKOFF_SECS: u64 = 300;
const GOOGLE_MEET_STICKY_SECS: u64 = 20;
const TEAMS_WEB_STICKY_SECS: u64 = 20;

impl CallDetector {
    pub fn new(config: CallDetectionConfig) -> Self {
        Self {
            config: Mutex::new(config),
            active_call: Mutex::new(None),
            browser_probe_next_allowed_at: Mutex::new(None),
            browser_probe_backoff_until: Mutex::new(HashMap::new()),
            recent_google_meet_until: Mutex::new(None),
            recent_teams_web_until: Mutex::new(None),
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
        auto_stop: CallEndAutoStopHandles,
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

                let is_recording = recording.load(Ordering::Relaxed);
                let started_by_call_detect = auto_stop
                    .recording_started_by_call_detect
                    .load(Ordering::Relaxed);

                // Default behavior preserved: when something else is recording
                // (manual `minutes record`, hotkey, live transcript), skip detection
                // entirely. Only observe calls when the detector's own banner
                // launched this recording AND the user opted into auto-stop.
                if is_recording && !(started_by_call_detect && config.stop_when_call_ends) {
                    continue;
                }

                match self.detect_active_call(&config) {
                    DetectActiveCallResult::Detected {
                        display_name,
                        process_name,
                    } => {
                        // Call came back (same app): if the previous call
                        // already fired a countdown that the user dismissed
                        // with "Keep recording", clear the latch so a later
                        // call-end can re-arm the auto-stop prompt. If a
                        // countdown is still ticking from a transient "no
                        // call" poll (e.g. Zoom hiccup, user coming back from
                        // mute), cancel it — the user is back on the call and
                        // doesn't want auto-stop to fire in the middle of it.
                        if is_recording && started_by_call_detect {
                            if auto_stop.countdown_active.load(Ordering::Relaxed) {
                                auto_stop.countdown_cancel.store(true, Ordering::Relaxed);
                                log_call_detect_event(
                                    "info",
                                    "call_end_countdown_cancelled_by_redetect",
                                    Some(&display_name),
                                    Some(&process_name),
                                    serde_json::json!({
                                        "reason": "same call re-detected while countdown was ticking",
                                    }),
                                );
                            }
                            self.reset_call_end_latch();
                        }
                        match self.note_active_call(&process_name, &display_name) {
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
                            "Browser meeting detection needs access",
                            &format!(
                                "Allow Minutes to control {} in System Settings > Privacy & Security > Automation so Meet and Teams detection can see browser tabs.",
                                browser_app
                            ),
                        );
                    }
                    DetectActiveCallResult::None => {
                        // A countdown already in flight owns the lifecycle.
                        // Without this guard any atomics flip mid-countdown
                        // (recording transiently going false, started_by
                        // cleared by a stray cmd_start_recording, etc.)
                        // orphans the countdown: the ClearIfStale arm below
                        // would wipe active_call and a later detector tick
                        // couldn't observe the ended call anymore even
                        // though the user's intent was still to auto-stop.
                        // This is the bug athal7 hit in issue #129.
                        let countdown_active = auto_stop.countdown_active.load(Ordering::Relaxed);
                        let active_snapshot = self.active_call_snapshot();
                        let call_end_fired = active_snapshot
                            .as_ref()
                            .map(|(_, _, already_fired)| *already_fired)
                            .unwrap_or(false);
                        let countdown_cancelled =
                            auto_stop.countdown_cancel.load(Ordering::Relaxed);
                        match decide_no_call_action(
                            is_recording,
                            started_by_call_detect,
                            config.stop_when_call_ends,
                            countdown_active,
                            call_end_fired,
                            countdown_cancelled,
                        ) {
                            NoCallDecision::DeferToCountdown => {}
                            NoCallDecision::ArmCountdown => {
                                if let Some((process_name, display_name, already_fired)) =
                                    active_snapshot
                                {
                                    if !already_fired {
                                        self.mark_call_end_fired();
                                        self.arm_call_end_countdown(
                                            &app,
                                            &auto_stop,
                                            &recording,
                                            &display_name,
                                            &process_name,
                                            config.call_end_stop_countdown_secs,
                                        );
                                    }
                                }
                            }
                            NoCallDecision::RearmCountdown => {
                                if let Some((process_name, display_name, _)) = active_snapshot {
                                    log_call_detect_event(
                                        "warn",
                                        "call_end_countdown_rearmed",
                                        Some(&display_name),
                                        Some(&process_name),
                                        serde_json::json!({
                                            "reason": "countdown latch was set but countdown_active was false before explicit cancellation or timer firing"
                                        }),
                                    );
                                    self.arm_call_end_countdown(
                                        &app,
                                        &auto_stop,
                                        &recording,
                                        &display_name,
                                        &process_name,
                                        config.call_end_stop_countdown_secs,
                                    );
                                }
                            }
                            NoCallDecision::ClearIfStale => {
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
                }
            }
        });
    }

    /// Emit `call:ended` to the frontend and spawn a thread that auto-stops
    /// the recording when the countdown elapses, unless the user cancels.
    /// Returns immediately; the thread owns its own wakeup cadence.
    fn arm_call_end_countdown(
        &self,
        app: &tauri::AppHandle,
        auto_stop: &CallEndAutoStopHandles,
        recording: &Arc<AtomicBool>,
        display_name: &str,
        process_name: &str,
        countdown_secs: u64,
    ) {
        // Bail if another countdown is already in flight. The UI should only
        // ever see one active banner per call-end transition.
        if auto_stop
            .countdown_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        auto_stop.countdown_cancel.store(false, Ordering::Relaxed);

        let secs = countdown_secs.max(1);
        eprintln!(
            "[call-detect] call ended: {} ({}). Auto-stop armed for {}s",
            display_name, process_name, secs
        );
        log_call_detect_event(
            "info",
            "call_ended",
            Some(display_name),
            Some(process_name),
            serde_json::json!({
                "countdown_secs": secs,
                "reason": "call app no longer detected while recording",
            }),
        );

        app.emit(
            "call:ended",
            CallEndedPayload {
                app_name: display_name.to_string(),
                process_name: process_name.to_string(),
                countdown_secs: secs,
            },
        )
        .ok();

        let app_for_thread = app.clone();
        let cancel = auto_stop.countdown_cancel.clone();
        let active = auto_stop.countdown_active.clone();
        let started_by = auto_stop.recording_started_by_call_detect.clone();
        let stop_flag = auto_stop.stop_flag.clone();
        let recording_flag = recording.clone();
        let display_for_thread = display_name.to_string();

        std::thread::spawn(move || {
            // Poll every 250ms so cancellation and external stops are snappy
            // without busy-spinning.
            let tick = Duration::from_millis(250);
            let total = Duration::from_secs(secs);
            let start = Instant::now();
            loop {
                std::thread::sleep(tick);

                if cancel.load(Ordering::Relaxed) {
                    eprintln!(
                        "[call-detect] auto-stop cancelled for {}",
                        display_for_thread
                    );
                    active.store(false, Ordering::Relaxed);
                    app_for_thread.emit("call:end-countdown:cancelled", ()).ok();
                    return;
                }

                if !recording_flag.load(Ordering::Relaxed) {
                    eprintln!("[call-detect] auto-stop aborted — recording already stopped");
                    active.store(false, Ordering::Relaxed);
                    app_for_thread.emit("call:end-countdown:cancelled", ()).ok();
                    return;
                }

                if start.elapsed() >= total {
                    // Timer elapsed: fire stop via the same mechanism as the
                    // "Stop" button. `request_stop` would require AppState;
                    // the stop_flag is what the recording loop observes.
                    eprintln!(
                        "[call-detect] auto-stop firing stop for {}",
                        display_for_thread
                    );
                    log_call_detect_event(
                        "info",
                        "call_end_auto_stop_fired",
                        Some(&display_for_thread),
                        None,
                        serde_json::json!({ "countdown_secs": secs }),
                    );
                    stop_flag.store(true, Ordering::Relaxed);
                    started_by.store(false, Ordering::Relaxed);
                    active.store(false, Ordering::Relaxed);
                    app_for_thread.emit("call:end-countdown:fired", ()).ok();
                    return;
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
        let has_teams_web = config.apps.iter().any(|app| app == "teams-web");
        let native_apps: Vec<&String> = config
            .apps
            .iter()
            .filter(|app| app.as_str() != "google-meet" && app.as_str() != "teams-web")
            .collect();
        let running = running_process_names();

        if has_google_meet && mic_live && sticky_alive(&self.recent_google_meet_until) {
            return detected_for(MeetingProvider::GoogleMeet);
        }

        if has_teams_web && mic_live && sticky_alive(&self.recent_teams_web_until) {
            return detected_for(MeetingProvider::TeamsWeb);
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

        if (has_google_meet || has_teams_web) && self.browser_probe_due() {
            self.schedule_next_browser_probe();
            match self.detect_browser_meeting(&running, has_google_meet, has_teams_web) {
                BrowserMeetProbe::Detected { provider } => {
                    let sticky = match provider {
                        MeetingProvider::GoogleMeet => &self.recent_google_meet_until,
                        MeetingProvider::TeamsWeb => &self.recent_teams_web_until,
                    };
                    remember_sticky(sticky, provider.sticky_duration());
                    return detected_for(provider);
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

    fn note_active_call(&self, process_name: &str, display_name: &str) -> DetectionTransition {
        let mut active = self.active_call.lock().unwrap();
        let now = Instant::now();
        match active.as_mut() {
            None => {
                *active = Some(ActiveCallState {
                    process_name: process_name.to_string(),
                    display_name: display_name.to_string(),
                    last_notified_at: now,
                    call_end_fired: false,
                });
                DetectionTransition::NewSession
            }
            Some(state) if state.process_name != process_name => {
                *state = ActiveCallState {
                    process_name: process_name.to_string(),
                    display_name: display_name.to_string(),
                    last_notified_at: now,
                    call_end_fired: false,
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

    /// Snapshot of the currently-active call session if any. Returns owned
    /// strings so callers don't have to hold the lock.
    fn active_call_snapshot(&self) -> Option<(String, String, bool)> {
        self.active_call.lock().unwrap().as_ref().map(|state| {
            (
                state.process_name.clone(),
                state.display_name.clone(),
                state.call_end_fired,
            )
        })
    }

    fn mark_call_end_fired(&self) {
        if let Some(state) = self.active_call.lock().unwrap().as_mut() {
            state.call_end_fired = true;
        }
    }

    /// Drop the "countdown already fired this session" latch. Used when a
    /// new call session becomes active during an ongoing recording so
    /// subsequent call-ends can re-arm the auto-stop prompt.
    fn reset_call_end_latch(&self) {
        if let Some(state) = self.active_call.lock().unwrap().as_mut() {
            state.call_end_fired = false;
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
            None,
            Some(browser_app),
            serde_json::json!({
                "reason": reason,
                "backoff_secs": BROWSER_PROBE_BACKOFF_SECS,
            }),
        );
    }

    fn detect_browser_meeting(
        &self,
        running: &[String],
        want_meet: bool,
        want_teams: bool,
    ) -> BrowserMeetProbe {
        let running_lower: Vec<String> = running.iter().map(|s| s.to_lowercase()).collect();
        let mut saw_browser = false;

        for (proc_fragment, app_name, kind, exact) in &[
            (
                "google chrome",
                "Google Chrome",
                BrowserKind::ChromeLike,
                false,
            ),
            (
                "chrome canary",
                "Google Chrome Canary",
                BrowserKind::ChromeLike,
                false,
            ),
            ("chromium", "Chromium", BrowserKind::ChromeLike, false),
            // Arc's binary is exactly "Arc"; substring match would catch
            // searchpartyd / searchpartyuseragent / TrialArchivingService.
            ("arc", "Arc", BrowserKind::ChromeLike, true),
            ("safari", "Safari", BrowserKind::Safari, false),
        ] {
            let proc_match = if *exact {
                running_lower.iter().any(|p| p == proc_fragment)
            } else {
                running_lower.iter().any(|p| p.contains(proc_fragment))
            };
            if !proc_match {
                continue;
            }
            saw_browser = true;
            if !self.browser_probe_allowed_for(app_name) {
                continue;
            }

            match query_browser_tabs(app_name, *kind) {
                AppleScriptProbe::Tabs(tabs) => {
                    for tab in &tabs {
                        if want_meet && looks_like_google_meet_meeting_url(&tab.url) {
                            return BrowserMeetProbe::Detected {
                                provider: MeetingProvider::GoogleMeet,
                            };
                        }
                        if want_teams && looks_like_teams_meeting_tab(&tab.url, &tab.title) {
                            return BrowserMeetProbe::Detected {
                                provider: MeetingProvider::TeamsWeb,
                            };
                        }
                    }
                }
                AppleScriptProbe::PermissionDenied => {
                    self.defer_browser_probe_for(app_name, "apple_events_permission_denied");
                    return BrowserMeetProbe::PermissionDenied {
                        browser_app: (*app_name).to_string(),
                    };
                }
                AppleScriptProbe::Error { stderr } => {
                    let snippet: String = stderr.chars().take(240).collect();
                    let reason = if snippet.is_empty() {
                        "browser_probe_error".to_string()
                    } else {
                        format!("browser_probe_error: {snippet}")
                    };
                    self.defer_browser_probe_for(app_name, &reason);
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
        "teams-web" => "Teams".into(),
        other => other.into(),
    }
}

#[derive(Debug, Clone, Copy)]
enum BrowserKind {
    ChromeLike,
    Safari,
}

#[derive(Debug, Clone)]
struct BrowserTab {
    url: String,
    title: String,
}

enum AppleScriptProbe {
    Tabs(Vec<BrowserTab>),
    PermissionDenied,
    Error { stderr: String },
}

fn query_browser_tabs(app_name: &str, kind: BrowserKind) -> AppleScriptProbe {
    // Chromium tabs expose `title`; Safari tabs expose `name`. The output is
    // line-pairs of URL + title, parsed by `run_applescript_tabs` below.
    let title_property = match kind {
        BrowserKind::ChromeLike => "title",
        BrowserKind::Safari => "name",
    };
    let script = format!(
        r#"tell application "{app_name}"
set output to ""
repeat with w in windows
  repeat with t in tabs of w
    set tabUrl to ""
    set tabTitle to ""
    try
      set tabUrl to (URL of t as text)
    end try
    try
      set tabTitle to ({title_property} of t as text)
    end try
    set output to output & tabUrl & linefeed & tabTitle & linefeed
  end repeat
end repeat
return output
end tell"#
    );
    run_applescript_tabs(&script)
}

fn run_applescript_tabs(script: &str) -> AppleScriptProbe {
    let output = match std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            return AppleScriptProbe::Error {
                stderr: format!("osascript spawn failed: {e}"),
            }
        }
    };

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = text.lines().collect();
        let mut tabs = Vec::with_capacity(lines.len() / 2);
        for chunk in lines.chunks(2) {
            let url = chunk
                .first()
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let title = chunk
                .get(1)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if url.is_empty() && title.is_empty() {
                continue;
            }
            tabs.push(BrowserTab { url, title });
        }
        return AppleScriptProbe::Tabs(tabs);
    }

    let stderr_raw = String::from_utf8_lossy(&output.stderr).to_string();
    let stderr_lc = stderr_raw.to_lowercase();
    if stderr_lc.contains("not authorized")
        || stderr_lc.contains("not permitted")
        || stderr_lc.contains("(-1743)")
    {
        AppleScriptProbe::PermissionDenied
    } else {
        AppleScriptProbe::Error {
            stderr: stderr_raw.trim().to_string(),
        }
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

/// Localized title prefixes the Teams web client sets on `document.title`
/// when the tab is in a meeting or 1:1 call. Chat/calendar/activity tabs use
/// other prefixes (e.g. "Czat | …"), so a prefix match disambiguates the
/// otherwise opaque `teams.*/v2/` URL.
///
/// The list is intentionally small — extend as new locales are reported.
/// All entries must be lowercase; matching is performed against
/// `title.to_lowercase().starts_with(prefix)`.
const TEAMS_MEETING_TITLE_PREFIXES: &[&str] = &[
    // English
    "meeting",
    "call ",
    "calling",
    // Polish
    "spotkanie",
    "połączenie",
    "trwa połączenie",
    // Spanish
    "reunión",
    "reunion",
    "llamada",
    // French
    "réunion",
    "appel",
    // German
    "besprechung",
    "anruf",
    // Portuguese
    "reunião",
    "chamada",
    // Italian
    "riunione",
    "chiamata",
    // Dutch
    "vergadering",
    "gesprek",
    // Russian
    "собрание",
    "встреча",
    "вызов",
    // Czech
    "schůzka",
    "hovor",
    // Hungarian
    "értekezlet",
    "hívás",
    // Romanian
    "ședință",
    "apel",
    // Turkish
    "toplantı",
    "arama",
    // CJK
    "会議",
    "会议",
    "會議",
    "회의",
];

fn title_indicates_teams_meeting(title: &str) -> bool {
    let lower = title.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }
    TEAMS_MEETING_TITLE_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn is_teams_v2_root(url: &str) -> bool {
    let lower = url.trim().to_lowercase();
    let without_scheme = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    without_scheme.starts_with("teams.live.com/v2/")
        || without_scheme.starts_with("teams.microsoft.com/v2/")
        || without_scheme == "teams.live.com/v2"
        || without_scheme == "teams.microsoft.com/v2"
}

/// Combined Teams meeting check: URL pattern OR (Teams v2 root + meeting-y
/// tab title). The title fallback exists because the Teams web SPA does not
/// surface the in-meeting hash route via AppleScript's `URL of t`.
fn looks_like_teams_meeting_tab(url: &str, title: &str) -> bool {
    if looks_like_teams_meeting_url(url) {
        return true;
    }
    is_teams_v2_root(url) && title_indicates_teams_meeting(title)
}

/// Match a Microsoft Teams in-browser meeting URL.
///
/// Accepts the specific paths used for active meeting sessions and rejects the
/// Teams chat / calendar home URLs — matching those would false-positive every
/// time a user leaves Teams open in a tab. The Live v2 web client puts both
/// chat and meetings under `/v2/`, so we cannot accept the bare `/v2/` root.
fn looks_like_teams_meeting_url(url: &str) -> bool {
    let lower = url.trim().to_lowercase();
    let without_scheme = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);

    // Personal Teams (teams.live.com).
    if let Some(rest) = without_scheme.strip_prefix("teams.live.com/") {
        if rest.starts_with("meet/") {
            return true;
        }
        // Hash- or query-routed meeting markers, including under /v2/.
        return rest.contains("pre-join-calling/")
            || rest.contains("meetup-join/")
            || rest.contains("modern-calling/")
            || rest.contains("calling-screen/")
            || rest.contains("meet/");
    }

    let Some(rest) = without_scheme.strip_prefix("teams.microsoft.com/") else {
        return false;
    };

    // Classic join links: /l/meetup-join/... and /meetup-join/...
    if rest.starts_with("l/meetup-join/") || rest.starts_with("meetup-join/") {
        return true;
    }

    // Hash-routed pre-join / in-meeting screens on both the legacy and v2
    // clients. Example: _#/pre-join-calling/..., v2/_#/pre-join-calling/...,
    // v2/#/meetup-join/..., v2/#/modern-calling/...
    if rest.contains("pre-join-calling/")
        || rest.contains("meetup-join/")
        || rest.contains("modern-calling/")
        || rest.contains("calling-screen/")
    {
        return true;
    }

    false
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
            process_names_from_ps_output(&text)
        }
        _ => Vec::new(),
    }
}

fn process_names_from_ps_output(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            // ps returns full paths like /Applications/zoom.us.app/Contents/MacOS/zoom.us.
            // Extract just the binary name.
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            Some(trimmed.rsplit('/').next().unwrap_or(trimmed).to_string())
        })
        .collect()
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

    fn test_call_detection_config(apps: Vec<String>) -> CallDetectionConfig {
        CallDetectionConfig {
            enabled: true,
            poll_interval_secs: 1,
            cooldown_minutes: 5,
            apps,
            stop_when_call_ends: false,
            call_end_stop_countdown_secs: 30,
        }
    }

    #[test]
    fn call_session_rearms_when_process_changes_or_ends() {
        let detector = CallDetector::new(test_call_detection_config(vec!["zoom.us".into()]));

        assert!(matches!(
            detector.note_active_call("zoom.us", "Zoom"),
            DetectionTransition::NewSession
        ));
        assert!(matches!(
            detector.note_active_call("zoom.us", "Zoom"),
            DetectionTransition::Noop
        ));
        detector.clear_active_call();
        assert!(matches!(
            detector.note_active_call("zoom.us", "Zoom"),
            DetectionTransition::NewSession
        ));
        assert!(matches!(
            detector.note_active_call("face.time", "FaceTime"),
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
        let detector = CallDetector::new(test_call_detection_config(vec![
            "zoom.us".into(),
            "google-meet".into(),
        ]));

        assert!(detector
            .current_config()
            .apps
            .iter()
            .any(|app| app == "google-meet"));
    }

    #[test]
    fn browser_probe_is_skipped_when_no_browser_processes_exist() {
        let detector = CallDetector::new(test_call_detection_config(vec!["google-meet".into()]));
        let running: Vec<String> = vec!["Finder".into(), "launchd".into()];
        assert!(matches!(
            detector.detect_browser_meeting(&running, true, false),
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
            run_applescript_tabs("this is not valid applescript @@@@"),
            AppleScriptProbe::Error { .. }
        ));
    }

    #[test]
    fn browser_probe_backoff_resets_after_expiry() {
        let detector = CallDetector::new(test_call_detection_config(vec!["google-meet".into()]));

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
        let detector = CallDetector::new(test_call_detection_config(vec!["google-meet".into()]));

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
        let detector = CallDetector::new(test_call_detection_config(vec![
            "Slack".into(),
            "google-meet".into(),
        ]));

        remember_sticky(
            &detector.recent_google_meet_until,
            MeetingProvider::GoogleMeet.sticky_duration(),
        );
        assert!(sticky_alive(&detector.recent_google_meet_until));

        {
            let mut sticky = detector.recent_google_meet_until.lock().unwrap();
            *sticky = Some(Instant::now() - Duration::from_secs(1));
        }

        assert!(!sticky_alive(&detector.recent_google_meet_until));
    }

    #[test]
    fn native_app_detection_wins_before_browser_meet_probe() {
        let detector = CallDetector::new(test_call_detection_config(vec![
            "zoom.us".into(),
            "google-meet".into(),
        ]));

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
    fn arc_exact_match_does_not_fire_on_system_processes() {
        let detector = CallDetector::new(test_call_detection_config(vec!["google-meet".into()]));
        // These macOS system processes contain "arc" as a substring but must
        // not be treated as the Arc browser.
        let running: Vec<String> = vec![
            "searchpartyd".into(),
            "searchpartyuseragent".into(),
            "TrialArchivingService".into(),
        ];
        assert!(matches!(
            detector.detect_browser_meeting(&running, true, false),
            BrowserMeetProbe::NoBrowserProcesses
        ));
    }

    #[test]
    fn arc_exact_match_fires_on_arc_process() {
        let detector = CallDetector::new(test_call_detection_config(vec!["google-meet".into()]));
        // Defer the probe so `detect_browser_meeting` skips the real
        // AppleScript call to Arc but still records `saw_browser`.
        detector.defer_browser_probe_for("Arc", "test");

        // "Arc" (the browser) must be recognised; "searchpartyd" must not
        // accidentally satisfy the check on its own.
        let running: Vec<String> = vec!["searchpartyd".into(), "Arc".into()];
        assert!(matches!(
            detector.detect_browser_meeting(&running, true, false),
            BrowserMeetProbe::NoMatch
        ));
    }

    #[test]
    fn sticky_google_meet_still_wins_when_no_native_app_is_active() {
        let detector = CallDetector::new(test_call_detection_config(vec![
            "zoom.us".into(),
            "google-meet".into(),
        ]));

        remember_sticky(
            &detector.recent_google_meet_until,
            MeetingProvider::GoogleMeet.sticky_duration(),
        );
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
        assert!(sticky_alive(&detector.recent_google_meet_until));
    }

    #[test]
    fn process_list_parser_extracts_binary_names() {
        let procs = process_names_from_ps_output(
            "\n/Applications/zoom.us.app/Contents/MacOS/zoom.us\nSafari\n  \n",
        );

        assert_eq!(procs, vec!["zoom.us", "Safari"]);
    }

    #[test]
    fn process_list_probe_does_not_panic() {
        let _procs = running_process_names();
    }

    #[test]
    fn mic_check_does_not_panic() {
        // Just verify the function returns without crashing.
        // Will return false unless something is using the mic right now.
        let _result = is_mic_in_use();
    }

    #[test]
    fn call_end_fires_once_per_session() {
        let detector = CallDetector::new(test_call_detection_config(vec!["zoom.us".into()]));

        assert!(matches!(
            detector.note_active_call("zoom.us", "Zoom"),
            DetectionTransition::NewSession
        ));

        let snap = detector.active_call_snapshot().unwrap();
        assert_eq!(snap.0, "zoom.us");
        assert_eq!(snap.1, "Zoom");
        assert!(!snap.2, "call_end_fired should start false");

        detector.mark_call_end_fired();
        let snap = detector.active_call_snapshot().unwrap();
        assert!(snap.2, "call_end_fired must flip to true");

        detector.clear_active_call();
        assert!(
            detector.active_call_snapshot().is_none(),
            "clearing the call must reset the snapshot"
        );

        assert!(matches!(
            detector.note_active_call("zoom.us", "Zoom"),
            DetectionTransition::NewSession
        ));
        let snap = detector.active_call_snapshot().unwrap();
        assert!(!snap.2);
    }

    #[test]
    fn active_countdown_defers_state_transitions() {
        // Regression for issue #129 follow-up (athal7): once the call-end
        // countdown is armed, subsequent "no active call" polls must not
        // clear active_call or re-arm. Only the countdown thread (or a
        // re-detected call) should end the countdown.
        //
        // Before the fix, any transient flip of `is_recording` /
        // `started_by_call_detect` during the 30s window sent the poll into
        // ClearIfStale, which wiped active_call and orphaned the countdown.

        // Happy path: recording + call-detect + stop_when_call_ends, no
        // countdown yet → arm one.
        assert_eq!(
            decide_no_call_action(true, true, true, false, false, false),
            NoCallDecision::ArmCountdown
        );

        // Countdown now active. Same inputs → defer, don't re-arm.
        assert_eq!(
            decide_no_call_action(true, true, true, true, true, false),
            NoCallDecision::DeferToCountdown
        );

        // Countdown active AND is_recording flipped false mid-countdown
        // (e.g. native call capture's target disappeared when Zoom quit).
        // Before the fix this returned ClearIfStale and wiped active_call.
        // After the fix it must defer, leaving the countdown thread to
        // observe `!recording_flag` and shut itself down cleanly.
        assert_eq!(
            decide_no_call_action(false, true, true, true, true, false),
            NoCallDecision::DeferToCountdown
        );

        // Countdown active AND started_by flipped false (e.g. stray
        // cmd_start_recording) — same invariant: defer, don't wipe state.
        assert_eq!(
            decide_no_call_action(true, false, true, true, true, false),
            NoCallDecision::DeferToCountdown
        );

        // Regression for the v0.14.0 follow-up: call_ended already fired,
        // but countdown_active was unexpectedly cleared before the timer
        // emitted call_end_auto_stop_fired. Re-arm rather than logging
        // `cleared` and leaving the recording running forever.
        assert_eq!(
            decide_no_call_action(true, true, true, false, true, false),
            NoCallDecision::RearmCountdown
        );

        // Explicit cancellation ("Keep recording" / "Stop now") is still a
        // real terminal transition. Do not re-arm after the user cancels.
        assert_eq!(
            decide_no_call_action(true, true, true, false, true, true),
            NoCallDecision::ClearIfStale
        );

        // No countdown + no recording in scope → free to clear stale state.
        assert_eq!(
            decide_no_call_action(false, false, true, false, false, false),
            NoCallDecision::ClearIfStale
        );
        assert_eq!(
            decide_no_call_action(true, false, true, false, false, false),
            NoCallDecision::ClearIfStale
        );
        assert_eq!(
            decide_no_call_action(true, true, false, false, false, false),
            NoCallDecision::ClearIfStale
        );
    }

    #[test]
    fn new_session_after_process_change_resets_call_end_fired() {
        // Ending Zoom, auto-stopping, then joining a Teams call must let the
        // Teams session auto-stop too. The "already fired" flag is per-session,
        // not per-detector.
        let detector = CallDetector::new(test_call_detection_config(vec![
            "zoom.us".into(),
            "Microsoft Teams".into(),
        ]));

        detector.note_active_call("zoom.us", "Zoom");
        detector.mark_call_end_fired();

        assert!(matches!(
            detector.note_active_call("Microsoft Teams", "Teams"),
            DetectionTransition::NewSession
        ));
        let snap = detector.active_call_snapshot().unwrap();
        assert_eq!(snap.0, "Microsoft Teams");
        assert!(!snap.2, "new session must reset call_end_fired");
    }

    #[test]
    fn teams_url_requires_real_meeting_path() {
        // Positive cases — these are live meeting URL shapes.
        assert!(looks_like_teams_meeting_url(
            "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc%40thread.v2/0?context=x"
        ));
        assert!(looks_like_teams_meeting_url(
            "https://teams.microsoft.com/meetup-join/19%3ameeting_abc%40thread.v2/0"
        ));
        assert!(looks_like_teams_meeting_url(
            "https://teams.live.com/meet/9876543210"
        ));
        assert!(looks_like_teams_meeting_url(
            "https://teams.live.com/v2/#/modern-calling/19:meeting_x@thread.v2/0"
        ));
        assert!(looks_like_teams_meeting_url(
            "https://teams.live.com/v2/#/calling-screen/19:meeting_y@thread.v2"
        ));
        // Teams Live v2 chat / home — must NOT match.
        assert!(!looks_like_teams_meeting_url("https://teams.live.com/v2/"));
        assert!(!looks_like_teams_meeting_url(
            "https://teams.live.com/v2/#/conversations/19:abc@thread.v2"
        ));
        assert!(looks_like_teams_meeting_url(
            "https://teams.microsoft.com/_#/pre-join-calling/19:meeting_abc@thread.v2"
        ));
        assert!(looks_like_teams_meeting_url(
            "https://teams.microsoft.com/v2/#/meetup-join/19:meeting_xyz@thread.v2/0"
        ));

        // Negative cases — these are Teams UI pages, not meetings.
        assert!(!looks_like_teams_meeting_url(
            "https://teams.microsoft.com/"
        ));
        assert!(!looks_like_teams_meeting_url(
            "https://teams.microsoft.com/_#/conversations/foo"
        ));
        assert!(!looks_like_teams_meeting_url(
            "https://teams.microsoft.com/_#/calendarv2"
        ));
        assert!(!looks_like_teams_meeting_url("https://teams.live.com/"));
        assert!(!looks_like_teams_meeting_url(
            "https://example.com/l/meetup-join/abc"
        ));
    }

    #[test]
    fn teams_v2_root_with_meeting_title_matches() {
        // Real-world example pulled from the user's Teams Live tab while in a
        // meeting — URL is opaque, title carries the localized "Spotkanie" prefix.
        assert!(looks_like_teams_meeting_tab(
            "https://teams.live.com/v2/",
            "Spotkanie | Meeting with Romuald Członkowski | Microsoft Teams"
        ));
        assert!(looks_like_teams_meeting_tab(
            "https://teams.live.com/v2/",
            "Meeting | Standup | Microsoft Teams"
        ));
        assert!(looks_like_teams_meeting_tab(
            "https://teams.microsoft.com/v2/",
            "Calling Romuald | Microsoft Teams"
        ));
        // Chat tab on the same `/v2/` URL must not match — title prefix differs.
        assert!(!looks_like_teams_meeting_tab(
            "https://teams.live.com/v2/",
            "Czat | 🔒 GitHub Secure | Microsoft Teams"
        ));
        assert!(!looks_like_teams_meeting_tab(
            "https://teams.live.com/v2/",
            "Chat | Project Apollo | Microsoft Teams"
        ));
        // Off-Teams URL with a meeting-y title should still be rejected.
        assert!(!looks_like_teams_meeting_tab(
            "https://example.com/v2/",
            "Meeting notes | Example"
        ));
    }

    #[test]
    fn sticky_teams_web_detection_survives_between_browser_probes() {
        let detector = CallDetector::new(test_call_detection_config(vec![
            "Slack".into(),
            "teams-web".into(),
        ]));

        remember_sticky(
            &detector.recent_teams_web_until,
            MeetingProvider::TeamsWeb.sticky_duration(),
        );
        assert!(sticky_alive(&detector.recent_teams_web_until));

        {
            let mut sticky = detector.recent_teams_web_until.lock().unwrap();
            *sticky = Some(Instant::now() - Duration::from_secs(1));
        }

        assert!(!sticky_alive(&detector.recent_teams_web_until));
    }
}
