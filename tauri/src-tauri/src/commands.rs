use crate::call_capture;
use minutes_core::capture::RecordingIntent;
use minutes_core::{CaptureMode, Config, ContentType};
use std::cmp::Reverse;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_shell::ShellExt;

pub struct AppState {
    pub recording: Arc<AtomicBool>,
    pub starting: Arc<AtomicBool>,
    pub stop_flag: Arc<AtomicBool>,
    pub processing: Arc<AtomicBool>,
    pub processing_stage: Arc<Mutex<Option<String>>>,
    pub latest_output: Arc<Mutex<Option<OutputNotice>>>,
    pub call_capture_health: Arc<Mutex<Option<crate::call_capture::CallSourceHealth>>>,
    pub completion_notifications_enabled: Arc<AtomicBool>,
    pub global_hotkey_enabled: Arc<AtomicBool>,
    pub global_hotkey_shortcut: Arc<Mutex<String>>,
    pub hotkey_runtime: Arc<Mutex<HotkeyRuntime>>,
    pub discard_short_hotkey_capture: Arc<AtomicBool>,
    pub pty_manager: Arc<Mutex<crate::pty::PtyManager>>,
    pub dictation_active: Arc<AtomicBool>,
    pub dictation_stop_flag: Arc<AtomicBool>,
    pub dictation_shortcut_enabled: Arc<AtomicBool>,
    pub dictation_shortcut: Arc<Mutex<String>>,
    pub live_transcript_active: Arc<AtomicBool>,
    pub live_transcript_stop_flag: Arc<AtomicBool>,
    pub live_shortcut_enabled: Arc<AtomicBool>,
    pub live_shortcut: Arc<Mutex<String>>,
    pub pending_update: Arc<Mutex<Option<PendingUpdate>>>,
    /// Whether the palette global shortcut is currently registered.
    pub palette_shortcut_enabled: Arc<AtomicBool>,
    /// The shortcut string registered for the palette (e.g. "CmdOrCtrl+Shift+K").
    pub palette_shortcut: Arc<Mutex<String>>,
    /// Explicit lifecycle state for the palette overlay window. Tracked as a
    /// four-state machine (Closed/Opening/Open/Closing) rather than a boolean
    /// so fast `⌘⇧K` mashing during the close path doesn't eat keypresses.
    /// See PLAN.md.command-palette-slice-2 D3.
    pub palette_lifecycle: Arc<Mutex<PaletteLifecycle>>,
    /// Set when a hotkey press lands in the `Closing` state. The close path
    /// drains this flag on completion and re-opens the palette if it was set.
    pub palette_reopen_pending: Arc<AtomicBool>,
}

/// Lifecycle state for the palette overlay window.
///
/// Transitions:
/// ```text
///     Closed ──hotkey──▶ Opening ──build_window──▶ Open
///     Open   ──hotkey──▶ Closing ──close──▶ Closed
///     Open   ──focus-lost──▶ Closing ──close──▶ Closed
///     Opening + hotkey  ==> ignored (mid-open race)
///     Closing + hotkey  ==> queue reopen; Closed triggers Opening again
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaletteLifecycle {
    #[default]
    Closed,
    Opening,
    Open,
    Closing,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingUpdate {
    pub version: String,
    pub body: String,
}

/// Surface a deferred update notification if one is pending and no session is active.
/// Call this after recording/live/dictation stops.
pub fn surface_deferred_update(app: &tauri::AppHandle) {
    let state = match app.try_state::<AppState>() {
        Some(s) => s,
        None => return,
    };
    if state.recording.load(Ordering::Relaxed)
        || state.starting.load(Ordering::Relaxed)
        || state.processing.load(Ordering::Relaxed)
        || state.live_transcript_active.load(Ordering::Relaxed)
        || state.dictation_active.load(Ordering::Relaxed)
    {
        return;
    }
    let pending = match state.pending_update.lock() {
        Ok(mut guard) => guard.take(),
        Err(_) => return,
    };
    if let Some(update) = pending {
        let _ = app.emit(
            "update-ready",
            serde_json::json!({
                "version": update.version,
                "body": update.body,
            }),
        );
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MeetingSection {
    pub heading: String,
    pub content: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SpeakerAttributionView {
    pub speaker_label: String,
    pub name: String,
    pub confidence: String,
    pub source: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ActionItemView {
    pub assignee: String,
    pub task: String,
    pub due: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DecisionView {
    pub text: String,
    pub topic: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MeetingDetail {
    pub path: String,
    pub title: String,
    pub date: String,
    pub duration: String,
    pub content_type: String,
    pub status: Option<String>,
    pub context: Option<String>,
    pub attendees: Vec<String>,
    pub calendar_event: Option<String>,
    pub action_items: Vec<ActionItemView>,
    pub decisions: Vec<DecisionView>,
    pub sections: Vec<MeetingSection>,
    pub speaker_map: Vec<SpeakerAttributionView>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OutputNotice {
    pub kind: String,
    pub title: String,
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReadinessItem {
    pub label: String,
    pub state: String,
    pub detail: String,
    pub optional: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RecoveryItem {
    pub kind: String,
    pub title: String,
    pub path: String,
    pub detail: String,
    pub retry_type: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessingJobView {
    pub id: String,
    pub title: String,
    pub mode: String,
    pub state: String,
    pub stage: Option<String>,
    pub output_path: Option<String>,
    pub audio_path: String,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub word_count: Option<usize>,
}

fn processing_job_view(job: minutes_core::jobs::ProcessingJob) -> ProcessingJobView {
    ProcessingJobView {
        id: job.id,
        title: job.title.unwrap_or_else(|| "Queued recording".into()),
        mode: match job.mode {
            CaptureMode::Meeting => "meeting".into(),
            CaptureMode::QuickThought => "quick-thought".into(),
            CaptureMode::Dictation => "dictation".into(),
            CaptureMode::LiveTranscript => "live-transcript".into(),
        },
        state: match job.state {
            minutes_core::jobs::JobState::Queued => "queued".into(),
            minutes_core::jobs::JobState::Transcribing => "transcribing".into(),
            minutes_core::jobs::JobState::TranscriptOnly => "transcript-only".into(),
            minutes_core::jobs::JobState::Diarizing => "diarizing".into(),
            minutes_core::jobs::JobState::Summarizing => "summarizing".into(),
            minutes_core::jobs::JobState::Saving => "saving".into(),
            minutes_core::jobs::JobState::NeedsReview => "needs-review".into(),
            minutes_core::jobs::JobState::Complete => "complete".into(),
            minutes_core::jobs::JobState::Failed => "failed".into(),
        },
        stage: job.stage,
        output_path: job.output_path,
        audio_path: job.audio_path,
        error: job.error,
        created_at: job.created_at.to_rfc3339(),
        started_at: job.started_at.map(|ts| ts.to_rfc3339()),
        finished_at: job.finished_at.map(|ts| ts.to_rfc3339()),
        word_count: job.word_count,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HotkeyChoice {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HotkeySettings {
    pub enabled: bool,
    pub shortcut: String,
    pub choices: Vec<HotkeyChoice>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCapabilities {
    pub platform: String,
    pub folder_reveal_label: String,
    pub supports_calendar_integration: bool,
    pub supports_call_detection: bool,
    pub supports_tray_artifact_copy: bool,
    pub supports_dictation_hotkey: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TerminalInfo {
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyCaptureStyle {
    Hold,
    Locked,
}

#[derive(Debug, Default)]
pub struct HotkeyRuntime {
    pub key_down: bool,
    pub key_down_started_at: Option<Instant>,
    pub active_capture: Option<HotkeyCaptureStyle>,
    pub recording_started_at: Option<Instant>,
    pub hold_generation: u64,
}

const HOTKEY_CHOICES: [(&str, &str); 3] = [
    ("CmdOrCtrl+Shift+M", "Cmd/Ctrl + Shift + M"),
    ("CmdOrCtrl+Shift+J", "Cmd/Ctrl + Shift + J"),
    ("CmdOrCtrl+Shift+T", "Cmd/Ctrl + Shift + T"),
];
const DICTATION_SHORTCUT_CHOICES: [(&str, &str); 3] = [
    ("CmdOrCtrl+Shift+Space", "Cmd/Ctrl + Shift + Space"),
    ("CmdOrCtrl+Alt+Space", "Cmd/Ctrl + Option/Alt + Space"),
    ("CmdOrCtrl+Shift+D", "Cmd/Ctrl + Shift + D"),
];
const HOTKEY_HOLD_THRESHOLD_MS: u64 = 300;
const HOTKEY_MIN_DURATION_MS: u64 = 400;

pub fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

pub fn supports_calendar_integration() -> bool {
    cfg!(target_os = "macos")
}

pub fn supports_call_detection() -> bool {
    cfg!(target_os = "macos")
}

pub fn supports_tray_artifact_copy() -> bool {
    cfg!(target_os = "macos")
}

pub fn supports_dictation_hotkey() -> bool {
    cfg!(target_os = "macos")
}

pub fn folder_reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Show in Finder"
    } else if cfg!(target_os = "windows") {
        "Show in Explorer"
    } else {
        "Show in Folder"
    }
}

pub fn default_hotkey_shortcut() -> &'static str {
    HOTKEY_CHOICES[0].0
}

pub fn default_dictation_shortcut() -> &'static str {
    DICTATION_SHORTCUT_CHOICES[0].0
}

fn shortcut_choices(choices: &[(&str, &str)]) -> Vec<HotkeyChoice> {
    choices
        .iter()
        .map(|(value, label)| HotkeyChoice {
            value: (*value).to_string(),
            label: (*label).to_string(),
        })
        .collect()
}

fn hotkey_choices() -> Vec<HotkeyChoice> {
    shortcut_choices(&HOTKEY_CHOICES)
}

fn dictation_shortcut_choices() -> Vec<HotkeyChoice> {
    shortcut_choices(&DICTATION_SHORTCUT_CHOICES)
}

fn validate_shortcut(shortcut: &str, choices: &[(&str, &str)]) -> Result<String, String> {
    choices
        .iter()
        .find_map(|(value, _)| (*value == shortcut).then(|| (*value).to_string()))
        .ok_or_else(|| {
            format!(
                "Unsupported shortcut: {}. Choose one of: {}",
                shortcut,
                choices
                    .iter()
                    .map(|(_, label)| *label)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

fn validate_hotkey_shortcut(shortcut: &str) -> Result<String, String> {
    validate_shortcut(shortcut, &HOTKEY_CHOICES)
}

fn validate_dictation_shortcut(shortcut: &str) -> Result<String, String> {
    validate_shortcut(shortcut, &DICTATION_SHORTCUT_CHOICES)
}

fn current_hotkey_settings(state: &AppState) -> HotkeySettings {
    let shortcut = state
        .global_hotkey_shortcut
        .lock()
        .ok()
        .map(|value| value.clone())
        .unwrap_or_else(|| default_hotkey_shortcut().to_string());
    HotkeySettings {
        enabled: state.global_hotkey_enabled.load(Ordering::Relaxed),
        shortcut,
        choices: hotkey_choices(),
    }
}

fn current_dictation_shortcut_settings(state: &AppState) -> HotkeySettings {
    let shortcut = state
        .dictation_shortcut
        .lock()
        .ok()
        .map(|value| value.clone())
        .unwrap_or_else(|| default_dictation_shortcut().to_string());
    HotkeySettings {
        enabled: state.dictation_shortcut_enabled.load(Ordering::Relaxed),
        shortcut,
        choices: dictation_shortcut_choices(),
    }
}

fn clear_hotkey_runtime(runtime: &Arc<Mutex<HotkeyRuntime>>) {
    if let Ok(mut current) = runtime.lock() {
        current.key_down = false;
        current.key_down_started_at = None;
        current.active_capture = None;
        current.recording_started_at = None;
    }
}

fn should_discard_hotkey_capture(started_at: Option<Instant>, now: Instant) -> bool {
    started_at
        .map(|started| now.duration_since(started).as_millis() < HOTKEY_MIN_DURATION_MS as u128)
        .unwrap_or(false)
}

fn reset_hotkey_capture_state(
    runtime: Option<&Arc<Mutex<HotkeyRuntime>>>,
    discard_short_hotkey_capture: Option<&Arc<AtomicBool>>,
) {
    if let Some(flag) = discard_short_hotkey_capture {
        flag.store(false, Ordering::Relaxed);
    }
    if let Some(runtime) = runtime {
        clear_hotkey_runtime(runtime);
    }
}

#[cfg(target_os = "macos")]
fn is_short_hotkey_tap(started_at: Option<Instant>, now: Instant) -> bool {
    started_at
        .map(|pressed| now.duration_since(pressed).as_millis() < HOTKEY_HOLD_THRESHOLD_MS as u128)
        .unwrap_or(false)
}

fn preserve_failed_capture(wav_path: &std::path::Path, config: &Config) -> Option<PathBuf> {
    let metadata = wav_path.metadata().ok()?;
    if metadata.len() == 0 {
        return None;
    }

    let dir = config.output_dir.join("failed-captures");
    std::fs::create_dir_all(&dir).ok()?;
    let dest = dir.join(format!(
        "{}-capture.wav",
        chrono::Local::now().format("%Y-%m-%d-%H%M%S")
    ));

    std::fs::copy(wav_path, &dest).ok()?;
    std::fs::remove_file(wav_path).ok();
    Some(dest)
}

fn preserve_failed_capture_path(path: &std::path::Path, config: &Config) -> Option<PathBuf> {
    let metadata = path.metadata().ok()?;
    if metadata.len() == 0 {
        return None;
    }

    let dir = config.output_dir.join("failed-captures");
    std::fs::create_dir_all(&dir).ok()?;
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("bin");
    let dest = dir.join(format!(
        "{}-capture.{}",
        chrono::Local::now().format("%Y-%m-%d-%H%M%S"),
        ext
    ));

    std::fs::copy(path, &dest).ok()?;
    std::fs::remove_file(path).ok();
    Some(dest)
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn start_native_call_recording(
    app_handle: &tauri::AppHandle,
    recording: &Arc<AtomicBool>,
    starting: &Arc<AtomicBool>,
    stop_flag: &Arc<AtomicBool>,
    processing: &Arc<AtomicBool>,
    processing_stage: &Arc<Mutex<Option<String>>>,
    latest_output: &Arc<Mutex<Option<OutputNotice>>>,
    call_capture_health: &Arc<Mutex<Option<crate::call_capture::CallSourceHealth>>>,
    completion_notifications_enabled: &Arc<AtomicBool>,
    hotkey_runtime: Option<&Arc<Mutex<HotkeyRuntime>>>,
    discard_short_hotkey_capture: Option<&Arc<AtomicBool>>,
    mode: CaptureMode,
    config: &Config,
    requested_title: Option<String>,
) -> Result<(), String> {
    minutes_core::pid::create().map_err(|error| error.to_string())?;
    let mut session = match call_capture::start_native_call_capture() {
        Ok(session) => session,
        Err(error) => {
            minutes_core::pid::remove().ok();
            return Err(error);
        }
    };
    let output_path = session.output_path().to_path_buf();
    let recording_started_at = chrono::Local::now();

    starting.store(false, Ordering::Relaxed);
    recording.store(true, Ordering::Relaxed);
    stop_flag.store(false, Ordering::Relaxed);
    sync_processing_indicator(processing, processing_stage);
    set_latest_output(latest_output, None);
    if let Ok(mut health) = call_capture_health.lock() {
        *health = Some(session.source_health());
    }
    minutes_core::pid::write_recording_metadata(mode).ok();
    crate::update_tray_state(app_handle, true);
    minutes_core::notes::save_recording_start().ok();

    eprintln!(
        "[minutes] Native call capture started: {}",
        output_path.display()
    );

    while !stop_flag.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
        if let Ok(mut health) = call_capture_health.lock() {
            *health = Some(session.source_health());
        }
        if minutes_core::pid::check_and_clear_sentinel() {
            break;
        }
        if let Some(status) = session.try_wait()? {
            if !status.success() {
                let preserved = preserve_failed_capture_path(&output_path, config);
                minutes_core::pid::remove().ok();
                minutes_core::pid::clear_recording_metadata().ok();
                minutes_core::notes::cleanup();
                recording.store(false, Ordering::Relaxed);
                starting.store(false, Ordering::Relaxed);
                if let Ok(mut health) = call_capture_health.lock() {
                    *health = None;
                }
                if let Some(saved) = preserved {
                    let notice = OutputNotice {
                        kind: "preserved-capture".into(),
                        title: "Native call capture failed".into(),
                        path: saved.display().to_string(),
                        detail:
                            "ScreenCaptureKit capture ended early, but the raw output was preserved."
                                .into(),
                    };
                    set_latest_output(latest_output, Some(notice.clone()));
                    maybe_show_completion_notification(
                        app_handle,
                        completion_notifications_enabled,
                        &notice,
                    );
                }
                reset_hotkey_capture_state(hotkey_runtime, discard_short_hotkey_capture);
                return Ok(());
            }
            break;
        }
    }

    if let Err(error) = session.stop() {
        let preserved = preserve_failed_capture_path(&output_path, config);
        minutes_core::notes::cleanup();
        minutes_core::pid::remove().ok();
        minutes_core::pid::clear_recording_metadata().ok();
        processing.store(false, Ordering::Relaxed);
        set_processing_stage(processing_stage, None);
        starting.store(false, Ordering::Relaxed);
        recording.store(false, Ordering::Relaxed);
        if let Ok(mut health) = call_capture_health.lock() {
            *health = None;
        }
        if let Some(saved) = preserved {
            let notice = OutputNotice {
                kind: "preserved-capture".into(),
                title: "Native call capture preserved".into(),
                path: saved.display().to_string(),
                detail: format!("Stopping native call capture failed: {}", error),
            };
            set_latest_output(latest_output, Some(notice.clone()));
            maybe_show_completion_notification(
                app_handle,
                completion_notifications_enabled,
                &notice,
            );
        }
        reset_hotkey_capture_state(hotkey_runtime, discard_short_hotkey_capture);
        return Ok(());
    }

    recording.store(false, Ordering::Relaxed);
    if let Ok(mut health) = call_capture_health.lock() {
        *health = Some(session.source_health());
    }
    let should_discard = discard_short_hotkey_capture
        .as_ref()
        .map(|flag| flag.swap(false, Ordering::Relaxed))
        .unwrap_or(false);
    if should_discard {
        if output_path.exists() {
            std::fs::remove_file(&output_path).ok();
        }
        minutes_core::notes::cleanup();
        minutes_core::pid::remove().ok();
        minutes_core::pid::clear_recording_metadata().ok();
        starting.store(false, Ordering::Relaxed);
        if let Ok(mut health) = call_capture_health.lock() {
            *health = None;
        }
        reset_hotkey_capture_state(hotkey_runtime, discard_short_hotkey_capture);
        return Ok(());
    }

    let recording_finished_at = chrono::Local::now();
    let user_notes = minutes_core::notes::read_notes();
    let pre_context = minutes_core::notes::read_context();
    // Don't block the stop path with a calendar query (can take 10s if Calendar.app hangs).
    // The pipeline already falls back to events_overlapping_now() during background processing.
    let calendar_event = None;

    match minutes_core::jobs::enqueue_capture_job(
        mode,
        requested_title,
        output_path.clone(),
        user_notes,
        pre_context,
        Some(recording_started_at),
        Some(recording_finished_at),
        calendar_event,
    ) {
        Ok(job) => {
            processing.store(true, Ordering::Relaxed);
            set_processing_stage(processing_stage, job.stage.as_deref());
            minutes_core::pid::set_processing_status(
                job.stage.as_deref(),
                Some(mode),
                job.title.as_deref(),
                Some(&job.id),
                minutes_core::jobs::active_job_count(),
            )
            .ok();
            minutes_core::pid::remove().ok();
            minutes_core::pid::clear_recording_metadata().ok();
            minutes_core::notes::cleanup();
            if let Ok(mut health) = call_capture_health.lock() {
                *health = Some(session.source_health());
            }
            spawn_processing_worker(
                app_handle.clone(),
                processing.clone(),
                processing_stage.clone(),
                latest_output.clone(),
                completion_notifications_enabled.clone(),
            );
            sync_processing_indicator(processing, processing_stage);
        }
        Err(error) => {
            let preserved = preserve_failed_capture_path(&output_path, config);
            minutes_core::notes::cleanup();
            minutes_core::pid::remove().ok();
            minutes_core::pid::clear_recording_metadata().ok();
            processing.store(false, Ordering::Relaxed);
            set_processing_stage(processing_stage, None);
            if let Ok(mut health) = call_capture_health.lock() {
                *health = None;
            }
            if let Some(saved) = preserved {
                let notice = OutputNotice {
                    kind: "preserved-capture".into(),
                    title: "Native call capture preserved".into(),
                    path: saved.display().to_string(),
                    detail: format!(
                        "Failed to queue native call capture for processing: {}",
                        error
                    ),
                };
                set_latest_output(latest_output, Some(notice.clone()));
                maybe_show_completion_notification(
                    app_handle,
                    completion_notifications_enabled,
                    &notice,
                );
            }
            starting.store(false, Ordering::Relaxed);
            reset_hotkey_capture_state(hotkey_runtime, discard_short_hotkey_capture);
            return Ok(());
        }
    }

    starting.store(false, Ordering::Relaxed);
    reset_hotkey_capture_state(hotkey_runtime, discard_short_hotkey_capture);
    Ok(())
}

pub fn recording_active(recording: &Arc<AtomicBool>) -> bool {
    recording.load(Ordering::Relaxed) || minutes_core::pid::status().recording
}

pub fn request_stop(
    recording: &Arc<AtomicBool>,
    stop_flag: &Arc<AtomicBool>,
) -> Result<(), String> {
    match minutes_core::pid::check_recording() {
        Ok(Some(pid)) => {
            if pid == std::process::id() {
                stop_flag.store(true, Ordering::Relaxed);
                recording.store(true, Ordering::Relaxed);
                Ok(())
            } else {
                minutes_core::pid::write_stop_sentinel().map_err(|e| e.to_string())?;

                #[cfg(unix)]
                {
                    let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                    if rc != 0 {
                        let err = std::io::Error::last_os_error();
                        eprintln!(
                            "SIGTERM failed (PID {}): {} — sentinel file will stop recording",
                            pid, err
                        );
                    }
                }

                Ok(())
            }
        }
        Ok(None) => {
            recording.store(false, Ordering::Relaxed);
            Err("Not recording".into())
        }
        Err(e) => Err(e.to_string()),
    }
}

fn wait_for_path_removal(path: &std::path::Path, timeout: Option<std::time::Duration>) -> bool {
    let start = std::time::Instant::now();
    while path.exists() {
        if let Some(timeout) = timeout {
            if start.elapsed() >= timeout {
                return false;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    true
}

pub fn wait_for_recording_shutdown(timeout: std::time::Duration) -> bool {
    let pid_path = minutes_core::pid::pid_path();
    wait_for_path_removal(&pid_path, Some(timeout))
}

pub fn wait_for_recording_shutdown_forever() {
    let pid_path = minutes_core::pid::pid_path();
    let _ = wait_for_path_removal(&pid_path, None);
}

fn parse_capture_mode(mode: Option<&str>) -> Result<CaptureMode, String> {
    match mode.unwrap_or("meeting") {
        "meeting" => Ok(CaptureMode::Meeting),
        "quick-thought" => Ok(CaptureMode::QuickThought),
        other => Err(format!(
            "Unsupported recording mode: {}. Use 'meeting' or 'quick-thought'.",
            other
        )),
    }
}

fn parse_recording_intent(intent: Option<&str>) -> Result<Option<RecordingIntent>, String> {
    match intent.unwrap_or("auto") {
        "auto" => Ok(None),
        "memo" => Ok(Some(RecordingIntent::Memo)),
        "room" => Ok(Some(RecordingIntent::Room)),
        "call" => Ok(Some(RecordingIntent::Call)),
        other => Err(format!(
            "Unsupported recording intent: {}. Use auto, memo, room, or call.",
            other
        )),
    }
}

fn parse_optional_string_setting(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn call_detection_has_sentinel(config: &Config, sentinel: &str) -> bool {
    config.call_detection.apps.iter().any(|app| app == sentinel)
}

fn set_call_detection_sentinel(config: &mut Config, sentinel: &str, enabled: bool) {
    config.call_detection.apps.retain(|app| app != sentinel);
    if enabled {
        config.call_detection.apps.push(sentinel.to_string());
    }
}

#[cfg(test)]
fn stage_label(stage: minutes_core::pipeline::PipelineStage, mode: CaptureMode) -> &'static str {
    match (stage, mode) {
        (minutes_core::pipeline::PipelineStage::Transcribing, CaptureMode::Meeting) => {
            "Transcribing meeting"
        }
        (minutes_core::pipeline::PipelineStage::Transcribing, CaptureMode::QuickThought) => {
            "Transcribing quick thought"
        }
        (minutes_core::pipeline::PipelineStage::Diarizing, _) => "Separating speakers",
        (minutes_core::pipeline::PipelineStage::Summarizing, CaptureMode::Meeting) => {
            "Generating meeting summary"
        }
        (minutes_core::pipeline::PipelineStage::Summarizing, CaptureMode::QuickThought) => {
            "Generating memo summary"
        }
        (minutes_core::pipeline::PipelineStage::Saving, CaptureMode::Meeting) => "Saving meeting",
        (minutes_core::pipeline::PipelineStage::Saving, CaptureMode::QuickThought) => {
            "Saving quick thought"
        }
        (minutes_core::pipeline::PipelineStage::Transcribing, CaptureMode::Dictation) => {
            "Transcribing dictation"
        }
        (minutes_core::pipeline::PipelineStage::Summarizing, CaptureMode::Dictation) => {
            "Generating dictation summary"
        }
        (minutes_core::pipeline::PipelineStage::Saving, CaptureMode::Dictation) => {
            "Saving dictation"
        }
        (_, CaptureMode::LiveTranscript) => "Processing live transcript",
    }
}

fn set_processing_stage(stage: &Arc<Mutex<Option<String>>>, value: Option<&str>) {
    if let Ok(mut current) = stage.lock() {
        *current = value.map(String::from);
    }
}

fn set_latest_output(
    latest_output: &Arc<Mutex<Option<OutputNotice>>>,
    notice: Option<OutputNotice>,
) {
    if let Ok(mut current) = latest_output.lock() {
        *current = notice;
    }
}

fn sync_processing_indicator(
    processing: &Arc<AtomicBool>,
    processing_stage: &Arc<Mutex<Option<String>>>,
) {
    let summary = minutes_core::jobs::processing_summary();
    processing.store(summary.is_some(), Ordering::Relaxed);
    set_processing_stage(
        processing_stage,
        summary.as_ref().and_then(|job| job.stage.as_deref()),
    );
}

fn output_notice_from_job(job: &minutes_core::jobs::ProcessingJob) -> Option<OutputNotice> {
    match job.state {
        minutes_core::jobs::JobState::NeedsReview => Some(OutputNotice {
            kind: "preserved-capture".into(),
            title: job
                .title
                .clone()
                .unwrap_or_else(|| "Recording needs review".into()),
            path: job.audio_path.clone(),
            detail: job.error.clone().unwrap_or_else(|| {
                "Transcript was marked as no speech. Raw capture preserved for retry.".into()
            }),
        }),
        minutes_core::jobs::JobState::Complete => {
            job.output_path.as_ref().map(|path| OutputNotice {
                kind: "saved".into(),
                title: job
                    .title
                    .clone()
                    .unwrap_or_else(|| "Processed recording".into()),
                path: path.clone(),
                detail: "Saved meeting markdown".into(),
            })
        }
        minutes_core::jobs::JobState::Failed => {
            let path = job
                .output_path
                .clone()
                .unwrap_or_else(|| job.audio_path.clone());
            Some(OutputNotice {
                kind: "preserved-capture".into(),
                title: job
                    .title
                    .clone()
                    .unwrap_or_else(|| "Processing failed".into()),
                path,
                detail: job
                    .error
                    .clone()
                    .unwrap_or_else(|| "Processing failed, recoverable capture preserved.".into()),
            })
        }
        _ => None,
    }
}

pub fn spawn_processing_worker(
    app_handle: tauri::AppHandle,
    processing: Arc<AtomicBool>,
    processing_stage: Arc<Mutex<Option<String>>>,
    latest_output: Arc<Mutex<Option<OutputNotice>>>,
    completion_notifications_enabled: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let config = Config::load();
        let result = minutes_core::jobs::process_pending_jobs(&config, |job| {
            sync_processing_indicator(&processing, &processing_stage);

            if let Some(notice) = output_notice_from_job(job) {
                set_latest_output(&latest_output, Some(notice.clone()));
                maybe_show_completion_notification(
                    &app_handle,
                    &completion_notifications_enabled,
                    &notice,
                );
            }
        });

        if let Err(error) = result {
            if !matches!(
                error,
                minutes_core::MinutesError::Pid(minutes_core::error::PidError::AlreadyRecording(_))
            ) {
                eprintln!("[minutes] processing worker failed: {}", error);
            }
        }

        sync_processing_indicator(&processing, &processing_stage);
    });
}

fn display_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_display = home.display().to_string();
        if let Some(stripped) = path.strip_prefix(&home_display) {
            return format!("~{}", stripped);
        }
    }
    path.to_string()
}

#[cfg(target_os = "macos")]
fn escape_applescript_literal(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

#[cfg(not(target_os = "macos"))]
fn escape_applescript_literal(text: &str) -> String {
    text.to_string()
}

pub fn open_target(app_handle: &tauri::AppHandle, target: &str) -> Result<(), String> {
    #[allow(deprecated)]
    app_handle
        .shell()
        .open(target.to_string(), None)
        .map_err(|e| e.to_string())
}

fn maybe_show_completion_notification(
    app_handle: &tauri::AppHandle,
    notifications_enabled: &Arc<AtomicBool>,
    notice: &OutputNotice,
) {
    if !notifications_enabled.load(Ordering::Relaxed) {
        return;
    }

    let should_notify = app_handle
        .get_webview_window("main")
        .map(|window| {
            let visible = window.is_visible().ok().unwrap_or(false);
            let focused = window.is_focused().ok().unwrap_or(false);
            !(visible && focused)
        })
        .unwrap_or(true);

    if !should_notify {
        return;
    }

    let body = format!("{} {}", notice.detail, display_path(&notice.path));
    show_user_notification(app_handle, &notice.title, &body);
}

pub fn show_user_notification(app_handle: &tauri::AppHandle, title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let identifier = app_handle.config().identifier.as_str();
        let _ = notify_rust::set_application(identifier);

        let mut notification = notify_rust::Notification::new();
        notification.summary(title);
        notification.body(body);
        notification.auto_icon();

        if notification.show().is_ok() {
            return;
        }
    }

    let plugin_notification_result = app_handle
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();

    if plugin_notification_result.is_ok() {
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"Minutes\" subtitle \"{}\"",
            escape_applescript_literal(body),
            escape_applescript_literal(title)
        );

        if std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .spawn()
            .is_ok()
        {
            return;
        }
    }

    app_handle
        .dialog()
        .message(body.to_string())
        .title(title.to_string())
        .kind(MessageDialogKind::Info)
        .show(|_| {});
}

pub fn frontmost_application_name() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let script = r#"tell application "System Events" to get name of first application process whose frontmost is true"#;
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if name.is_empty() || name == "Minutes" {
            None
        } else {
            Some(name)
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn latest_saved_artifact_path(
    latest_output: &Arc<Mutex<Option<OutputNotice>>>,
) -> Result<PathBuf, String> {
    if let Ok(current) = latest_output.lock() {
        if let Some(notice) = current.clone() {
            if notice.kind == "saved" && !notice.path.trim().is_empty() {
                let path = PathBuf::from(notice.path);
                if path.exists() {
                    return Ok(path);
                }
            }
        }
    }

    let config = Config::load();
    let filters = minutes_core::search::SearchFilters {
        content_type: None,
        since: None,
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
    };
    let latest = minutes_core::search::search("", &config, &filters)
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
        .ok_or_else(|| "No saved meetings or memos yet.".to_string())?;
    Ok(latest.path)
}

fn extract_paste_text(content: &str, kind: &str) -> Result<String, String> {
    let (_, body) = minutes_core::markdown::split_frontmatter(content);
    let sections = parse_sections(body);
    let target_heading = match kind {
        "summary" => "Summary",
        "transcript" => "Transcript",
        other => {
            return Err(format!(
                "Unsupported paste payload: {}. Use 'summary' or 'transcript'.",
                other
            ));
        }
    };

    sections
        .into_iter()
        .find(|section| section.heading.eq_ignore_ascii_case(target_heading))
        .map(|section| section.content.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("The latest artifact does not contain a {} section.", kind))
}

pub(crate) fn copy_to_clipboard(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;

        let mut child = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not start pbcopy: {}", e))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| format!("Could not write to clipboard: {}", e))?;
        }

        let status = child
            .wait()
            .map_err(|e| format!("Could not finish clipboard write: {}", e))?;
        if status.success() {
            Ok(())
        } else {
            Err("pbcopy failed to update the clipboard.".into())
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        Err("Tray copy/paste automation is currently available on macOS only.".into())
    }
}

fn paste_into_application(app_name: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            r#"tell application "{}" to activate
delay 0.15
tell application "System Events" to keystroke "v" using command down"#,
            escape_applescript_literal(app_name)
        );

        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|e| format!("Could not run paste automation: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!(
                "Paste automation failed{}. Minutes already copied the text to your clipboard.",
                if stderr.trim().is_empty() {
                    ".".to_string()
                } else {
                    format!(" ({})", stderr.trim())
                }
            ))
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app_name;
        Err("Tray paste automation is currently available on macOS only.".into())
    }
}

pub fn paste_latest_artifact(
    latest_output: &Arc<Mutex<Option<OutputNotice>>>,
    kind: &str,
    target_app: Option<&str>,
) -> Result<String, String> {
    let path = latest_saved_artifact_path(latest_output)?;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Could not read latest artifact {}: {}", path.display(), e))?;
    let payload = extract_paste_text(&content, kind)?;
    copy_to_clipboard(&payload)?;

    if let Some(app_name) = target_app.filter(|name| !name.trim().is_empty()) {
        paste_into_application(app_name)?;
        Ok(format!(
            "Copied the latest {} and pasted it into {}.",
            kind, app_name
        ))
    } else {
        Ok(format!(
            "Copied the latest {} to the clipboard. Switch to your app and paste.",
            kind
        ))
    }
}

fn parse_sections(body: &str) -> Vec<MeetingSection> {
    let mut sections = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_lines: Vec<String> = Vec::new();

    for line in body.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(existing_heading) = current_heading.take() {
                sections.push(MeetingSection {
                    heading: existing_heading,
                    content: current_lines.join("\n").trim().to_string(),
                });
            }
            current_heading = Some(heading.trim().to_string());
            current_lines.clear();
        } else if current_heading.is_some() {
            current_lines.push(line.to_string());
        }
    }

    if let Some(existing_heading) = current_heading.take() {
        sections.push(MeetingSection {
            heading: existing_heading,
            content: current_lines.join("\n").trim().to_string(),
        });
    }

    sections
}

fn model_status(config: &Config) -> ReadinessItem {
    let model_name = &config.transcription.model;
    let model_file = config
        .transcription
        .model_path
        .join(format!("ggml-{}.bin", model_name));
    let exists = model_file.exists();

    ReadinessItem {
        label: "Speech model".into(),
        state: if exists { "ready" } else { "attention" }.into(),
        detail: if exists {
            format!("{} is installed at {}.", model_name, model_file.display())
        } else {
            format!(
                "{} is not installed yet. Download it before recording.",
                model_name
            )
        },
        optional: false,
    }
}

fn microphone_status() -> ReadinessItem {
    let devices = minutes_core::capture::list_input_devices();
    let has_devices = !devices.is_empty();

    ReadinessItem {
        label: "Microphone & audio input".into(),
        state: if has_devices { "ready" } else { "attention" }.into(),
        detail: if has_devices {
            format!(
                "{} audio input device{} detected. Minutes may still prompt for microphone access the first time you record.",
                devices.len(),
                if devices.len() == 1 { "" } else { "s" }
            )
        } else {
            "No audio input devices detected. Check hardware and system audio settings.".into()
        },
        optional: false,
    }
}

fn call_capture_status() -> ReadinessItem {
    match call_capture::availability() {
        call_capture::CallCaptureAvailability::Available { backend } => ReadinessItem {
            label: "Call capture".into(),
            state: "ready".into(),
            detail: format!(
                "Native call capture is available via {}. Screen Recording permission will be requested when capture actually starts if macOS still needs it.",
                backend
            ),
            optional: true,
        },
        call_capture::CallCaptureAvailability::PermissionRequired { detail } => ReadinessItem {
            label: "Call capture".into(),
            state: "attention".into(),
            detail,
            optional: true,
        },
        call_capture::CallCaptureAvailability::Unavailable { detail } => ReadinessItem {
            label: "Call capture".into(),
            state: "attention".into(),
            detail,
            optional: true,
        },
        call_capture::CallCaptureAvailability::Unsupported { detail } => ReadinessItem {
            label: "Call capture".into(),
            state: "unsupported".into(),
            detail,
            optional: true,
        },
    }
}

fn calendar_status() -> ReadinessItem {
    #[cfg(not(target_os = "macos"))]
    {
        return ReadinessItem {
            label: "Calendar suggestions".into(),
            state: "unsupported".into(),
            detail: "Calendar suggestions are currently available on macOS only.".into(),
            optional: true,
        };
    }

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "Calendar" to get name of every calendar"#)
        .output();

    match output {
        Ok(result) if result.status.success() => ReadinessItem {
            label: "Calendar suggestions".into(),
            state: "ready".into(),
            detail: "Calendar access is available for upcoming-meeting suggestions.".into(),
            optional: true,
        },
        Ok(result) => {
            let stderr = String::from_utf8_lossy(&result.stderr);
            ReadinessItem {
                label: "Calendar suggestions".into(),
                state: "attention".into(),
                detail: if stderr.trim().is_empty() {
                    "Calendar access is unavailable right now. Suggestions will stay hidden until access is granted.".into()
                } else {
                    format!(
                        "Calendar access is unavailable right now ({}). Suggestions will stay hidden until access is granted.",
                        stderr.trim()
                    )
                },
                optional: true,
            }
        }
        Err(e) => ReadinessItem {
            label: "Calendar suggestions".into(),
            state: "attention".into(),
            detail: format!(
                "Calendar checks are unavailable right now ({}). Suggestions will stay hidden.",
                e
            ),
            optional: true,
        },
    }
}

fn watcher_status(config: &Config) -> ReadinessItem {
    let existing = config
        .watch
        .paths
        .iter()
        .filter(|path| path.exists())
        .count();
    let total = config.watch.paths.len();
    let state = if total > 0 && existing == total {
        "ready"
    } else {
        "attention"
    };

    let detail = if total == 0 {
        "No watch folders configured. Voice-memo ingestion is available but not set up.".into()
    } else if existing == total {
        format!(
            "{} watch folder{} ready for inbox processing.",
            total,
            if total == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "{} of {} watch folders currently exist. Missing folders will prevent automatic inbox processing.",
            existing, total
        )
    };

    ReadinessItem {
        label: "Watcher folders".into(),
        state: state.into(),
        detail,
        optional: true,
    }
}

fn output_dir_status(config: &Config) -> ReadinessItem {
    let exists = config.output_dir.exists();
    ReadinessItem {
        label: "Meeting output folder".into(),
        state: if exists { "ready" } else { "attention" }.into(),
        detail: if exists {
            format!(
                "Meeting markdown is stored in {}.",
                config.output_dir.display()
            )
        } else {
            format!(
                "Output folder {} does not exist yet. Minutes will create it on demand.",
                config.output_dir.display()
            )
        },
        optional: false,
    }
}

fn vault_status(config: &Config) -> ReadinessItem {
    use minutes_core::vault;
    match vault::check_health(config) {
        vault::VaultStatus::NotConfigured => ReadinessItem {
            label: "Vault sync (Obsidian / Logseq)".into(),
            state: "attention".into(),
            detail: "Not configured. Use Settings > Set Up Vault to connect your vault.".into(),
            optional: true,
        },
        vault::VaultStatus::Healthy { strategy, path } => ReadinessItem {
            label: "Vault sync (Obsidian / Logseq)".into(),
            state: "ready".into(),
            detail: format!("Strategy: {}. Path: {}.", strategy, path.display()),
            optional: true,
        },
        vault::VaultStatus::BrokenSymlink { link_path, target } => ReadinessItem {
            label: "Vault sync (Obsidian / Logseq)".into(),
            state: "attention".into(),
            detail: format!(
                "Broken symlink at {} → {}. Re-run vault setup.",
                link_path.display(),
                target.display()
            ),
            optional: true,
        },
        vault::VaultStatus::PermissionDenied { path } => ReadinessItem {
            label: "Vault sync (Obsidian / Logseq)".into(),
            state: "attention".into(),
            detail: format!(
                "Permission denied: {}. Try Set Up Vault from the app.",
                path.display()
            ),
            optional: true,
        },
        vault::VaultStatus::MissingVaultDir { path } => ReadinessItem {
            label: "Vault sync (Obsidian / Logseq)".into(),
            state: "attention".into(),
            detail: format!("Vault directory missing: {}.", path.display()),
            optional: true,
        },
    }
}

// ── Vault Tauri commands ─────────────────────────────────────

#[tauri::command]
pub fn cmd_vault_status() -> serde_json::Value {
    let config = Config::load();
    let health = minutes_core::vault::check_health(&config);
    let (status, strategy, path, detail) = match health {
        minutes_core::vault::VaultStatus::NotConfigured => (
            "not_configured",
            "".into(),
            "".into(),
            "Not configured".into(),
        ),
        minutes_core::vault::VaultStatus::Healthy { strategy, path } => {
            let p = path.display().to_string();
            (
                "healthy",
                strategy,
                p.clone(),
                format!("Vault active at {}", p),
            )
        }
        minutes_core::vault::VaultStatus::BrokenSymlink { link_path, target } => (
            "broken",
            "symlink".into(),
            link_path.display().to_string(),
            format!("Broken symlink → {}", target.display()),
        ),
        minutes_core::vault::VaultStatus::PermissionDenied { path } => (
            "permission_denied",
            "".into(),
            path.display().to_string(),
            "Permission denied".into(),
        ),
        minutes_core::vault::VaultStatus::MissingVaultDir { path } => (
            "missing",
            "".into(),
            path.display().to_string(),
            "Vault directory missing".into(),
        ),
    };
    serde_json::json!({
        "status": status,
        "strategy": strategy,
        "path": path,
        "detail": detail,
        "enabled": config.vault.enabled,
    })
}

#[tauri::command]
pub fn cmd_vault_setup(path: String) -> Result<serde_json::Value, String> {
    let vault_path = std::path::PathBuf::from(&path);
    if !vault_path.exists() {
        return Err(format!("Path does not exist: {}", path));
    }

    let mut config = Config::load();
    let strategy = minutes_core::vault::recommend_strategy(&vault_path);

    // For symlink strategy, try to create the symlink
    if strategy == minutes_core::vault::VaultStrategy::Symlink {
        let link_path = vault_path.join(&config.vault.meetings_subdir);
        if let Err(e) = minutes_core::vault::create_symlink(&link_path, &config.output_dir) {
            // Fall back to copy if symlink fails
            eprintln!("[vault] symlink failed ({}), falling back to copy", e);
            config.vault.strategy = "copy".into();
        } else {
            config.vault.strategy = "symlink".into();
        }
    } else {
        config.vault.strategy = strategy.to_string();
    }

    config.vault.enabled = true;
    config.vault.path = vault_path;

    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;

    let health = minutes_core::vault::check_health(&config);
    let status = match health {
        minutes_core::vault::VaultStatus::Healthy { strategy, path } => {
            format!("Vault configured ({}): {}", strategy, path.display())
        }
        _ => "Vault configured but health check shows issues. Check Readiness Center.".into(),
    };

    Ok(serde_json::json!({
        "status": "ok",
        "strategy": config.vault.strategy,
        "detail": status,
    }))
}

#[tauri::command]
pub fn cmd_vault_unlink() -> Result<String, String> {
    let mut config = Config::load();
    if !config.vault.enabled {
        return Ok("Vault is not configured.".into());
    }
    let old = config.vault.path.display().to_string();
    config.vault.enabled = false;
    config.vault.path = std::path::PathBuf::new();
    config.vault.strategy = "auto".into();
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;
    Ok(format!("Vault unlinked (was: {})", old))
}

fn is_hidden_or_system_file(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with('.'))
        .unwrap_or(false)
}

fn recovery_title(path: &std::path::Path, fallback: &str) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.replace('-', " "))
        .map(|stem| stem.trim().to_string())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn scan_recovery_items(config: &Config) -> Vec<RecoveryItem> {
    let mut found: Vec<(SystemTime, RecoveryItem)> = Vec::new();

    let current_wav = minutes_core::pid::current_wav_path();
    if current_wav.exists() && !minutes_core::pid::status().recording {
        if let Ok(metadata) = current_wav.metadata() {
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            found.push((
                modified,
                RecoveryItem {
                    kind: "stale-recording".into(),
                    title: "Unprocessed live recording".into(),
                    path: current_wav.display().to_string(),
                    detail: "Minutes found an unfinished live capture that never made it through the pipeline.".into(),
                    retry_type: "meeting".into(),
                },
            ));
        }
    }

    let failed_captures = config.output_dir.join("failed-captures");
    if let Ok(entries) = std::fs::read_dir(&failed_captures) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && !is_hidden_or_system_file(&path) {
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                found.push((
                    modified,
                    RecoveryItem {
                        kind: "preserved-capture".into(),
                        title: recovery_title(&path, "Preserved capture"),
                        path: path.display().to_string(),
                        detail:
                            "A live recording was preserved because capture or processing failed."
                                .into(),
                        retry_type: "meeting".into(),
                    },
                ));
            }
        }
    }

    for watch_path in &config.watch.paths {
        let failed_dir = watch_path.join("failed");
        if let Ok(entries) = std::fs::read_dir(&failed_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && !is_hidden_or_system_file(&path) {
                    let modified = entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    found.push((
                        modified,
                        RecoveryItem {
                            kind: "watch-failed".into(),
                            title: recovery_title(&path, "Failed watched file"),
                            path: path.display().to_string(),
                            detail: "A watched audio file failed to process and is waiting for manual retry.".into(),
                            retry_type: config.watch.r#type.clone(),
                        },
                    ));
                }
            }
        }
    }

    found.sort_by_key(|(modified, _)| Reverse(*modified));
    found.into_iter().map(|(_, item)| item).collect()
}

/// Start recording in a background thread.
#[allow(clippy::too_many_arguments)]
pub fn start_recording(
    app_handle: tauri::AppHandle,
    recording: Arc<AtomicBool>,
    starting: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    processing: Arc<AtomicBool>,
    processing_stage: Arc<Mutex<Option<String>>>,
    latest_output: Arc<Mutex<Option<OutputNotice>>>,
    call_capture_health: Arc<Mutex<Option<crate::call_capture::CallSourceHealth>>>,
    completion_notifications_enabled: Arc<AtomicBool>,
    hotkey_runtime: Option<Arc<Mutex<HotkeyRuntime>>>,
    discard_short_hotkey_capture: Option<Arc<AtomicBool>>,
    mode: CaptureMode,
    requested_intent: Option<RecordingIntent>,
    allow_degraded: bool,
    requested_title: Option<String>,
    language_override: Option<String>,
) {
    let mut config = Config::load();
    if let Some(language) = language_override {
        config.transcription.language = Some(language);
    }
    let preflight = match minutes_core::capture::preflight_recording(
        mode,
        requested_intent,
        allow_degraded,
        &config,
    ) {
        Ok(preflight) => preflight,
        Err(error) => {
            eprintln!("Recording preflight failed: {}", error);
            show_user_notification(&app_handle, "Recording blocked", &error);
            starting.store(false, Ordering::Relaxed);
            recording.store(false, Ordering::Relaxed);
            reset_hotkey_capture_state(
                hotkey_runtime.as_ref(),
                discard_short_hotkey_capture.as_ref(),
            );
            return;
        }
    };
    let native_call_capture_available = preflight.intent == RecordingIntent::Call
        && matches!(
            call_capture::availability(),
            call_capture::CallCaptureAvailability::Available { .. }
        );
    if let Some(reason) = &preflight.blocking_reason {
        if !(preflight.intent == RecordingIntent::Call && native_call_capture_available) {
            eprintln!("Recording preflight blocked: {}", reason);
            show_user_notification(&app_handle, "Recording blocked", reason);
            starting.store(false, Ordering::Relaxed);
            recording.store(false, Ordering::Relaxed);
            reset_hotkey_capture_state(
                hotkey_runtime.as_ref(),
                discard_short_hotkey_capture.as_ref(),
            );
            return;
        }
    }
    for warning in &preflight.warnings {
        eprintln!("[minutes] {}", warning);
    }

    #[cfg(target_os = "macos")]
    if preflight.intent == RecordingIntent::Call && native_call_capture_available {
        match start_native_call_recording(
            &app_handle,
            &recording,
            &starting,
            &stop_flag,
            &processing,
            &processing_stage,
            &latest_output,
            &call_capture_health,
            &completion_notifications_enabled,
            hotkey_runtime.as_ref(),
            discard_short_hotkey_capture.as_ref(),
            mode,
            &config,
            requested_title.clone(),
        ) {
            Ok(()) => {
                return;
            }
            Err(error) => {
                eprintln!("Native call recording unavailable, falling back: {}", error);
                if let Some(reason) = &preflight.blocking_reason {
                    show_user_notification(
                        &app_handle,
                        "Recording blocked",
                        &format!("{}\n\nNative call capture failed: {}", reason, error),
                    );
                    starting.store(false, Ordering::Relaxed);
                    recording.store(false, Ordering::Relaxed);
                    reset_hotkey_capture_state(
                        hotkey_runtime.as_ref(),
                        discard_short_hotkey_capture.as_ref(),
                    );
                    return;
                }
            }
        }
    }

    let wav_path = minutes_core::pid::current_wav_path();
    let recording_started_at = chrono::Local::now();

    if let Err(e) = minutes_core::pid::create() {
        eprintln!("Failed to create PID: {}", e);
        show_user_notification(
            &app_handle,
            "Recording",
            &format!("Could not start recording: {}", e),
        );
        starting.store(false, Ordering::Relaxed);
        recording.store(false, Ordering::Relaxed);
        reset_hotkey_capture_state(
            hotkey_runtime.as_ref(),
            discard_short_hotkey_capture.as_ref(),
        );
        return;
    }
    starting.store(false, Ordering::Relaxed);
    recording.store(true, Ordering::Relaxed);
    stop_flag.store(false, Ordering::Relaxed);
    sync_processing_indicator(&processing, &processing_stage);
    set_latest_output(&latest_output, None);
    minutes_core::pid::write_recording_metadata(mode).ok();
    crate::update_tray_state(&app_handle, true);

    minutes_core::notes::save_recording_start().ok();
    eprintln!("{} started...", mode.noun());

    // Inject live transcript context into the assistant workspace so the Recall
    // panel (and any connected agent) can read the live JSONL during recording.
    if let Ok(workspace) = crate::context::create_workspace(&config) {
        update_assistant_live_context(&workspace, true);
    }

    let mut clear_processing_on_exit = true;
    match minutes_core::capture::record_to_wav(&wav_path, stop_flag, &config) {
        Ok(()) => {
            recording.store(false, Ordering::Relaxed);
            let should_discard = discard_short_hotkey_capture
                .as_ref()
                .map(|flag| flag.swap(false, Ordering::Relaxed))
                .unwrap_or(false);
            if should_discard {
                if wav_path.exists() {
                    std::fs::remove_file(&wav_path).ok();
                }
                eprintln!("Discarded short {} capture.", mode.noun());
            } else {
                let recording_finished_at = chrono::Local::now();
                let user_notes = minutes_core::notes::read_notes();
                let pre_context = minutes_core::notes::read_context();
                // Don't block the stop path with a calendar query (can take 10s if Calendar.app hangs).
                // The pipeline already falls back to events_overlapping_now() during background processing.
                let calendar_event = None;

                match minutes_core::jobs::queue_live_capture(
                    mode,
                    requested_title.clone(),
                    &wav_path,
                    user_notes,
                    pre_context,
                    Some(recording_started_at),
                    Some(recording_finished_at),
                    calendar_event,
                ) {
                    Ok(job) => {
                        processing.store(true, Ordering::Relaxed);
                        set_processing_stage(&processing_stage, job.stage.as_deref());
                        minutes_core::pid::set_processing_status(
                            job.stage.as_deref(),
                            Some(mode),
                            job.title.as_deref(),
                            Some(&job.id),
                            minutes_core::jobs::active_job_count(),
                        )
                        .ok();
                        minutes_core::pid::remove().ok();
                        minutes_core::pid::clear_recording_metadata().ok();
                        minutes_core::notes::cleanup();
                        clear_processing_on_exit = false;
                        spawn_processing_worker(
                            app_handle.clone(),
                            processing.clone(),
                            processing_stage.clone(),
                            latest_output.clone(),
                            completion_notifications_enabled.clone(),
                        );
                    }
                    Err(e) => {
                        if let Some(saved) = preserve_failed_capture(&wav_path, &config) {
                            let notice = OutputNotice {
                                kind: "preserved-capture".into(),
                                title: "Raw capture preserved".into(),
                                path: saved.display().to_string(),
                                detail: format!(
                                    "Failed to queue background processing. Raw {} capture preserved.",
                                    mode.noun()
                                ),
                            };
                            set_latest_output(&latest_output, Some(notice.clone()));
                            maybe_show_completion_notification(
                                &app_handle,
                                &completion_notifications_enabled,
                                &notice,
                            );
                            eprintln!(
                                "Queue error: {}. Raw audio preserved at {}",
                                e,
                                saved.display()
                            );
                        } else {
                            eprintln!("Queue error: {}", e);
                        }
                    }
                }
            }
        }
        Err(e) => {
            recording.store(false, Ordering::Relaxed);
            if let Some(saved) = preserve_failed_capture(&wav_path, &config) {
                let detail = match mode {
                    CaptureMode::Meeting => {
                        "Recording failed before processing, but the captured meeting audio was preserved."
                    }
                    CaptureMode::QuickThought => {
                        "Recording failed before processing, but the quick thought audio was preserved."
                    }
                    CaptureMode::Dictation => {
                        "Dictation failed, but the audio was preserved."
                    }
                    CaptureMode::LiveTranscript => {
                        "Live transcript failed, but the audio was preserved."
                    }
                };
                let notice = OutputNotice {
                    kind: "preserved-capture".into(),
                    title: "Partial capture preserved".into(),
                    path: saved.display().to_string(),
                    detail: detail.into(),
                };
                set_latest_output(&latest_output, Some(notice.clone()));
                maybe_show_completion_notification(
                    &app_handle,
                    &completion_notifications_enabled,
                    &notice,
                );
                eprintln!(
                    "Capture error: {}. Partial audio preserved at {}",
                    e,
                    saved.display()
                );
            } else {
                eprintln!("Capture error: {}", e);
            }
        }
    }

    // Remove live transcript context from assistant workspace
    if let Ok(workspace) = crate::context::create_workspace(&config) {
        update_assistant_live_context(&workspace, false);
    }

    if clear_processing_on_exit {
        minutes_core::notes::cleanup();
        minutes_core::pid::remove().ok();
        processing.store(false, Ordering::Relaxed);
        set_processing_stage(&processing_stage, None);
        minutes_core::pid::clear_processing_status().ok();
        minutes_core::pid::clear_recording_metadata().ok();
    } else {
        sync_processing_indicator(&processing, &processing_stage);
    }
    starting.store(false, Ordering::Relaxed);
    recording.store(false, Ordering::Relaxed);
    reset_hotkey_capture_state(
        hotkey_runtime.as_ref(),
        discard_short_hotkey_capture.as_ref(),
    );
}

#[allow(clippy::too_many_arguments)]
pub fn launch_recording(
    app: tauri::AppHandle,
    state: &AppState,
    mode: CaptureMode,
    requested_intent: Option<RecordingIntent>,
    allow_degraded: bool,
    requested_title: Option<String>,
    language_override: Option<String>,
    hotkey_runtime: Option<Arc<Mutex<HotkeyRuntime>>>,
    discard_short_hotkey_capture: Option<Arc<AtomicBool>>,
) -> Result<(), String> {
    if recording_active(&state.recording) || state.starting.load(Ordering::Relaxed) {
        return Err("Already recording".into());
    }
    if state.live_transcript_active.load(Ordering::Relaxed) {
        return Err("Live transcript in progress — stop it first".into());
    }

    state.starting.store(true, Ordering::Relaxed);
    let rec = state.recording.clone();
    let starting = state.starting.clone();
    let stop = state.stop_flag.clone();
    let processing = state.processing.clone();
    let processing_stage = state.processing_stage.clone();
    let latest_output = state.latest_output.clone();
    let call_capture_health = state.call_capture_health.clone();
    let completion_notifications_enabled = state.completion_notifications_enabled.clone();
    let app_done = app.clone();

    std::thread::spawn(move || {
        start_recording(
            app,
            rec,
            starting,
            stop,
            processing,
            processing_stage,
            latest_output,
            call_capture_health,
            completion_notifications_enabled,
            hotkey_runtime,
            discard_short_hotkey_capture,
            mode,
            requested_intent,
            allow_degraded,
            requested_title,
            language_override,
        );
        crate::update_tray_state(&app_done, false);
    });

    Ok(())
}

pub fn handle_desktop_control_request(
    app: tauri::AppHandle,
    state: &AppState,
    request: minutes_core::desktop_control::DesktopControlRequest,
) -> minutes_core::desktop_control::DesktopControlResponse {
    fn activation_detail(state: &AppState) -> String {
        state
            .latest_output
            .lock()
            .ok()
            .and_then(|notice| notice.clone())
            .map(|notice| notice.detail)
            .filter(|detail| !detail.trim().is_empty())
            .unwrap_or_else(|| {
                "Minutes desktop app did not confirm that recording became active.".into()
            })
    }

    let detail = match request.action {
        minutes_core::desktop_control::DesktopControlAction::StartRecording(payload) => {
            match launch_recording(
                app,
                state,
                payload.mode,
                payload.intent,
                payload.allow_degraded,
                payload.title,
                payload.language,
                None,
                None,
            ) {
                Ok(()) => {
                    let start = Instant::now();
                    while start.elapsed() < Duration::from_secs(12) {
                        if recording_active(&state.recording) {
                            return minutes_core::desktop_control::DesktopControlResponse {
                                id: request.id,
                                handled_at: chrono::Local::now(),
                                accepted: true,
                                detail:
                                    "Recording request accepted by the running Minutes desktop app."
                                        .into(),
                            };
                        }
                        if !state.starting.load(Ordering::Relaxed) {
                            return minutes_core::desktop_control::DesktopControlResponse {
                                id: request.id,
                                handled_at: chrono::Local::now(),
                                accepted: false,
                                detail: activation_detail(state),
                            };
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    return minutes_core::desktop_control::DesktopControlResponse {
                        id: request.id,
                        handled_at: chrono::Local::now(),
                        accepted: false,
                        detail: activation_detail(state),
                    };
                }
                Err(error) => error,
            }
        }
    };

    minutes_core::desktop_control::DesktopControlResponse {
        id: request.id,
        handled_at: chrono::Local::now(),
        accepted: false,
        detail,
    }
}

fn spawn_hotkey_recording(app: &tauri::AppHandle, style: HotkeyCaptureStyle) {
    let state = app.state::<AppState>();
    if let Ok(mut runtime) = state.hotkey_runtime.lock() {
        runtime.active_capture = Some(style);
        runtime.recording_started_at = Some(Instant::now());
    }
    state
        .discard_short_hotkey_capture
        .store(false, Ordering::Relaxed);
    let hotkey_runtime = state.hotkey_runtime.clone();
    let discard_short_hotkey_capture = state.discard_short_hotkey_capture.clone();
    let _ = launch_recording(
        app.clone(),
        &state,
        CaptureMode::QuickThought,
        Some(RecordingIntent::Memo),
        false,
        None,
        None,
        Some(hotkey_runtime),
        Some(discard_short_hotkey_capture),
    );
}

pub fn handle_global_hotkey_event(
    app: &tauri::AppHandle,
    shortcut_state: tauri_plugin_global_shortcut::ShortcutState,
) {
    let state = app.state::<AppState>();
    if !state.global_hotkey_enabled.load(Ordering::Relaxed) {
        return;
    }

    match shortcut_state {
        tauri_plugin_global_shortcut::ShortcutState::Pressed => {
            let generation = {
                let mut runtime = match state.hotkey_runtime.lock() {
                    Ok(runtime) => runtime,
                    Err(_) => return,
                };
                if runtime.key_down {
                    return;
                }
                runtime.key_down = true;
                runtime.key_down_started_at = Some(Instant::now());
                runtime.hold_generation = runtime.hold_generation.wrapping_add(1);
                runtime.hold_generation
            };

            let recording = state.recording.clone();
            let processing = state.processing.clone();
            let runtime = state.hotkey_runtime.clone();
            let app_handle = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(HOTKEY_HOLD_THRESHOLD_MS));
                let should_start_hold = {
                    let runtime = match runtime.lock() {
                        Ok(runtime) => runtime,
                        Err(_) => return,
                    };
                    runtime.key_down
                        && runtime.hold_generation == generation
                        && runtime.active_capture.is_none()
                        && !recording.load(Ordering::Relaxed)
                        && !processing.load(Ordering::Relaxed)
                        && !minutes_core::pid::status().recording
                };
                if should_start_hold {
                    spawn_hotkey_recording(&app_handle, HotkeyCaptureStyle::Hold);
                }
            });
        }
        tauri_plugin_global_shortcut::ShortcutState::Released => {
            let now = Instant::now();
            let (active_capture, recording_started_at, was_short_tap) = {
                let mut runtime = match state.hotkey_runtime.lock() {
                    Ok(runtime) => runtime,
                    Err(_) => return,
                };
                let pressed_at = runtime.key_down_started_at;
                runtime.key_down = false;
                runtime.key_down_started_at = None;
                let was_short_tap = pressed_at
                    .map(|pressed| {
                        now.duration_since(pressed).as_millis() < HOTKEY_HOLD_THRESHOLD_MS as u128
                    })
                    .unwrap_or(false);
                (
                    runtime.active_capture,
                    runtime.recording_started_at,
                    was_short_tap,
                )
            };

            if let Some(_style) = active_capture {
                if should_discard_hotkey_capture(recording_started_at, now) {
                    state
                        .discard_short_hotkey_capture
                        .store(true, Ordering::Relaxed);
                }
                if let Ok(mut runtime) = state.hotkey_runtime.lock() {
                    runtime.active_capture = None;
                    runtime.recording_started_at = None;
                }
                if let Err(err) = request_stop(&state.recording, &state.stop_flag) {
                    show_user_notification(
                        app,
                        "Quick thought",
                        &format!("Could not stop recording: {}", err),
                    );
                }
                return;
            }

            if !was_short_tap {
                return;
            }

            if recording_active(&state.recording) {
                if let Err(err) = request_stop(&state.recording, &state.stop_flag) {
                    show_user_notification(
                        app,
                        "Quick thought",
                        &format!("Could not stop recording: {}", err),
                    );
                }
                return;
            }

            spawn_hotkey_recording(app, HotkeyCaptureStyle::Locked);
        }
    }
}

pub fn handle_dictation_shortcut_event(
    app: &tauri::AppHandle,
    shortcut_state: tauri_plugin_global_shortcut::ShortcutState,
) {
    let state = app.state::<AppState>();
    if !state.dictation_shortcut_enabled.load(Ordering::Relaxed) {
        return;
    }

    if shortcut_state != tauri_plugin_global_shortcut::ShortcutState::Pressed {
        return;
    }

    let shortcut = state
        .dictation_shortcut
        .lock()
        .ok()
        .map(|value| value.clone())
        .unwrap_or_else(|| default_dictation_shortcut().to_string());
    minutes_core::logging::append_log(&serde_json::json!({
        "ts": chrono::Local::now().to_rfc3339(),
        "level": "info",
        "step": "dictation_shortcut_event",
        "file": "",
        "extra": {
            "shortcut": shortcut,
            "state": "pressed",
        }
    }))
    .ok();

    if state.dictation_active.load(Ordering::Relaxed) {
        minutes_core::logging::append_log(&serde_json::json!({
            "ts": chrono::Local::now().to_rfc3339(),
            "level": "info",
            "step": "dictation_shortcut_action",
            "file": "",
            "extra": {
                "shortcut": shortcut,
                "action": "stop",
            }
        }))
        .ok();
        state.dictation_stop_flag.store(true, Ordering::Relaxed);
        return;
    }

    if let Err(error) = start_dictation_session(app, None) {
        minutes_core::logging::append_log(&serde_json::json!({
            "ts": chrono::Local::now().to_rfc3339(),
            "level": "error",
            "step": "dictation_shortcut_action",
            "file": "",
            "error": error,
            "extra": {
                "shortcut": shortcut,
                "action": "start_failed",
            }
        }))
        .ok();
        show_user_notification(app, "Dictation", &error);
    } else {
        minutes_core::logging::append_log(&serde_json::json!({
            "ts": chrono::Local::now().to_rfc3339(),
            "level": "info",
            "step": "dictation_shortcut_action",
            "file": "",
            "extra": {
                "shortcut": shortcut,
                "action": "start",
            }
        }))
        .ok();
    }
}

#[tauri::command]
pub fn cmd_start_recording(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    mode: Option<String>,
    intent: Option<String>,
    allow_degraded: Option<bool>,
    title: Option<String>,
    language: Option<String>,
) -> Result<(), String> {
    let capture_mode = parse_capture_mode(mode.as_deref())?;
    let requested_intent = parse_recording_intent(intent.as_deref())?;
    launch_recording(
        app,
        &state,
        capture_mode,
        requested_intent,
        allow_degraded.unwrap_or(false),
        title,
        language,
        None,
        None,
    )
}

#[tauri::command]
pub fn cmd_stop_recording(state: tauri::State<AppState>) -> Result<(), String> {
    request_stop(&state.recording, &state.stop_flag)
}

#[tauri::command]
pub fn cmd_extend_recording() -> Result<(), String> {
    minutes_core::capture::write_extend_sentinel().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cmd_add_note(text: String) -> Result<String, String> {
    minutes_core::notes::add_note(&text)
}

#[tauri::command]
pub fn cmd_status(state: tauri::State<AppState>) -> serde_json::Value {
    let recording = state.recording.load(Ordering::Relaxed);
    let shared_processing = minutes_core::pid::read_processing_status();
    let processing = state.processing.load(Ordering::Relaxed) || shared_processing.processing;
    let status = minutes_core::pid::status();
    let processing_stage = state
        .processing_stage
        .lock()
        .ok()
        .and_then(|stage| stage.clone())
        .or(shared_processing.stage);
    let latest_output = state
        .latest_output
        .lock()
        .ok()
        .and_then(|notice| notice.clone());
    let call_capture_health = state
        .call_capture_health
        .lock()
        .ok()
        .and_then(|health| health.clone());
    let processing_jobs: Vec<ProcessingJobView> = minutes_core::jobs::active_jobs()
        .into_iter()
        .map(processing_job_view)
        .collect();

    // Get elapsed time if recording
    let elapsed = if recording || (status.recording && !processing) {
        let start_path = minutes_core::notes::recording_start_path();
        if start_path.exists() {
            if let Ok(s) = std::fs::read_to_string(&start_path) {
                if let Ok(start) = s.trim().parse::<u64>() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let e = now.saturating_sub(start);
                    Some(format!("{}:{:02}", e / 60, e % 60))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let audio_level = if recording || (status.recording && !processing) {
        minutes_core::capture::audio_level()
    } else {
        0
    };

    serde_json::json!({
        "recording": recording || (status.recording && !processing),
        "processing": processing,
        "recordingMode": status.recording_mode,
        "processingStage": processing_stage,
        "processingTitle": status.processing_title,
        "processingJobId": status.processing_job_id,
        "processingJobCount": status.processing_job_count,
        "processingJobs": processing_jobs,
        "latestOutput": latest_output,
        "callCaptureHealth": call_capture_health,
        "pid": status.pid,
        "elapsed": elapsed,
        "audioLevel": audio_level,
    })
}

#[tauri::command]
pub fn cmd_processing_jobs(limit: Option<usize>) -> serde_json::Value {
    let jobs: Vec<ProcessingJobView> = minutes_core::jobs::display_jobs(limit, true)
        .into_iter()
        .map(processing_job_view)
        .collect();
    serde_json::to_value(jobs).unwrap_or(serde_json::json!([]))
}

#[tauri::command]
pub fn cmd_retry_processing_job(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    job_id: String,
) -> Result<(), String> {
    let queued = minutes_core::jobs::requeue_job(&job_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Processing job not found: {}", job_id))?;

    minutes_core::pid::set_processing_status(
        queued.stage.as_deref(),
        Some(queued.mode),
        queued.title.as_deref(),
        Some(&queued.id),
        minutes_core::jobs::active_job_count(),
    )
    .ok();
    sync_processing_indicator(&state.processing, &state.processing_stage);
    spawn_processing_worker(
        app,
        state.processing.clone(),
        state.processing_stage.clone(),
        state.latest_output.clone(),
        state.completion_notifications_enabled.clone(),
    );
    Ok(())
}

/// Scan ~/.minutes/preps/ for existing prep files and return a set of
/// first-name slugs that have been prepped (for lifecycle badge display).
fn scan_prep_slugs() -> std::collections::HashSet<String> {
    let preps_dir = Config::minutes_dir().join("preps");
    let mut slugs = std::collections::HashSet::new();
    if let Ok(entries) = std::fs::read_dir(&preps_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".prep.md") {
                // slug format: YYYY-MM-DD-{name}.prep.md → extract {name}
                if let Some(stem) = name.strip_suffix(".prep.md") {
                    // skip date prefix (11 chars: "YYYY-MM-DD-")
                    if stem.len() > 11 {
                        slugs.insert(stem[11..].to_lowercase());
                    }
                }
            }
        }
    }
    slugs
}

/// Check if a meeting's attendees include anyone with a matching prep file.
fn meeting_has_prep(attendees: &[String], prep_slugs: &std::collections::HashSet<String>) -> bool {
    attendees.iter().any(|name| {
        let first = name.split_whitespace().next().unwrap_or(name);
        prep_slugs.contains(&first.to_lowercase())
    })
}

#[tauri::command]
pub fn cmd_list_meetings(limit: Option<usize>) -> serde_json::Value {
    let config = Config::load();
    let prep_slugs = scan_prep_slugs();
    let filters = minutes_core::search::SearchFilters {
        content_type: None,
        since: None,
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
    };
    match minutes_core::search::search("", &config, &filters) {
        Ok(results) => {
            let limited: Vec<_> = results.into_iter().take(limit.unwrap_or(20)).collect();
            let enriched: Vec<serde_json::Value> = limited
                .iter()
                .map(|r| {
                    let mut val = serde_json::to_value(r).unwrap_or(serde_json::json!({}));
                    // Read frontmatter to check for lifecycle badges
                    let badges = compute_lifecycle_badges(&r.path, &prep_slugs);
                    val["badges"] = serde_json::json!(badges);
                    val
                })
                .collect();
            serde_json::json!(enriched)
        }
        Err(_) => serde_json::json!([]),
    }
}

/// Compute lifecycle badge strings for a meeting artifact.
fn compute_lifecycle_badges(
    path: &std::path::Path,
    prep_slugs: &std::collections::HashSet<String>,
) -> Vec<String> {
    let mut badges = Vec::new();

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return badges,
    };
    let (fm_str, body) = minutes_core::markdown::split_frontmatter(&content);
    let fm: Result<minutes_core::markdown::Frontmatter, _> =
        serde_yaml::from_str(&format!("---\n{}\n---", fm_str));

    if let Ok(fm) = fm {
        if meeting_has_prep(&fm.attendees, prep_slugs) {
            badges.push("prepped".into());
        }
        // "recorded" badge: all meetings/memos with transcripts are recorded
        if body.contains("## Transcript") || body.contains("## Summary") {
            badges.push("recorded".into());
        }
        // "debriefed" badge: has decisions or resolved intents (added by debrief)
        if !fm.decisions.is_empty() || fm.intents.iter().any(|i| i.status != "open") {
            badges.push("debriefed".into());
        }
    }

    badges
}

#[tauri::command]
pub fn cmd_search(query: String) -> serde_json::Value {
    let config = Config::load();
    let filters = minutes_core::search::SearchFilters {
        content_type: None,
        since: None,
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
    };
    match minutes_core::search::search(&query, &config, &filters) {
        Ok(results) => serde_json::to_value(&results).unwrap_or(serde_json::json!([])),
        Err(_) => serde_json::json!([]),
    }
}

#[tauri::command]
pub fn cmd_list_devices() -> serde_json::Value {
    let config = Config::load();
    let configured_device = config.recording.device.clone();
    let devices = minutes_core::capture::list_input_devices();
    serde_json::json!({
        "devices": devices,
        "configured_device": configured_device,
    })
}

#[tauri::command]
pub fn cmd_delete_meeting(path: String, with_audio: bool, force: bool) -> Result<String, String> {
    let md_path = std::path::PathBuf::from(&path);
    if !md_path.exists() {
        return Err(format!("File not found: {}", path));
    }

    let title = md_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let audio_path = md_path.with_extension("wav");
    let has_audio = audio_path.exists();

    if force {
        std::fs::remove_file(&md_path).map_err(|e| e.to_string())?;
        if with_audio && has_audio {
            std::fs::remove_file(&audio_path).map_err(|e| e.to_string())?;
        }
        Ok(format!("Deleted: {}", title))
    } else {
        let config = Config::load();
        let archive_dir = config.output_dir.join("archive");
        std::fs::create_dir_all(&archive_dir).map_err(|e| e.to_string())?;

        let dest_md = archive_dir.join(md_path.file_name().unwrap());
        std::fs::rename(&md_path, &dest_md).map_err(|e| e.to_string())?;

        if with_audio && has_audio {
            let dest_audio = archive_dir.join(audio_path.file_name().unwrap());
            std::fs::rename(&audio_path, &dest_audio).map_err(|e| e.to_string())?;
        }
        Ok(format!("Archived: {}", title))
    }
}

#[tauri::command]
pub fn cmd_open_file(app: tauri::AppHandle, path: String) -> Result<(), String> {
    open_target(&app, &path)
}

#[tauri::command]
pub fn cmd_clear_latest_output(state: tauri::State<AppState>) {
    set_latest_output(&state.latest_output, None);
}

#[tauri::command]
pub fn cmd_set_completion_notifications(state: tauri::State<AppState>, enabled: bool) {
    state
        .completion_notifications_enabled
        .store(enabled, Ordering::Relaxed);
}

#[tauri::command]
pub fn cmd_global_hotkey_settings(state: tauri::State<AppState>) -> HotkeySettings {
    current_hotkey_settings(&state)
}

#[tauri::command]
pub fn cmd_dictation_shortcut_settings(state: tauri::State<AppState>) -> HotkeySettings {
    current_dictation_shortcut_settings(&state)
}

#[tauri::command]
pub fn cmd_set_global_hotkey(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    enabled: bool,
    shortcut: String,
) -> Result<HotkeySettings, String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let next_shortcut = validate_hotkey_shortcut(&shortcut)?;
    let previous = current_hotkey_settings(&state);
    let manager = app.global_shortcut();

    if previous.enabled {
        manager
            .unregister(previous.shortcut.as_str())
            .map_err(|e| format!("Could not unregister {}: {}", previous.shortcut, e))?;
    }

    if enabled {
        if let Err(e) = manager.register(next_shortcut.as_str()) {
            if previous.enabled {
                let _ = manager.register(previous.shortcut.as_str());
            }
            return Err(format!(
                "Could not register {}. Another app may already be using it. ({})",
                next_shortcut, e
            ));
        }
    }

    state
        .global_hotkey_enabled
        .store(enabled, Ordering::Relaxed);
    if let Ok(mut current) = state.global_hotkey_shortcut.lock() {
        *current = next_shortcut;
    }

    Ok(current_hotkey_settings(&state))
}

#[tauri::command]
pub fn cmd_set_dictation_shortcut(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    enabled: bool,
    shortcut: String,
) -> Result<HotkeySettings, String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let next_shortcut = validate_dictation_shortcut(&shortcut)?;
    let previous = current_dictation_shortcut_settings(&state);
    let manager = app.global_shortcut();
    let quick_thought_shortcut = current_hotkey_settings(&state).shortcut;

    if next_shortcut == quick_thought_shortcut {
        return Err(format!(
            "{} is already used by the quick-thought shortcut. Choose a different dictation shortcut.",
            next_shortcut
        ));
    }

    if previous.enabled {
        manager
            .unregister(previous.shortcut.as_str())
            .map_err(|e| format!("Could not unregister {}: {}", previous.shortcut, e))?;
    }

    if enabled {
        if let Err(e) = manager.register(next_shortcut.as_str()) {
            if previous.enabled {
                let _ = manager.register(previous.shortcut.as_str());
            }
            return Err(format!(
                "Could not register {}. Another app may already be using it. ({})",
                next_shortcut, e
            ));
        }
    }

    state
        .dictation_shortcut_enabled
        .store(enabled, Ordering::Relaxed);
    if let Ok(mut current) = state.dictation_shortcut.lock() {
        *current = next_shortcut.clone();
    }

    let mut config = Config::load();
    config.dictation.shortcut_enabled = enabled;
    config.dictation.shortcut = next_shortcut.clone();
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;

    // Preload model when user enables dictation for the first time
    if enabled {
        let preload_config = Config::load();
        std::thread::spawn(move || {
            minutes_core::dictation::preload_model(&preload_config).ok();
        });
    }

    Ok(current_dictation_shortcut_settings(&state))
}

#[tauri::command]
pub fn cmd_permission_center() -> serde_json::Value {
    let config = Config::load();
    let items = vec![
        model_status(&config),
        microphone_status(),
        call_capture_status(),
        calendar_status(),
        watcher_status(&config),
        output_dir_status(&config),
        vault_status(&config),
    ];
    serde_json::to_value(items).unwrap_or(serde_json::json!([]))
}

#[tauri::command]
pub fn cmd_desktop_capabilities() -> DesktopCapabilities {
    DesktopCapabilities {
        platform: current_platform().into(),
        folder_reveal_label: folder_reveal_label().into(),
        supports_calendar_integration: supports_calendar_integration(),
        supports_call_detection: supports_call_detection(),
        supports_tray_artifact_copy: supports_tray_artifact_copy(),
        supports_dictation_hotkey: supports_dictation_hotkey(),
    }
}

#[tauri::command]
pub fn cmd_recovery_items() -> serde_json::Value {
    let config = Config::load();
    serde_json::to_value(scan_recovery_items(&config)).unwrap_or(serde_json::json!([]))
}

#[tauri::command]
pub fn cmd_retry_recovery(
    state: tauri::State<AppState>,
    path: String,
    content_type: String,
) -> Result<(), String> {
    if recording_active(&state.recording) || state.processing.load(Ordering::Relaxed) {
        return Err("Finish the current recording before retrying recovery items.".into());
    }

    let audio_path = PathBuf::from(&path);
    if !audio_path.exists() {
        return Err(format!("Recovery item not found: {}", path));
    }

    let ct = match content_type.as_str() {
        "meeting" => ContentType::Meeting,
        "memo" => ContentType::Memo,
        other => return Err(format!("Unsupported recovery type: {}", other)),
    };

    // Run pipeline on a background thread so the UI stays responsive
    let processing = state.processing.clone();
    let processing_stage = state.processing_stage.clone();
    let latest_output = state.latest_output.clone();

    processing.store(true, Ordering::Relaxed);
    set_processing_stage(&processing_stage, Some("Preparing transcript..."));

    std::thread::spawn(move || {
        let config = Config::load();
        match minutes_core::pipeline::process_with_progress(
            &audio_path,
            ct,
            None,
            &config,
            |stage| {
                let label = match stage {
                    minutes_core::pipeline::PipelineStage::Transcribing => "Transcribing...",
                    minutes_core::pipeline::PipelineStage::Diarizing => "Identifying speakers...",
                    minutes_core::pipeline::PipelineStage::Summarizing => "Generating summary...",
                    minutes_core::pipeline::PipelineStage::Saving => "Saving...",
                };
                set_processing_stage(&processing_stage, Some(label));
                let _ = minutes_core::pid::set_processing_status(
                    Some(label),
                    Some(minutes_core::pid::CaptureMode::Meeting),
                    None,
                    None,
                    0,
                );
            },
        ) {
            Ok(result) => {
                let notice = OutputNotice {
                    kind: "saved".into(),
                    title: result.title.clone(),
                    path: result.path.display().to_string(),
                    detail: "Recovery item was processed successfully.".into(),
                };
                set_latest_output(&latest_output, Some(notice));
                eprintln!("Recovery retry succeeded: {}", result.path.display());
            }
            Err(e) => {
                let notice = OutputNotice {
                    kind: "error".into(),
                    title: "Retry failed".into(),
                    path: audio_path.display().to_string(),
                    detail: format!("Recovery retry failed: {}", e),
                };
                set_latest_output(&latest_output, Some(notice));
                eprintln!("Recovery retry failed: {}", e);
            }
        }
        processing.store(false, Ordering::Relaxed);
        set_processing_stage(&processing_stage, None);
        minutes_core::pid::clear_processing_status().ok();
    });

    Ok(())
}

#[tauri::command]
pub fn cmd_get_meeting_detail(path: String) -> Result<MeetingDetail, String> {
    let config = Config::load();
    let meeting_path = std::path::PathBuf::from(&path);
    minutes_core::notes::validate_meeting_path(&meeting_path, &config.output_dir)?;

    let content = std::fs::read_to_string(&meeting_path).map_err(|e| e.to_string())?;
    let (frontmatter_str, body) = minutes_core::markdown::split_frontmatter(&content);
    let frontmatter: minutes_core::markdown::Frontmatter =
        serde_yaml::from_str(frontmatter_str.trim()).map_err(|e| e.to_string())?;

    let content_type = match frontmatter.r#type {
        ContentType::Meeting => "meeting",
        ContentType::Memo => "memo",
        ContentType::Dictation => "dictation",
    }
    .to_string();

    let status = frontmatter.status.map(|status| {
        match status {
            minutes_core::markdown::OutputStatus::Complete => "complete",
            minutes_core::markdown::OutputStatus::NoSpeech => "no-speech",
            minutes_core::markdown::OutputStatus::TranscriptOnly => "transcript-only",
        }
        .to_string()
    });

    let speaker_map: Vec<SpeakerAttributionView> = frontmatter
        .speaker_map
        .iter()
        .map(|a| SpeakerAttributionView {
            speaker_label: a.speaker_label.clone(),
            name: a.name.clone(),
            confidence: format!("{:?}", a.confidence).to_lowercase(),
            source: format!("{:?}", a.source).to_lowercase(),
        })
        .collect();

    let action_items: Vec<ActionItemView> = frontmatter
        .action_items
        .iter()
        .map(|a| ActionItemView {
            assignee: a.assignee.clone(),
            task: a.task.clone(),
            due: a.due.clone(),
            status: a.status.clone(),
        })
        .collect();

    let decisions: Vec<DecisionView> = frontmatter
        .decisions
        .iter()
        .map(|d| DecisionView {
            text: d.text.clone(),
            topic: d.topic.clone(),
        })
        .collect();

    Ok(MeetingDetail {
        path,
        title: frontmatter.title,
        date: frontmatter.date.to_rfc3339(),
        duration: frontmatter.duration,
        content_type,
        status,
        context: frontmatter.context,
        attendees: frontmatter.attendees,
        calendar_event: frontmatter.calendar_event,
        action_items,
        decisions,
        sections: parse_sections(body),
        speaker_map,
    })
}

#[tauri::command]
pub async fn cmd_list_voices() -> Result<serde_json::Value, String> {
    let conn = minutes_core::voice::open_db().map_err(|e| e.to_string())?;
    let profiles = minutes_core::voice::list_profiles(&conn).map_err(|e| e.to_string())?;
    serde_json::to_value(&profiles).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cmd_confirm_speaker(
    meeting_path: String,
    speaker_label: String,
    name: String,
) -> Result<String, String> {
    let path = std::path::PathBuf::from(&meeting_path);
    if !path.exists() {
        return Err(format!("Meeting not found: {}", meeting_path));
    }

    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let (fm_str, body) = minutes_core::markdown::split_frontmatter(&content);
    if fm_str.is_empty() {
        return Err("Meeting has no frontmatter".into());
    }

    let mut frontmatter: minutes_core::markdown::Frontmatter =
        serde_yaml::from_str(fm_str).map_err(|e| e.to_string())?;

    let found = frontmatter
        .speaker_map
        .iter_mut()
        .find(|a| a.speaker_label == speaker_label);

    if let Some(attr) = found {
        attr.name = name.clone();
        attr.confidence = minutes_core::diarize::Confidence::High;
        attr.source = minutes_core::diarize::AttributionSource::Manual;
    } else {
        return Err(format!(
            "Speaker '{}' not found in speaker_map",
            speaker_label
        ));
    }

    let new_body = minutes_core::diarize::apply_confirmed_names(body, &frontmatter.speaker_map);
    let new_yaml = serde_yaml::to_string(&frontmatter).map_err(|e| e.to_string())?;
    let new_content = format!("---\n{}---\n{}", new_yaml, new_body);
    std::fs::write(&path, new_content).map_err(|e| e.to_string())?;

    Ok(format!("Confirmed: {} = {}", speaker_label, name))
}

#[tauri::command]
pub async fn cmd_upcoming_meetings() -> serde_json::Value {
    tauri::async_runtime::spawn_blocking(|| {
        let events = minutes_core::calendar::upcoming_events(120); // 2 hour lookahead
        serde_json::to_value(&events).unwrap_or(serde_json::json!([]))
    })
    .await
    .unwrap_or(serde_json::json!([]))
}

#[tauri::command]
pub fn cmd_needs_setup() -> serde_json::Value {
    let config = Config::load();
    let model_name = &config.transcription.model;
    let model_dir = &config.transcription.model_path;
    let model_file = model_dir.join(format!("ggml-{}.bin", model_name));
    let has_model = model_file.exists();

    let meetings_dir = config.output_dir.clone();
    let has_meetings_dir = meetings_dir.exists();

    serde_json::json!({
        "needsSetup": !has_model,
        "hasModel": has_model,
        "modelName": model_name,
        "hasMeetingsDir": has_meetings_dir,
    })
}

#[tauri::command]
pub async fn cmd_download_model(model: String) -> Result<String, String> {
    // Run in a blocking thread so the UI stays responsive during download
    tauri::async_runtime::spawn_blocking(move || {
        let config = Config::load();
        let model_dir = &config.transcription.model_path;
        let model_file = model_dir.join(format!("ggml-{}.bin", model));

        if model_file.exists() {
            return Ok(format!("Model '{}' already downloaded", model));
        }

        std::fs::create_dir_all(model_dir).map_err(|e| e.to_string())?;

        let url = format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
            model
        );

        eprintln!("[minutes] Downloading model: {} from {}", model, url);

        let status = std::process::Command::new("curl")
            .args([
                "-L",
                "-o",
                &model_file.to_string_lossy(),
                &url,
                "--progress-bar",
            ])
            .status()
            .map_err(|e| format!("curl failed: {}", e))?;

        if !status.success() {
            return Err("Download failed".into());
        }

        let size = std::fs::metadata(&model_file)
            .map(|m| m.len() / (1024 * 1024))
            .unwrap_or(0);

        Ok(format!("Downloaded '{}' model ({} MB)", model, size))
    })
    .await
    .map_err(|e| format!("Download task failed: {}", e))?
}

// ── Terminal / AI Assistant commands ──────────────────────────

fn meeting_title_from_path(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.replace('-', " "))
        .unwrap_or_else(|| "Meeting Discussion".into())
}

fn terminal_title_for_mode(mode: &str, meeting_path: Option<&str>) -> Result<String, String> {
    match mode {
        "assistant" => Ok("Minutes Assistant".into()),
        "meeting" => Ok(format!(
            "Discussing: {}",
            meeting_title_from_path(meeting_path.ok_or("meeting_path required for meeting mode")?)
        )),
        other => Err(format!(
            "Unknown mode: {}. Use 'meeting' or 'assistant'.",
            other
        )),
    }
}

fn sync_workspace_for_mode(
    workspace: &Path,
    config: &Config,
    mode: &str,
    meeting_path: Option<&str>,
) -> Result<(), String> {
    // write_assistant_context preserves live transcript markers if present (U2/T3)
    crate::context::write_assistant_context(workspace, config)?;

    match mode {
        "assistant" => crate::context::clear_active_meeting_context(workspace),
        "meeting" => {
            let path = meeting_path.ok_or("meeting_path required for meeting mode")?;
            let meeting = PathBuf::from(path);
            minutes_core::notes::validate_meeting_path(&meeting, &config.output_dir)?;
            crate::context::write_active_meeting_context(workspace, &meeting, config)
        }
        other => Err(format!(
            "Unknown mode: {}. Use 'meeting' or 'assistant'.",
            other
        )),
    }
}

fn is_shell_command(command: &str) -> bool {
    matches!(
        Path::new(command)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(command),
        "bash" | "zsh" | "sh" | "fish"
    )
}

fn context_switch_prompt(command: &str, mode: &str, title: &str) -> String {
    let plain_text = match mode {
        "meeting" => format!(
            "Minutes changed focus to {title}. Read CURRENT_MEETING.md and CLAUDE.md, then help with that meeting."
        ),
        _ => "Minutes cleared the active meeting focus. Resume general assistant mode and reread CLAUDE.md if needed."
            .into(),
    };

    if is_shell_command(command) {
        format!("cat <<'__MINUTES__'\n{plain_text}\n__MINUTES__\n")
    } else {
        format!("{plain_text}\n")
    }
}

/// Resolve an agent name or path to an executable.
///
/// Accepts either:
/// - A bare command name ("claude", "codex", "bash") — looked up via PATH
///   (with PATHEXT on Windows, so `claude.cmd` resolves from `claude`), then
///   searched in well-known install dirs as a fallback
/// - An absolute path ("/usr/local/bin/my-agent") — used directly if it exists
///
/// This is intentionally open: users can set `assistant.agent` to any binary
/// they want, including wrapper scripts or custom agent CLIs.
pub fn find_agent_binary(name: &str) -> Option<PathBuf> {
    // If it's an absolute path, check it directly
    let as_path = PathBuf::from(name);
    if as_path.is_absolute() && as_path.exists() {
        return Some(as_path);
    }

    // PATH lookup (cross-platform). On Windows this respects PATHEXT and
    // resolves `claude` → `claude.cmd` / `claude.exe` correctly. GUI apps
    // launched from Finder/Explorer often have a minimal PATH, so the
    // fallback below catches common install dirs that aren't on PATH.
    if let Ok(path) = which::which(name) {
        return Some(path);
    }

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let mut search_dirs: Vec<PathBuf> = vec![
        home.join(".cargo/bin"),
        home.join(".local/bin"),
        home.join(".npm-global/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ];
    if cfg!(windows) {
        // npm-global on Windows lands in %APPDATA%\npm by default, which
        // isn't always on PATH for GUI processes. LOCALAPPDATA covers a few
        // installer conventions (e.g., scoop, native installers).
        if let Some(appdata) = dirs::data_dir() {
            search_dirs.push(appdata.join("npm"));
        }
        if let Some(local) = dirs::data_local_dir() {
            search_dirs.push(local.join("npm"));
            search_dirs.push(local.join("Programs"));
        }
    }

    let exts: &[&str] = if cfg!(windows) {
        &["", "cmd", "exe", "bat"]
    } else {
        &[""]
    };
    for dir in &search_dirs {
        for ext in exts {
            let mut candidate = dir.join(name);
            if !ext.is_empty() {
                candidate.set_extension(ext);
            }
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Platform-correct path to the user's config file, used in error messages.
fn user_config_path_for_display() -> String {
    Config::config_path().display().to_string()
}

/// Shared spawn logic used by both cmd_spawn_terminal and the tray menu handler.
/// Returns (session_id, window_title) on success.
pub fn spawn_terminal(
    app: &tauri::AppHandle,
    pty_manager: &std::sync::Arc<Mutex<crate::pty::PtyManager>>,
    mode: &str,
    meeting_path: Option<&str>,
    agent_override: Option<&str>,
) -> Result<(String, String), String> {
    let config = Config::load();
    let title = terminal_title_for_mode(mode, meeting_path)?;
    let workspace = crate::context::create_workspace(&config)?;
    sync_workspace_for_mode(&workspace, &config, mode, meeting_path)?;

    let mut manager = pty_manager.lock().map_err(|_| "PTY manager lock failed")?;

    if manager.assistant_session_id().is_some() {
        manager.set_session_title(crate::pty::ASSISTANT_SESSION_ID, title.clone())?;
        // Only send a context switch prompt when actively switching to a
        // meeting (not when merely re-opening the panel in assistant mode,
        // which would inject unwanted text into Claude Code's input).
        if mode == "meeting" {
            if let Some(command) = manager.session_command(crate::pty::ASSISTANT_SESSION_ID) {
                let prompt = context_switch_prompt(&command, mode, &title);
                manager.write_input(crate::pty::ASSISTANT_SESSION_ID, prompt.as_bytes())?;
            }
        }
    } else {
        let agent_name = agent_override.unwrap_or(&config.assistant.agent);
        let agent_bin = find_agent_binary(agent_name).ok_or_else(|| {
            let install_hint = if agent_name == "claude" {
                " Install Claude Code with `npm i -g @anthropic-ai/claude-code`."
            } else {
                ""
            };
            format!(
                "'{}' not found on PATH or in common install dirs.{} \
                 Then set the agent in {} under [assistant].",
                agent_name,
                install_hint,
                user_config_path_for_display(),
            )
        })?;

        manager.spawn(
            crate::pty::SpawnConfig {
                session_id: crate::pty::ASSISTANT_SESSION_ID.into(),
                app_handle: app.clone(),
                command: agent_bin.to_str().unwrap_or(agent_name).to_string(),
                args: config.assistant.agent_args.clone(),
                cwd: workspace.clone(),
                context_dir: workspace.clone(),
                title: title.clone(),
                target_window: "main".into(),
            },
            120,
            30,
        )?;
    }

    drop(manager);

    // Emit recall:expand event to the main window instead of opening a
    // separate terminal window. The JS in index.html handles the panel
    // expand animation and xterm.js initialisation.
    if let Some(win) = app.get_webview_window("main") {
        win.show().ok();
        win.set_focus().ok();
        app.emit_to(
            "main",
            "recall:expand",
            serde_json::json!({ "title": title, "mode": mode }),
        )
        .ok();
    }

    Ok((crate::pty::ASSISTANT_SESSION_ID.into(), title))
}

#[tauri::command]
pub fn cmd_spawn_terminal(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    mode: String,
    meeting_path: Option<String>,
    agent: Option<String>,
) -> Result<String, String> {
    let (session_id, _) = spawn_terminal(
        &app,
        &state.pty_manager,
        &mode,
        meeting_path.as_deref(),
        agent.as_deref(),
    )?;
    Ok(session_id)
}

#[tauri::command]
pub fn cmd_pty_input(
    state: tauri::State<AppState>,
    session_id: String,
    data: String,
) -> Result<(), String> {
    let mut manager = state.pty_manager.lock().map_err(|_| "Lock failed")?;
    manager.write_input(&session_id, data.as_bytes())
}

#[tauri::command]
pub fn cmd_pty_resize(
    state: tauri::State<AppState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let manager = state.pty_manager.lock().map_err(|_| "Lock failed")?;
    manager.resize(&session_id, cols, rows)
}

#[tauri::command]
pub fn cmd_pty_kill(state: tauri::State<AppState>, session_id: String) -> Result<(), String> {
    let mut manager = state.pty_manager.lock().map_err(|_| "Lock failed")?;
    manager.kill_session(&session_id);
    Ok(())
}

/// Well-known agent CLIs to check for in cmd_list_agents.
const WELL_KNOWN_AGENTS: &[&str] = &["claude", "codex", "bash", "zsh"];

#[tauri::command]
pub fn cmd_list_agents() -> serde_json::Value {
    let agents: Vec<serde_json::Value> = WELL_KNOWN_AGENTS
        .iter()
        .filter_map(|name| {
            find_agent_binary(name).map(|path| {
                serde_json::json!({
                    "name": name,
                    "path": path.display().to_string(),
                })
            })
        })
        .collect();
    serde_json::json!(agents)
}

#[tauri::command]
pub fn cmd_terminal_info(state: tauri::State<AppState>, session_id: String) -> TerminalInfo {
    let title = state
        .pty_manager
        .lock()
        .ok()
        .and_then(|manager| manager.session_title(&session_id))
        .unwrap_or_else(|| "Minutes Assistant".into());
    TerminalInfo { title }
}

// ── Settings commands ─────────────────────────────────────────

#[tauri::command]
pub fn cmd_get_settings() -> serde_json::Value {
    let config = Config::load();
    let path = Config::config_path();

    // Check env vars for API key status
    let anthropic_key_set = std::env::var("ANTHROPIC_API_KEY").is_ok();
    let openai_key_set = std::env::var("OPENAI_API_KEY").is_ok();

    // Check Ollama reachability
    let ollama_reachable = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(std::time::Duration::from_secs(2)))
            .build(),
    )
    .get(&format!("{}/api/tags", config.summarization.ollama_url))
    .call()
    .is_ok();

    // Check which whisper model is downloaded
    let model_path = config.transcription.model_path.clone();
    let downloaded_models: Vec<String> = ["tiny", "base", "small", "medium", "large-v3"]
        .iter()
        .filter(|m| {
            let pattern = format!("ggml-{}", m);
            model_path
                .read_dir()
                .into_iter()
                .flatten()
                .flatten()
                .any(|e| {
                    e.file_name()
                        .to_str()
                        .map(|n| n.contains(&pattern))
                        .unwrap_or(false)
                })
        })
        .map(|s| s.to_string())
        .collect();

    serde_json::json!({
        "config_path": path.display().to_string(),
        "recording": {
            "device": config.recording.device,
        },
        "transcription": {
            "model": config.transcription.model,
            "downloaded_models": downloaded_models,
            "language": config.transcription.language,
        },
        "diarization": {
            "engine": config.diarization.engine,
        },
        "summarization": {
            "engine": config.summarization.engine,
            "agent_command": config.summarization.agent_command,
            "ollama_model": config.summarization.ollama_model,
            "ollama_url": config.summarization.ollama_url,
            "anthropic_key_set": anthropic_key_set,
            "openai_key_set": openai_key_set,
            "ollama_reachable": ollama_reachable,
        },
        "screen_context": {
            "enabled": config.screen_context.enabled,
            "interval_secs": config.screen_context.interval_secs,
            "keep_after_summary": config.screen_context.keep_after_summary,
        },
        "assistant": {
            "agent": config.assistant.agent,
            "agent_args": config.assistant.agent_args,
        },
        "hooks": {
            "post_record": config.hooks.post_record,
        },
        "call_detection": {
            "enabled": config.call_detection.enabled,
            "poll_interval_secs": config.call_detection.poll_interval_secs,
            "cooldown_minutes": config.call_detection.cooldown_minutes,
            "apps": config.call_detection.apps,
            "google_meet_enabled": call_detection_has_sentinel(&config, "google-meet"),
        },
        "dictation": {
            "model": config.dictation.model,
            "destination": config.dictation.destination,
            "accumulate": config.dictation.accumulate,
            "daily_note_log": config.dictation.daily_note_log,
            "cleanup_engine": config.dictation.cleanup_engine,
            "auto_paste": config.dictation.auto_paste,
            "silence_timeout_ms": config.dictation.silence_timeout_ms,
            "max_utterance_secs": config.dictation.max_utterance_secs,
            "shortcut_enabled": config.dictation.shortcut_enabled,
            "shortcut": config.dictation.shortcut,
            "hotkey_enabled": config.dictation.hotkey_enabled,
            "hotkey_keycode": config.dictation.hotkey_keycode,
        },
    })
}

#[tauri::command]
pub fn cmd_set_setting(section: String, key: String, value: String) -> Result<String, String> {
    let mut config = Config::load();

    match (section.as_str(), key.as_str()) {
        // Transcription
        ("transcription", "model") => config.transcription.model = value.clone(),
        ("transcription", "language") => {
            config.transcription.language = parse_optional_string_setting(&value);
        }

        // Recording
        ("recording", "device") => {
            config.recording.device = parse_optional_string_setting(&value);
        }

        // Diarization
        ("diarization", "engine") => config.diarization.engine = value.clone(),

        // Summarization
        ("summarization", "engine") => config.summarization.engine = value.clone(),
        ("summarization", "agent_command") => config.summarization.agent_command = value.clone(),
        ("summarization", "ollama_model") => config.summarization.ollama_model = value.clone(),
        ("summarization", "ollama_url") => config.summarization.ollama_url = value.clone(),

        // Screen context
        ("screen_context", "enabled") => {
            config.screen_context.enabled = value == "true";
        }
        ("screen_context", "interval_secs") => {
            config.screen_context.interval_secs = value
                .parse()
                .map_err(|_| "interval_secs must be a number")?;
        }
        ("screen_context", "keep_after_summary") => {
            config.screen_context.keep_after_summary = value == "true";
        }

        // Assistant
        ("assistant", "agent") => config.assistant.agent = value.clone(),
        ("assistant", "agent_args") => {
            config.assistant.agent_args = if value.trim().is_empty() {
                vec![]
            } else {
                value.split_whitespace().map(String::from).collect()
            };
        }

        // Call detection
        ("call_detection", "enabled") => {
            config.call_detection.enabled = value == "true";
        }
        ("call_detection", "poll_interval_secs") => {
            config.call_detection.poll_interval_secs = value
                .parse()
                .map_err(|_| "poll_interval_secs must be a number")?;
        }
        ("call_detection", "cooldown_minutes") => {
            config.call_detection.cooldown_minutes = value
                .parse()
                .map_err(|_| "cooldown_minutes must be a number")?;
        }
        ("call_detection", "google_meet_enabled") => {
            set_call_detection_sentinel(&mut config, "google-meet", value == "true");
        }

        // Dictation
        ("dictation", "model") => {
            config.dictation.model = value.clone();
            // Re-preload the new model in background so next dictation is instant
            let preload_config = config.clone();
            std::thread::spawn(move || {
                if let Err(e) = minutes_core::dictation::preload_model(&preload_config) {
                    eprintln!("[dictation] model re-preload failed: {}", e);
                }
            });
        }
        ("dictation", "daily_note_log") => {
            config.dictation.daily_note_log = value == "true";
        }
        ("dictation", "accumulate") => {
            config.dictation.accumulate = value == "true";
        }
        ("dictation", "silence_timeout_ms") => {
            config.dictation.silence_timeout_ms = value
                .parse()
                .map_err(|_| "silence_timeout_ms must be a number")?;
        }
        ("dictation", "destination") => config.dictation.destination = value.clone(),
        ("dictation", "auto_paste") => {
            config.dictation.auto_paste = value == "true";
        }
        ("dictation", "cleanup_engine") => config.dictation.cleanup_engine = value.clone(),
        ("dictation", "shortcut_enabled") => {
            config.dictation.shortcut_enabled = value == "true";
        }
        ("dictation", "shortcut") => config.dictation.shortcut = value.clone(),
        ("dictation", "hotkey_enabled") => {
            config.dictation.hotkey_enabled = value == "true";
        }
        ("dictation", "hotkey_keycode") => {
            config.dictation.hotkey_keycode = value
                .parse()
                .map_err(|_| "hotkey_keycode must be a number")?;
        }

        // Live transcript
        ("live_transcript", "shortcut_enabled") => {
            config.live_transcript.shortcut_enabled = value == "true";
        }
        ("live_transcript", "shortcut") => {
            config.live_transcript.shortcut = value.clone();
        }

        // Hooks
        ("hooks", "post_record") => {
            config.hooks.post_record = parse_optional_string_setting(&value);
        }

        _ => return Err(format!("Unknown setting: {}.{}", section, key)),
    }

    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;

    Ok(format!("Set {}.{} = {}", section, key, value))
}

#[tauri::command]
pub fn cmd_get_autostart(app: tauri::AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
pub fn cmd_set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())
    } else {
        manager.disable().map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn cmd_get_storage_stats() -> serde_json::Value {
    let config = Config::load();

    fn walk_size(path: &std::path::Path) -> (u64, usize) {
        let mut total_bytes = 0u64;
        let mut file_count = 0usize;
        for entry in walkdir::WalkDir::new(path).into_iter().flatten() {
            if entry.file_type().is_file() {
                total_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                file_count += 1;
            }
        }
        (total_bytes, file_count)
    }

    let meetings_dir = &config.output_dir;
    let memos_dir = config.output_dir.join("memos");
    let models_dir = &config.transcription.model_path;
    let screens_dir = Config::minutes_dir().join("screens");

    let (meetings_bytes, meetings_count) = walk_size(meetings_dir);
    let (memos_bytes, memos_count) = walk_size(&memos_dir);
    let (models_bytes, _) = walk_size(models_dir);
    let (screens_bytes, screens_count) = walk_size(&screens_dir);

    serde_json::json!({
        "meetings": { "bytes": meetings_bytes, "count": meetings_count },
        "memos": { "bytes": memos_bytes, "count": memos_count },
        "models": { "bytes": models_bytes },
        "screens": { "bytes": screens_bytes, "count": screens_count },
        "total_bytes": meetings_bytes + memos_bytes + models_bytes + screens_bytes,
    })
}

#[tauri::command]
pub fn cmd_open_meeting_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    open_target(&app, &url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn preserve_failed_capture_moves_audio_into_failed_captures() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().join("meetings"),
            ..Config::default()
        };
        let wav = dir.path().join("current.wav");
        std::fs::write(&wav, vec![1_u8; 256]).unwrap();

        let preserved = preserve_failed_capture(&wav, &config).unwrap();

        assert!(!wav.exists());
        assert!(preserved.exists());
        assert!(preserved.starts_with(config.output_dir.join("failed-captures")));
    }

    #[test]
    fn wait_for_path_removal_returns_false_after_timeout() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("still-there.pid");
        std::fs::write(&path, "123").unwrap();

        let removed = wait_for_path_removal(&path, Some(std::time::Duration::from_millis(50)));

        assert!(!removed);
        assert!(path.exists());
    }

    #[test]
    fn wait_for_path_removal_returns_true_when_file_disappears() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("gone-soon.pid");
        std::fs::write(&path, "123").unwrap();

        let path_for_thread = path.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            std::fs::remove_file(path_for_thread).unwrap();
        });

        let removed = wait_for_path_removal(&path, Some(std::time::Duration::from_secs(1)));

        assert!(removed);
        assert!(!path.exists());
    }

    #[test]
    fn stage_label_maps_pipeline_stage_to_user_facing_copy() {
        assert_eq!(
            stage_label(
                minutes_core::pipeline::PipelineStage::Transcribing,
                CaptureMode::QuickThought
            ),
            "Transcribing quick thought"
        );
        assert_eq!(
            stage_label(
                minutes_core::pipeline::PipelineStage::Saving,
                CaptureMode::Meeting
            ),
            "Saving meeting"
        );
    }

    #[test]
    fn parse_optional_string_setting_preserves_auto_detect_state() {
        assert_eq!(parse_optional_string_setting(""), None);
        assert_eq!(parse_optional_string_setting("   "), None);
        assert_eq!(parse_optional_string_setting("en"), Some("en".to_string()));
        assert_eq!(
            parse_optional_string_setting(" es "),
            Some("es".to_string())
        );
    }

    #[test]
    fn call_detection_sentinel_toggle_is_idempotent() {
        let mut config = Config::default();
        assert!(!call_detection_has_sentinel(&config, "google-meet"));

        set_call_detection_sentinel(&mut config, "google-meet", true);
        assert!(call_detection_has_sentinel(&config, "google-meet"));

        set_call_detection_sentinel(&mut config, "google-meet", true);
        assert_eq!(
            config
                .call_detection
                .apps
                .iter()
                .filter(|app| app.as_str() == "google-meet")
                .count(),
            1
        );

        set_call_detection_sentinel(&mut config, "google-meet", false);
        assert!(!call_detection_has_sentinel(&config, "google-meet"));
    }

    #[test]
    fn set_latest_output_replaces_previous_notice() {
        let latest_output = Arc::new(Mutex::new(None));
        set_latest_output(
            &latest_output,
            Some(OutputNotice {
                kind: "saved".into(),
                title: "Demo".into(),
                path: "/tmp/demo.md".into(),
                detail: "Saved".into(),
            }),
        );

        let current = latest_output.lock().unwrap().clone().unwrap();
        assert_eq!(current.title, "Demo");
        assert_eq!(current.path, "/tmp/demo.md");
    }

    #[test]
    fn needs_review_jobs_surface_as_preserved_capture_notices() {
        let job = minutes_core::jobs::ProcessingJob {
            id: "job-review".into(),
            title: Some("Interview".into()),
            mode: CaptureMode::Meeting,
            content_type: ContentType::Meeting,
            state: minutes_core::jobs::JobState::NeedsReview,
            stage: minutes_core::jobs::JobState::NeedsReview.default_stage(),
            output_path: Some("/tmp/interview.md".into()),
            audio_path: "/tmp/interview.wav".into(),
            error: Some("silence strip removed ALL audio".into()),
            created_at: chrono::Local::now(),
            started_at: None,
            finished_at: Some(chrono::Local::now()),
            recording_started_at: None,
            recording_finished_at: None,
            user_notes: None,
            pre_context: None,
            calendar_event: None,
            word_count: Some(0),
            owner_pid: None,
        };

        let notice = output_notice_from_job(&job).expect("needs-review notice");
        assert_eq!(notice.kind, "preserved-capture");
        assert_eq!(notice.path, "/tmp/interview.wav");
        assert!(notice.detail.contains("silence strip"));
    }

    #[test]
    fn desktop_capabilities_align_with_helper_flags() {
        let caps = cmd_desktop_capabilities();

        assert_eq!(caps.platform, current_platform());
        assert_eq!(caps.folder_reveal_label, folder_reveal_label());
        assert_eq!(
            caps.supports_calendar_integration,
            supports_calendar_integration()
        );
        assert_eq!(caps.supports_call_detection, supports_call_detection());
        assert_eq!(
            caps.supports_tray_artifact_copy,
            supports_tray_artifact_copy()
        );
        assert_eq!(caps.supports_dictation_hotkey, supports_dictation_hotkey());
    }

    #[test]
    fn scan_recovery_items_finds_failed_capture_and_watch_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let watch_dir = dir.path().join("watch");
        let failed_dir = watch_dir.join("failed");
        let output_dir = dir.path().join("meetings");
        let failed_captures = output_dir.join("failed-captures");
        std::fs::create_dir_all(&failed_dir).unwrap();
        std::fs::create_dir_all(&failed_captures).unwrap();

        let failed_watch = failed_dir.join("idea.m4a");
        let failed_capture = failed_captures.join("capture.wav");
        std::fs::write(&failed_watch, "watch").unwrap();
        std::fs::write(&failed_capture, "capture").unwrap();

        let config = Config {
            output_dir: output_dir.clone(),
            watch: minutes_core::config::WatchConfig {
                paths: vec![watch_dir],
                ..Config::default().watch
            },
            ..Config::default()
        };

        let items = scan_recovery_items(&config);
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| item.kind == "watch-failed"));
        assert!(items.iter().any(|item| item.kind == "preserved-capture"));
    }

    #[test]
    fn model_status_reports_missing_model() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = Config {
            transcription: minutes_core::config::TranscriptionConfig {
                model: "small".into(),
                model_path: dir.path().join("models"),
                min_words: 3,
                language: Some("en".into()),
                vad_model: "silero-v6.2.0".into(),
                noise_reduction: false,
                ..minutes_core::config::TranscriptionConfig::default()
            },
            ..Config::default()
        };

        let status = model_status(&config);
        assert_eq!(status.label, "Speech model");
        assert_eq!(status.state, "attention");
    }

    #[test]
    fn display_path_rewrites_home_prefix() {
        let home = dirs::home_dir().unwrap();
        let path = home.join("meetings/demo.md");
        let displayed = display_path(&path.display().to_string());
        assert!(displayed.starts_with("~/"));
    }

    #[test]
    fn parse_sections_preserves_top_level_order() {
        let body = "## Summary\n\nHello\n\n## Notes\n\n- One\n\n## Transcript\n\n[0:00] Hi\n";
        let sections = parse_sections(body);

        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].heading, "Summary");
        assert_eq!(sections[1].heading, "Notes");
        assert_eq!(sections[2].heading, "Transcript");
        assert!(sections[2].content.contains("[0:00] Hi"));
    }

    #[test]
    fn validate_hotkey_shortcut_accepts_known_values() {
        assert_eq!(
            validate_hotkey_shortcut("CmdOrCtrl+Shift+M").unwrap(),
            "CmdOrCtrl+Shift+M"
        );
    }

    #[test]
    fn validate_hotkey_shortcut_rejects_unknown_values() {
        assert!(validate_hotkey_shortcut("CmdOrCtrl+Shift+P").is_err());
    }

    #[test]
    fn short_hotkey_capture_is_discarded() {
        let started = Instant::now() - std::time::Duration::from_millis(200);
        assert!(should_discard_hotkey_capture(Some(started), Instant::now()));
    }

    #[test]
    fn long_hotkey_capture_is_kept() {
        let started = Instant::now() - std::time::Duration::from_millis(450);
        assert!(!should_discard_hotkey_capture(
            Some(started),
            Instant::now()
        ));
    }

    #[test]
    fn reset_hotkey_capture_state_clears_runtime_and_discard_flag() {
        let runtime = Arc::new(Mutex::new(HotkeyRuntime {
            key_down: true,
            key_down_started_at: Some(Instant::now()),
            active_capture: Some(HotkeyCaptureStyle::Locked),
            recording_started_at: Some(Instant::now()),
            hold_generation: 9,
        }));
        let discard = Arc::new(AtomicBool::new(true));

        reset_hotkey_capture_state(Some(&runtime), Some(&discard));

        let current = runtime.lock().unwrap();
        assert!(!current.key_down);
        assert!(current.key_down_started_at.is_none());
        assert!(current.active_capture.is_none());
        assert!(current.recording_started_at.is_none());
        assert!(!discard.load(Ordering::Relaxed));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn short_hotkey_tap_detection_matches_threshold() {
        let started = Instant::now() - std::time::Duration::from_millis(200);
        assert!(is_short_hotkey_tap(Some(started), Instant::now()));

        let started = Instant::now() - std::time::Duration::from_millis(350);
        assert!(!is_short_hotkey_tap(Some(started), Instant::now()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn clear_dictation_hotkey_capture_state_resets_press_tracking() {
        let mut runtime = DictationHotkeyRuntime {
            generation: 2,
            keycode: 57,
            lifecycle: DictationHotkeyLifecycle::Active,
            last_error: None,
            monitor: None,
            key_down: true,
            key_down_started_at: Some(Instant::now()),
            active_capture: Some(HotkeyCaptureStyle::Hold),
            hold_generation: 4,
        };

        clear_dictation_hotkey_capture_state(&mut runtime);

        assert!(!runtime.key_down);
        assert!(runtime.key_down_started_at.is_none());
        assert!(runtime.active_capture.is_none());
        assert_eq!(runtime.hold_generation, 4);
    }

    #[test]
    fn extract_paste_text_returns_summary_section() {
        let content = "---\ntitle: Demo\n---\n\n## Summary\n\nShort summary.\n\n## Transcript\n\nFull transcript.\n";
        let summary = extract_paste_text(content, "summary").unwrap();
        assert_eq!(summary, "Short summary.");
    }

    #[test]
    fn extract_paste_text_rejects_missing_summary() {
        let content = "---\ntitle: Demo\n---\n\n## Transcript\n\nFull transcript.\n";
        assert!(extract_paste_text(content, "summary").is_err());
    }
}

// ── Dictation commands ──────────────────────────────────────

#[tauri::command]
pub fn cmd_start_dictation(
    app: tauri::AppHandle,
    _state: tauri::State<AppState>,
) -> Result<String, String> {
    start_dictation_session(&app, None)
}

#[tauri::command]
pub fn cmd_stop_dictation(state: tauri::State<AppState>) -> Result<String, String> {
    if state.dictation_active.load(Ordering::Relaxed) {
        state.dictation_stop_flag.store(true, Ordering::Relaxed);
        return Ok("Dictation stop requested".into());
    }
    if dictation_pid_active() {
        return Err("Dictation is running in another Minutes process.".into());
    }
    Err("Dictation is not active".into())
}

fn show_dictation_overlay(app: &tauri::AppHandle) {
    use tauri::WebviewUrl;

    // Close existing overlay if any
    if let Some(win) = app.get_webview_window("dictation-overlay") {
        win.close().ok();
    }

    // Position: bottom-right HUD, anchored to the current monitor work area.
    let width = 320.0;
    let height = 88.0;
    let inset_x = 16.0;
    let inset_y = 16.0;

    let monitor = app
        .get_webview_window("main")
        .and_then(|window| window.current_monitor().ok().flatten())
        .or_else(|| {
            app.get_webview_window("main")
                .and_then(|window| window.primary_monitor().ok().flatten())
        });

    let (x, y) = if let Some(monitor) = monitor {
        let scale = monitor.scale_factor();
        let work_area = monitor.work_area();
        let work_x = work_area.position.x as f64 / scale;
        let work_y = work_area.position.y as f64 / scale;
        let work_width = work_area.size.width as f64 / scale;
        let work_height = work_area.size.height as f64 / scale;
        (
            work_x + work_width - width - inset_x,
            work_y + work_height - height - inset_y,
        )
    } else {
        (1440.0 - width - inset_x, 900.0 - height - inset_y)
    };

    match tauri::WebviewWindowBuilder::new(
        app,
        "dictation-overlay",
        WebviewUrl::App("dictation-overlay.html".into()),
    )
    .title("Dictation")
    .inner_size(width, height)
    .position(x, y)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .focused(false)
    .skip_taskbar(true)
    .build()
    {
        Ok(_) => eprintln!("[dictation] overlay shown"),
        Err(e) => eprintln!("[dictation] overlay failed: {}", e),
    }
}

// ── Live transcript commands ─────────────────────────────────

/// RAII guard that resets the live_transcript_active flag on drop (even on panic).
struct LiveActiveGuard(Arc<AtomicBool>);
impl Drop for LiveActiveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Shared live transcript session runner. Spawned on a background thread by both
/// cmd_start_live_transcript and handle_live_shortcut_event.
fn run_live_session(app: tauri::AppHandle, active: Arc<AtomicBool>, stop_flag: Arc<AtomicBool>) {
    let _guard = LiveActiveGuard(active);

    let config = Config::load();

    if let Ok(workspace) = crate::context::create_workspace(&config) {
        update_assistant_live_context(&workspace, true);
    }

    crate::update_tray_state_with_mode(&app, true, true);

    let result = minutes_core::live_transcript::run(stop_flag.clone(), &config);

    stop_flag.store(false, Ordering::Relaxed);

    if let Ok(workspace) = crate::context::create_workspace(&config) {
        update_assistant_live_context(&workspace, false);
    }

    match result {
        Ok((lines, duration, _path)) => {
            eprintln!(
                "[live-transcript] ended: {} lines in {:.0}s",
                lines, duration
            );
            if let Some(win) = app.get_webview_window("main") {
                win.emit(
                    "live-transcript:stopped",
                    serde_json::json!({ "lines": lines, "duration_secs": duration }),
                )
                .ok();
            }
        }
        Err(e) => {
            eprintln!("[live-transcript] error: {}", e);
            if let Some(win) = app.get_webview_window("main") {
                win.emit(
                    "live-transcript:error",
                    serde_json::json!({ "error": e.to_string() }),
                )
                .ok();
            }
        }
    }

    crate::update_tray_state(&app, false);
}

/// Try to acquire the live transcript state. Returns Err with a message on conflict.
fn try_acquire_live(state: &AppState) -> Result<(), String> {
    if state
        .live_transcript_active
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("Live transcript already active".into());
    }
    if recording_active(&state.recording) {
        state.live_transcript_active.store(false, Ordering::SeqCst);
        return Err("Recording already in progress — it already includes a live transcript".into());
    }
    if state.dictation_active.load(Ordering::Relaxed) {
        state.live_transcript_active.store(false, Ordering::SeqCst);
        return Err("Dictation in progress — stop dictation first".into());
    }
    Ok(())
}

#[tauri::command]
pub fn cmd_start_live_transcript(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    try_acquire_live(&state)?;

    let active = state.live_transcript_active.clone();
    let stop_flag = state.live_transcript_stop_flag.clone();
    stop_flag.store(false, Ordering::Relaxed);

    let app_clone = app.clone();
    std::thread::spawn(move || run_live_session(app_clone, active, stop_flag));

    if let Some(win) = app.get_webview_window("main") {
        win.emit("live-transcript:started", ()).ok();
    }

    Ok(())
}

#[tauri::command]
pub fn cmd_stop_live_transcript(state: tauri::State<AppState>) -> Result<(), String> {
    if state.live_transcript_active.load(Ordering::Relaxed) {
        state
            .live_transcript_stop_flag
            .store(true, Ordering::Relaxed);
        return Ok(());
    }
    // Check for external live transcript (started from CLI)
    let lt_pid = minutes_core::pid::live_transcript_pid_path();
    if let Ok(Some(pid)) = minutes_core::pid::check_pid_file(&lt_pid) {
        minutes_core::pid::write_stop_sentinel()
            .map_err(|e| format!("failed to write stop sentinel: {}", e))?;
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        return Ok(());
    }
    Err("No live transcript session active".into())
}

#[tauri::command]
pub fn cmd_live_transcript_status(state: tauri::State<AppState>) -> serde_json::Value {
    let in_app_active = state.live_transcript_active.load(Ordering::Relaxed);
    let status = minutes_core::live_transcript::session_status();
    let audio_level = if in_app_active {
        minutes_core::streaming::stream_audio_level()
    } else {
        0
    };
    serde_json::json!({
        "active": in_app_active || status.active,
        "line_count": status.line_count,
        "duration_secs": status.duration_secs,
        "audioLevel": audio_level,
    })
}

/// Update the CLAUDE.md in the assistant workspace to mention (or un-mention)
/// the live transcript. This makes any agent (Claude, Codex, Gemini) aware
/// of the live JSONL file without requiring MCP.
pub fn handle_live_shortcut_event(
    app: &tauri::AppHandle,
    shortcut_state: tauri_plugin_global_shortcut::ShortcutState,
) {
    let state = app.state::<AppState>();
    if !state.live_shortcut_enabled.load(Ordering::Relaxed) {
        return;
    }
    if shortcut_state != tauri_plugin_global_shortcut::ShortcutState::Pressed {
        return;
    }

    // Toggle: if active, stop. If idle, start.
    if state.live_transcript_active.load(Ordering::Relaxed) {
        state
            .live_transcript_stop_flag
            .store(true, Ordering::Relaxed);
    } else if try_acquire_live(&state).is_ok() {
        let active = state.live_transcript_active.clone();
        let stop_flag = state.live_transcript_stop_flag.clone();
        stop_flag.store(false, Ordering::Relaxed);
        let app_clone = app.clone();
        std::thread::spawn(move || run_live_session(app_clone, active, stop_flag));
        if let Some(win) = app.get_webview_window("main") {
            win.emit("live-transcript:started", ()).ok();
        }
    }
    // else: conflicting mode, silently ignore (shortcut is best-effort)
}

#[tauri::command]
pub fn cmd_live_shortcut_settings(state: tauri::State<AppState>) -> HotkeySettings {
    let enabled = state.live_shortcut_enabled.load(Ordering::Relaxed);
    let shortcut = state
        .live_shortcut
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|_| "CmdOrCtrl+Shift+L".into());
    HotkeySettings {
        enabled,
        shortcut,
        choices: vec![],
    }
}

#[tauri::command]
pub fn cmd_set_live_shortcut(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    enabled: bool,
    shortcut: String,
) -> Result<HotkeySettings, String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let next_shortcut = validate_hotkey_shortcut(&shortcut)?;
    let previous = cmd_live_shortcut_settings(state.clone());
    let manager = app.global_shortcut();

    if previous.enabled {
        manager
            .unregister(previous.shortcut.as_str())
            .map_err(|e| format!("Could not unregister {}: {}", previous.shortcut, e))?;
    }

    if enabled {
        if let Err(e) = manager.register(next_shortcut.as_str()) {
            if previous.enabled {
                let _ = manager.register(previous.shortcut.as_str());
            }
            return Err(format!(
                "Could not register {}. Another app may already be using it. ({})",
                next_shortcut, e
            ));
        }
    }

    state
        .live_shortcut_enabled
        .store(enabled, Ordering::Relaxed);
    if let Ok(mut current) = state.live_shortcut.lock() {
        *current = next_shortcut.clone();
    }

    // Persist to config.toml
    cmd_set_setting(
        "live_transcript".into(),
        "shortcut_enabled".into(),
        enabled.to_string(),
    )
    .ok();
    cmd_set_setting("live_transcript".into(), "shortcut".into(), next_shortcut).ok();

    Ok(cmd_live_shortcut_settings(state))
}

fn update_assistant_live_context(workspace: &std::path::Path, live_active: bool) {
    let claude_md = workspace.join("CLAUDE.md");
    let existing = std::fs::read_to_string(&claude_md).unwrap_or_default();

    let marker_start = "<!-- LIVE_TRANSCRIPT_START -->";
    let marker_end = "<!-- LIVE_TRANSCRIPT_END -->";

    // Remove any existing live transcript section (T4: validate marker order)
    let cleaned = if let (Some(start), Some(end)) =
        (existing.find(marker_start), existing.find(marker_end))
    {
        if start < end {
            let end_pos = end + marker_end.len();
            format!("{}{}", &existing[..start], &existing[end_pos..])
        } else {
            // Markers out of order (corrupt file). Remove both markers individually.
            existing.replace(marker_start, "").replace(marker_end, "")
        }
    } else {
        // Remove any orphaned single marker
        existing.replace(marker_start, "").replace(marker_end, "")
    };

    let updated = if live_active {
        let jsonl_path = minutes_core::pid::live_transcript_jsonl_path();
        let section = format!(
            "\n{marker_start}\n\
            ## Live Transcript Active\n\
            \n\
            A live meeting transcript is being recorded right now.\n\
            \n\
            **JSONL file:** `{path}`\n\
            \n\
            Each line is a JSON object with: `line` (sequence number), `ts` (wall clock), \
            `offset_ms` (ms since session start), `duration_ms`, `text`, `speaker` (null for now).\n\
            \n\
            To read the latest utterances:\n\
            - **File:** `cat {path} | tail -5` (last 5 utterances)\n\
            - **CLI:** `minutes transcript --since 5m` (last 5 minutes)\n\
            - **MCP:** Use `read_live_transcript` tool with `since: \"5m\"`\n\
            \n\
            The user may ask for coaching during the meeting. Read the recent transcript \
            to understand what's being discussed, then provide tactical advice.\n\
            {marker_end}\n",
            marker_start = marker_start,
            marker_end = marker_end,
            path = jsonl_path.display(),
        );
        format!("{}{}", cleaned.trim_end(), section)
    } else {
        cleaned
    };

    // Atomic write: write to temp file then rename (T7)
    let content = updated.trim_end().to_string() + "\n";
    let tmp = claude_md.with_extension("md.tmp");
    if std::fs::write(&tmp, &content).is_ok() {
        std::fs::rename(&tmp, &claude_md).ok();
    }
}

// ── Native hotkey for dictation (macOS only) ─────────────────

#[cfg(target_os = "macos")]
use std::sync::{LazyLock, Mutex as StdMutex, MutexGuard as StdMutexGuard};

#[derive(Debug, Clone, serde::Serialize)]
pub struct DictationHotkeyStatus {
    pub state: String,
    pub enabled: bool,
    pub pending: bool,
    pub keycode: i64,
    pub message: String,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DictationHotkeyLifecycle {
    Disabled,
    Starting,
    Active,
    Failed,
}

#[cfg(target_os = "macos")]
struct DictationHotkeyRuntime {
    generation: u64,
    keycode: i64,
    lifecycle: DictationHotkeyLifecycle,
    last_error: Option<String>,
    monitor: Option<minutes_core::hotkey_macos::HotkeyMonitor>,
    key_down: bool,
    key_down_started_at: Option<Instant>,
    active_capture: Option<HotkeyCaptureStyle>,
    hold_generation: u64,
}

#[cfg(target_os = "macos")]
impl Default for DictationHotkeyRuntime {
    fn default() -> Self {
        Self {
            generation: 0,
            keycode: minutes_core::hotkey_macos::KEYCODE_CAPS_LOCK,
            lifecycle: DictationHotkeyLifecycle::Disabled,
            last_error: None,
            monitor: None,
            key_down: false,
            key_down_started_at: None,
            active_capture: None,
            hold_generation: 0,
        }
    }
}

#[cfg(target_os = "macos")]
static DICTATION_HOTKEY_RUNTIME: LazyLock<StdMutex<DictationHotkeyRuntime>> =
    LazyLock::new(|| StdMutex::new(DictationHotkeyRuntime::default()));

#[cfg(target_os = "macos")]
fn lock_dictation_hotkey_runtime() -> StdMutexGuard<'static, DictationHotkeyRuntime> {
    DICTATION_HOTKEY_RUNTIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(not(target_os = "macos"))]
fn dictation_hotkey_status_for_other_platform() -> DictationHotkeyStatus {
    DictationHotkeyStatus {
        state: "unsupported".into(),
        enabled: false,
        pending: false,
        keycode: 57,
        message:
            "Native dictation hotkey is currently available on macOS only. Use the CLI or MCP dictation flow on this platform for now."
                .into(),
    }
}

#[cfg(target_os = "macos")]
fn build_dictation_hotkey_status(runtime: &DictationHotkeyRuntime) -> DictationHotkeyStatus {
    let state = match runtime.lifecycle {
        DictationHotkeyLifecycle::Disabled => "disabled",
        DictationHotkeyLifecycle::Starting => "starting",
        DictationHotkeyLifecycle::Active => "active",
        DictationHotkeyLifecycle::Failed => "failed",
    }
    .to_string();

    let message = match runtime.lifecycle {
        DictationHotkeyLifecycle::Disabled => {
            "Hold the selected key to dictate, or tap to lock and tap again to stop. Requires Input Monitoring permission.".to_string()
        }
        DictationHotkeyLifecycle::Starting => "Starting native dictation hotkey...".to_string(),
        DictationHotkeyLifecycle::Active => {
            "Active - hold the selected key to dictate, or tap to lock and tap again to stop.".to_string()
        }
        DictationHotkeyLifecycle::Failed => runtime
            .last_error
            .clone()
            .unwrap_or_else(|| "Could not start the native dictation hotkey.".to_string()),
    };

    DictationHotkeyStatus {
        enabled: matches!(runtime.lifecycle, DictationHotkeyLifecycle::Active),
        pending: matches!(runtime.lifecycle, DictationHotkeyLifecycle::Starting),
        state,
        keycode: runtime.keycode,
        message,
    }
}

#[cfg(target_os = "macos")]
fn current_dictation_hotkey_status() -> DictationHotkeyStatus {
    let runtime = lock_dictation_hotkey_runtime();
    build_dictation_hotkey_status(&runtime)
}

#[cfg(target_os = "macos")]
fn emit_dictation_hotkey_status(app: &tauri::AppHandle) {
    let status = current_dictation_hotkey_status();
    app.emit("dictation-hotkey:status", &status).ok();
}

pub(crate) fn dictation_pid_active() -> bool {
    minutes_core::pid::check_pid_file(&minutes_core::pid::dictation_pid_path())
        .ok()
        .flatten()
        .is_some()
}

#[cfg(target_os = "macos")]
fn clear_dictation_hotkey_capture_state(runtime: &mut DictationHotkeyRuntime) {
    runtime.key_down = false;
    runtime.key_down_started_at = None;
    runtime.active_capture = None;
}

/// Public entry point for the shortcut manager to start a dictation session.
pub fn start_dictation_session_public(
    app: &tauri::AppHandle,
    capture_style: Option<HotkeyCaptureStyle>,
) -> Result<(), String> {
    start_dictation_session(app, capture_style).map(|_| ())
}

fn start_dictation_session(
    app: &tauri::AppHandle,
    capture_style: Option<HotkeyCaptureStyle>,
) -> Result<String, String> {
    let state = app.state::<AppState>();

    if state.recording.load(Ordering::Relaxed) {
        return Err("Recording in progress — stop recording before dictating".into());
    }

    if state.dictation_active.load(Ordering::Relaxed) || dictation_pid_active() {
        return Err("Dictation is already in progress.".into());
    }

    show_dictation_overlay(app);
    app.emit("dictation:state", "loading").ok();

    state.dictation_stop_flag.store(false, Ordering::Relaxed);
    state.dictation_active.store(true, Ordering::Relaxed);

    #[cfg(target_os = "macos")]
    if let Some(style) = capture_style {
        let mut runtime = lock_dictation_hotkey_runtime();
        runtime.active_capture = Some(style);
    }

    let app_clone = app.clone();
    let stop_flag = Arc::clone(&state.dictation_stop_flag);
    let dictation_active = Arc::clone(&state.dictation_active);

    std::thread::spawn(move || {
        let config = Config::load();
        let app_for_events = app_clone.clone();
        let app_for_results = app_clone.clone();

        let result = minutes_core::dictation::run(
            stop_flag,
            &config,
            move |event| {
                use minutes_core::dictation::DictationEvent;
                let state_str = match &event {
                    DictationEvent::Listening => "listening",
                    DictationEvent::Accumulating => "accumulating",
                    DictationEvent::Processing => "processing",
                    DictationEvent::PartialText(_) => "partial",
                    DictationEvent::SilenceCountdown { .. } => "",
                    DictationEvent::Success => "success",
                    DictationEvent::Error => "error",
                    DictationEvent::Cancelled => "cancelled",
                    DictationEvent::Yielded => "yielded",
                };
                if !state_str.is_empty() {
                    app_for_events.emit("dictation:state", state_str).ok();
                }

                if let DictationEvent::PartialText(text) = &event {
                    app_for_events.emit("dictation:partial", text.as_str()).ok();
                }

                if let DictationEvent::SilenceCountdown {
                    total_ms,
                    remaining_ms,
                } = &event
                {
                    app_for_events
                        .emit(
                            "dictation:silence",
                            serde_json::json!({
                                "total_ms": total_ms,
                                "remaining_ms": remaining_ms,
                            }),
                        )
                        .ok();
                }

                if matches!(
                    &event,
                    DictationEvent::Accumulating | DictationEvent::PartialText(_)
                ) {
                    let level = minutes_core::streaming::stream_audio_level();
                    app_for_events.emit("dictation:level", level).ok();
                }
            },
            move |result| {
                app_for_results.emit("dictation:result", &result.text).ok();
            },
        );

        dictation_active.store(false, Ordering::Relaxed);
        #[cfg(target_os = "macos")]
        {
            let mut runtime = lock_dictation_hotkey_runtime();
            clear_dictation_hotkey_capture_state(&mut runtime);
        }

        match result {
            Ok(()) => {
                // Session ended normally (silence timeout or yield).
                // Dismiss overlay if it wasn't already dismissed by a terminal event.
                app_clone.emit("dictation:state", "cancelled").ok();
            }
            Err(e) => {
                eprintln!("[dictation] error: {}", e);
                app_clone.emit("dictation:state", "error").ok();
            }
        }
    });

    Ok("Dictation started".into())
}

#[cfg(target_os = "macos")]
pub fn start_dictation_hotkey_with_keycode(
    app: tauri::AppHandle,
    keycode: i64,
) -> Result<DictationHotkeyStatus, String> {
    use minutes_core::hotkey_macos::{HotkeyEvent, HotkeyMonitor, HotkeyMonitorStatus};

    let previous_monitor = {
        let mut runtime = lock_dictation_hotkey_runtime();
        runtime.generation = runtime.generation.wrapping_add(1);
        runtime.keycode = keycode;
        runtime.lifecycle = DictationHotkeyLifecycle::Starting;
        runtime.last_error = None;
        clear_dictation_hotkey_capture_state(&mut runtime);
        runtime.monitor.take()
    };
    if let Some(monitor) = previous_monitor {
        monitor.stop();
    }
    emit_dictation_hotkey_status(&app);

    let generation = {
        let runtime = lock_dictation_hotkey_runtime();
        runtime.generation
    };

    let app_for_status = app.clone();
    let app_for_events = app.clone();
    let monitor = match HotkeyMonitor::start(
        keycode,
        move |event| match event {
            HotkeyEvent::Press => {
                minutes_core::logging::append_log(&serde_json::json!({
                    "ts": chrono::Local::now().to_rfc3339(),
                    "level": "info",
                    "step": "dictation_hotkey_event",
                    "file": "",
                    "extra": {
                        "event": "press",
                        "keycode": keycode,
                    }
                }))
                .ok();
                let generation = {
                    let mut runtime = lock_dictation_hotkey_runtime();
                    if runtime.key_down {
                        minutes_core::logging::append_log(&serde_json::json!({
                            "ts": chrono::Local::now().to_rfc3339(),
                            "level": "info",
                            "step": "dictation_hotkey_skip",
                            "file": "",
                            "extra": {
                                "reason": "key_already_down",
                                "keycode": keycode,
                            }
                        }))
                        .ok();
                        return;
                    }
                    runtime.key_down = true;
                    runtime.key_down_started_at = Some(Instant::now());
                    runtime.hold_generation = runtime.hold_generation.wrapping_add(1);
                    runtime.hold_generation
                };

                let app_for_hold = app_for_events.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(HOTKEY_HOLD_THRESHOLD_MS));
                    let should_start_hold = {
                        let runtime = lock_dictation_hotkey_runtime();
                        runtime.key_down
                            && runtime.hold_generation == generation
                            && runtime.active_capture.is_none()
                    };
                    if !should_start_hold {
                        minutes_core::logging::append_log(&serde_json::json!({
                            "ts": chrono::Local::now().to_rfc3339(),
                            "level": "info",
                            "step": "dictation_hotkey_skip",
                            "file": "",
                            "extra": {
                                "reason": "hold_threshold_not_met",
                                "keycode": keycode,
                            }
                        }))
                        .ok();
                        return;
                    }
                    minutes_core::logging::append_log(&serde_json::json!({
                        "ts": chrono::Local::now().to_rfc3339(),
                        "level": "info",
                        "step": "dictation_hotkey_action",
                        "file": "",
                        "extra": {
                            "action": "start_hold",
                            "keycode": keycode,
                        }
                    }))
                    .ok();
                    if let Err(error) =
                        start_dictation_session(&app_for_hold, Some(HotkeyCaptureStyle::Hold))
                    {
                        show_user_notification(&app_for_hold, "Dictation", &error);
                    }
                });
            }
            HotkeyEvent::Release => {
                minutes_core::logging::append_log(&serde_json::json!({
                    "ts": chrono::Local::now().to_rfc3339(),
                    "level": "info",
                    "step": "dictation_hotkey_event",
                    "file": "",
                    "extra": {
                        "event": "release",
                        "keycode": keycode,
                    }
                }))
                .ok();
                let now = Instant::now();
                let (active_capture, was_short_tap) = {
                    let mut runtime = lock_dictation_hotkey_runtime();
                    let pressed_at = runtime.key_down_started_at;
                    runtime.key_down = false;
                    runtime.key_down_started_at = None;
                    (runtime.active_capture, is_short_hotkey_tap(pressed_at, now))
                };

                if matches!(active_capture, Some(HotkeyCaptureStyle::Hold)) {
                    minutes_core::logging::append_log(&serde_json::json!({
                        "ts": chrono::Local::now().to_rfc3339(),
                        "level": "info",
                        "step": "dictation_hotkey_action",
                        "file": "",
                        "extra": {
                            "action": "stop_hold",
                            "keycode": keycode,
                        }
                    }))
                    .ok();
                    if let Some(state) = app_for_events.try_state::<AppState>() {
                        state.dictation_stop_flag.store(true, Ordering::Relaxed);
                    }
                    return;
                }

                if !was_short_tap {
                    minutes_core::logging::append_log(&serde_json::json!({
                        "ts": chrono::Local::now().to_rfc3339(),
                        "level": "info",
                        "step": "dictation_hotkey_skip",
                        "file": "",
                        "extra": {
                            "reason": "release_without_short_tap",
                            "keycode": keycode,
                        }
                    }))
                    .ok();
                    return;
                }

                if let Some(state) = app_for_events.try_state::<AppState>() {
                    if state.dictation_active.load(Ordering::Relaxed) {
                        minutes_core::logging::append_log(&serde_json::json!({
                            "ts": chrono::Local::now().to_rfc3339(),
                            "level": "info",
                            "step": "dictation_hotkey_action",
                            "file": "",
                            "extra": {
                                "action": "stop_locked",
                                "keycode": keycode,
                            }
                        }))
                        .ok();
                        state.dictation_stop_flag.store(true, Ordering::Relaxed);
                        return;
                    }
                }

                if dictation_pid_active() {
                    minutes_core::logging::append_log(&serde_json::json!({
                        "ts": chrono::Local::now().to_rfc3339(),
                        "level": "info",
                        "step": "dictation_hotkey_skip",
                        "file": "",
                        "extra": {
                            "reason": "dictation_pid_active",
                            "keycode": keycode,
                        }
                    }))
                    .ok();
                    return;
                }

                minutes_core::logging::append_log(&serde_json::json!({
                    "ts": chrono::Local::now().to_rfc3339(),
                    "level": "info",
                    "step": "dictation_hotkey_action",
                    "file": "",
                    "extra": {
                        "action": "start_locked",
                        "keycode": keycode,
                    }
                }))
                .ok();
                if let Err(error) =
                    start_dictation_session(&app_for_events, Some(HotkeyCaptureStyle::Locked))
                {
                    show_user_notification(&app_for_events, "Dictation", &error);
                }
            }
        },
        move |status| {
            let (should_prompt, should_emit) = {
                let mut runtime = lock_dictation_hotkey_runtime();
                if runtime.generation != generation {
                    return;
                }
                runtime.keycode = keycode;
                match status {
                    HotkeyMonitorStatus::Starting => {
                        runtime.lifecycle = DictationHotkeyLifecycle::Starting;
                        runtime.last_error = None;
                        minutes_core::logging::append_log(&serde_json::json!({
                            "ts": chrono::Local::now().to_rfc3339(),
                            "level": "info",
                            "step": "dictation_hotkey_status",
                            "file": "",
                            "extra": {
                                "state": "starting",
                                "keycode": keycode,
                            }
                        }))
                        .ok();
                        (false, true)
                    }
                    HotkeyMonitorStatus::Active => {
                        runtime.lifecycle = DictationHotkeyLifecycle::Active;
                        runtime.last_error = None;
                        minutes_core::logging::append_log(&serde_json::json!({
                            "ts": chrono::Local::now().to_rfc3339(),
                            "level": "info",
                            "step": "dictation_hotkey_status",
                            "file": "",
                            "extra": {
                                "state": "active",
                                "keycode": keycode,
                            }
                        }))
                        .ok();
                        (false, true)
                    }
                    HotkeyMonitorStatus::Failed(message) => {
                        runtime.lifecycle = DictationHotkeyLifecycle::Failed;
                        runtime.last_error = Some(message);
                        runtime.monitor = None;
                        minutes_core::logging::append_log(&serde_json::json!({
                            "ts": chrono::Local::now().to_rfc3339(),
                            "level": "error",
                            "step": "dictation_hotkey_status",
                            "file": "",
                            "error": runtime.last_error,
                            "extra": {
                                "state": "failed",
                                "keycode": keycode,
                            }
                        }))
                        .ok();
                        (true, true)
                    }
                    HotkeyMonitorStatus::Stopped => {
                        runtime.lifecycle = DictationHotkeyLifecycle::Disabled;
                        runtime.last_error = None;
                        clear_dictation_hotkey_capture_state(&mut runtime);
                        runtime.monitor = None;
                        minutes_core::logging::append_log(&serde_json::json!({
                            "ts": chrono::Local::now().to_rfc3339(),
                            "level": "info",
                            "step": "dictation_hotkey_status",
                            "file": "",
                            "extra": {
                                "state": "stopped",
                                "keycode": keycode,
                            }
                        }))
                        .ok();
                        (false, true)
                    }
                }
            };
            if should_prompt {
                minutes_core::hotkey_macos::prompt_accessibility_permission();
            }
            if should_emit {
                emit_dictation_hotkey_status(&app_for_status);
            }
        },
    ) {
        Ok(monitor) => monitor,
        Err(error) => {
            {
                let mut runtime = lock_dictation_hotkey_runtime();
                if runtime.generation == generation {
                    runtime.lifecycle = DictationHotkeyLifecycle::Failed;
                    runtime.last_error = Some(error.clone());
                    runtime.monitor = None;
                }
            }
            emit_dictation_hotkey_status(&app);
            return Err(error);
        }
    };

    let mut monitor_slot = Some(monitor);
    {
        let mut runtime = lock_dictation_hotkey_runtime();
        if runtime.generation == generation
            && !matches!(runtime.lifecycle, DictationHotkeyLifecycle::Failed)
        {
            runtime.monitor = monitor_slot.take();
        }
    }
    if let Some(monitor) = monitor_slot {
        monitor.stop();
    }

    Ok(current_dictation_hotkey_status())
}

/// Stop the native dictation hotkey monitor.
#[cfg(target_os = "macos")]
pub fn stop_dictation_hotkey() {
    let monitor = {
        let mut runtime = lock_dictation_hotkey_runtime();
        runtime.generation = runtime.generation.wrapping_add(1);
        runtime.lifecycle = DictationHotkeyLifecycle::Disabled;
        runtime.last_error = None;
        clear_dictation_hotkey_capture_state(&mut runtime);
        runtime.monitor.take()
    };
    if let Some(monitor) = monitor {
        monitor.stop();
    }
}

#[tauri::command]
pub fn cmd_enable_dictation_hotkey(
    app: tauri::AppHandle,
    enabled: bool,
    keycode: Option<i64>,
) -> Result<DictationHotkeyStatus, String> {
    #[cfg(target_os = "macos")]
    {
        if enabled {
            let kc = keycode.unwrap_or(minutes_core::hotkey_macos::KEYCODE_CAPS_LOCK);
            start_dictation_hotkey_with_keycode(app, kc)
        } else {
            stop_dictation_hotkey();
            emit_dictation_hotkey_status(&app);
            Ok(current_dictation_hotkey_status())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, enabled, keycode);
        Err(dictation_hotkey_status_for_other_platform().message)
    }
}

#[tauri::command]
pub fn cmd_dictation_hotkey_status() -> DictationHotkeyStatus {
    #[cfg(target_os = "macos")]
    {
        current_dictation_hotkey_status()
    }
    #[cfg(not(target_os = "macos"))]
    {
        dictation_hotkey_status_for_other_platform()
    }
}

#[tauri::command]
pub fn cmd_check_accessibility() -> serde_json::Value {
    #[cfg(target_os = "macos")]
    {
        let trusted = minutes_core::hotkey_macos::is_accessibility_trusted();
        serde_json::json!({
            "trusted": trusted,
            "platform": "macos",
            "note": "Accessibility status only. The native dictation hotkey still requires Input Monitoring."
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        serde_json::json!({
            "trusted": true,
            "platform": current_platform(),
            "note": "Accessibility checks are only relevant to the macOS dictation hotkey."
        })
    }
}

#[tauri::command]
pub fn cmd_request_accessibility() -> String {
    #[cfg(target_os = "macos")]
    {
        minutes_core::hotkey_macos::prompt_accessibility_permission();
        "Input Monitoring settings opened".into()
    }
    #[cfg(not(target_os = "macos"))]
    {
        "Accessibility settings are only used for the macOS dictation hotkey.".into()
    }
}

// ── Unified Shortcut Commands ────────────────────────────────

#[tauri::command]
pub fn cmd_set_shortcut(
    app: tauri::AppHandle,
    slot: String,
    enabled: bool,
    shortcut: String,
    keycode: i64,
) -> Result<crate::shortcut_manager::ShortcutStatus, String> {
    use crate::shortcut_manager::{ShortcutManager, ShortcutSlot};

    let slot = ShortcutSlot::from_str(&slot)?;

    // Validate shortcut string
    if shortcut.len() > 50 {
        return Err("Shortcut string too long (max 50 characters)".into());
    }
    if !shortcut.is_empty()
        && !shortcut
            .chars()
            .all(|c| c.is_alphanumeric() || "+_ ".contains(c))
    {
        return Err(format!("Invalid characters in shortcut: {}", shortcut));
    }

    // Validate keycode range
    if !(-1..=255).contains(&keycode) {
        return Err(format!("Invalid keycode: {}", keycode));
    }

    // Acquire lock, perform registration/unregistration, then DROP before file I/O.
    let status = {
        let mgr_state = app.state::<std::sync::Arc<std::sync::Mutex<ShortcutManager>>>();
        let mut mgr = mgr_state
            .lock()
            .map_err(|_| "Shortcut manager lock poisoned".to_string())?;

        if enabled {
            mgr.register(slot, shortcut.clone(), keycode, &app)?
        } else {
            mgr.unregister(slot, &app)?;
            let mut s = mgr.build_status(slot);
            // Preserve the shortcut choice in status even when disabling
            if !shortcut.is_empty() {
                s.shortcut = shortcut.clone();
                s.keycode = keycode;
            }
            s
        }
    }; // lock dropped here

    if enabled {
        // Persist to config (no lock held)
        let mut config = Config::load();
        match slot {
            ShortcutSlot::Dictation => {
                config.dictation.shortcut_enabled = true;
                config.dictation.shortcut = status.shortcut.clone();
                let backend = crate::shortcut_manager::classify_shortcut(keycode);
                if backend == crate::shortcut_manager::ShortcutBackend::Native {
                    config.dictation.hotkey_enabled = true;
                    config.dictation.hotkey_keycode = keycode;
                } else {
                    config.dictation.hotkey_enabled = false;
                }
            }
            ShortcutSlot::QuickThought => {}
        }
        config
            .save()
            .map_err(|e| format!("Failed to save config: {}", e))?;

        // Preload model when dictation is first enabled
        if matches!(slot, ShortcutSlot::Dictation) {
            let config = Config::load();
            std::thread::spawn(move || {
                minutes_core::dictation::preload_model(&config).ok();
            });
        }

        Ok(status)
    } else {
        // Persist disabled state but keep the shortcut/keycode for later re-enable
        let mut config = Config::load();
        match slot {
            ShortcutSlot::Dictation => {
                config.dictation.shortcut_enabled = false;
                config.dictation.hotkey_enabled = false;
                if !shortcut.is_empty() {
                    let backend = crate::shortcut_manager::classify_shortcut(keycode);
                    if backend == crate::shortcut_manager::ShortcutBackend::Native {
                        config.dictation.hotkey_keycode = keycode;
                    } else {
                        config.dictation.shortcut = shortcut;
                    }
                }
            }
            ShortcutSlot::QuickThought => {}
        }
        config
            .save()
            .map_err(|e| format!("Failed to save config: {}", e))?;

        Ok(status)
    }
}

#[tauri::command]
pub fn cmd_shortcut_status(
    app: tauri::AppHandle,
    slot: String,
) -> Result<crate::shortcut_manager::ShortcutStatus, String> {
    use crate::shortcut_manager::{ShortcutManager, ShortcutSlot};

    let slot = ShortcutSlot::from_str(&slot)?;
    let mgr_state = app.state::<std::sync::Arc<std::sync::Mutex<ShortcutManager>>>();
    let mgr = mgr_state
        .lock()
        .map_err(|_| "Shortcut manager lock poisoned".to_string())?;
    Ok(mgr.build_status(slot))
}

#[tauri::command]
pub fn cmd_suspend_shortcut(app: tauri::AppHandle, slot: String) -> Result<(), String> {
    use crate::shortcut_manager::{ShortcutManager, ShortcutSlot};
    let slot = ShortcutSlot::from_str(&slot)?;
    let mgr_state = app.state::<std::sync::Arc<std::sync::Mutex<ShortcutManager>>>();
    let mut mgr = mgr_state
        .lock()
        .map_err(|_| "Shortcut manager lock poisoned".to_string())?;
    mgr.unregister(slot, &app)?;
    Ok(())
}

#[tauri::command]
pub fn cmd_probe_shortcut(keycode: i64) -> serde_json::Value {
    let backend = crate::shortcut_manager::classify_shortcut(keycode);
    let needs_native = backend == crate::shortcut_manager::ShortcutBackend::Native;

    let permission_granted = if needs_native {
        #[cfg(target_os = "macos")]
        {
            minutes_core::hotkey_macos::is_input_monitoring_granted()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    } else {
        true // Standard backend needs no permission
    };

    serde_json::json!({
        "keycode": keycode,
        "backend": if needs_native { "native" } else { "standard" },
        "needs_permission": needs_native && !permission_granted,
        "permission_granted": permission_granted,
        "supported": !needs_native || cfg!(target_os = "macos"),
    })
}

#[tauri::command]
pub async fn cmd_install_update(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    use tauri_plugin_updater::UpdaterExt;

    // Block restart if any recording/processing activity is in progress
    let state = app.state::<AppState>();
    if state.recording.load(Ordering::Relaxed) {
        return Err("Cannot update while recording. Stop the recording first.".into());
    }
    if state.starting.load(Ordering::Relaxed) {
        return Err("Recording is starting. Wait a moment and try again.".into());
    }
    if state.processing.load(Ordering::Relaxed) {
        return Err("Processing a recording. Wait until it finishes.".into());
    }
    if state.live_transcript_active.load(Ordering::Relaxed) {
        return Err("Cannot update during live transcription. Stop it first.".into());
    }
    if state.dictation_active.load(Ordering::Relaxed) {
        return Err("Cannot update during dictation. Stop it first.".into());
    }

    // Download and install (the background checker only checked, not downloaded)
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No update available.".to_string())?;

    let version = update.version.clone();
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| format!("Update failed: {}", e))?;

    // Clear pending update state
    if let Ok(mut pending) = state.pending_update.lock() {
        *pending = None;
    }

    eprintln!("[updater] v{} installed, restarting", version);
    app.restart();

    #[allow(unreachable_code)]
    Ok(serde_json::json!({"restarting": true}))
}

// ─────────────────────────────────────────────────────────────────────
// Command palette window management
// ─────────────────────────────────────────────────────────────────────

/// Global-shortcut handler for the palette toggle (`⌘⇧K` by default).
///
/// Reacts to `Pressed` only. The palette is a toggle on press, not a
/// hold-to-talk, so `Released` is ignored. Routes through the
/// lifecycle-aware `toggle_palette_window` helper to survive fast
/// double-press races.
pub fn handle_palette_shortcut_event(
    app: &tauri::AppHandle,
    shortcut_state: tauri_plugin_global_shortcut::ShortcutState,
) {
    if shortcut_state != tauri_plugin_global_shortcut::ShortcutState::Pressed {
        return;
    }
    let state = app.state::<AppState>();
    if !state.palette_shortcut_enabled.load(Ordering::Relaxed) {
        return;
    }
    toggle_palette_window(app);
}

/// Toggle the palette overlay window based on the current lifecycle state.
///
/// The state machine:
/// - `Closed`  → `Opening` → build window → `Open`
/// - `Open`    → `Closing` → close window → `Closed`
/// - `Opening` → ignore (duplicate press mid-create)
/// - `Closing` → queue a reopen; when close completes, transition
///   `Closed → Opening` immediately
///
/// All transitions happen under a `Mutex`, so two concurrent hotkey
/// handlers (e.g., plugin dispatch on a worker thread) cannot race each
/// other. The window itself is created on the main thread via
/// `app.run_on_main_thread`.
pub fn toggle_palette_window(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();

    let transition: Option<PaletteTransition> = {
        let mut lifecycle = match state.palette_lifecycle.lock() {
            Ok(guard) => guard,
            Err(_) => {
                eprintln!("[palette] palette_lifecycle mutex poisoned; dropping hotkey");
                return;
            }
        };
        match *lifecycle {
            PaletteLifecycle::Closed => {
                *lifecycle = PaletteLifecycle::Opening;
                Some(PaletteTransition::Open)
            }
            PaletteLifecycle::Open => {
                *lifecycle = PaletteLifecycle::Closing;
                Some(PaletteTransition::Close)
            }
            PaletteLifecycle::Opening => None,
            PaletteLifecycle::Closing => {
                state.palette_reopen_pending.store(true, Ordering::Relaxed);
                None
            }
        }
    };

    match transition {
        Some(PaletteTransition::Open) => create_or_show_palette_window(app),
        Some(PaletteTransition::Close) => close_palette_window(app),
        None => {}
    }
}

#[derive(Debug)]
enum PaletteTransition {
    Open,
    Close,
}

/// Close the palette window and drain any queued reopen request. Both
/// `palette_close` (invoked from the webview's Esc key and focus-lost
/// handlers) and the shortcut-toggle path funnel through here.
pub fn close_palette_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("palette") {
        if let Err(e) = win.close() {
            eprintln!("[palette] failed to close palette window: {}", e);
        }
    }

    let state = app.state::<AppState>();
    let reopen = {
        let mut lifecycle = match state.palette_lifecycle.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        *lifecycle = PaletteLifecycle::Closed;
        state.palette_reopen_pending.swap(false, Ordering::Relaxed)
    };

    if reopen {
        let mut lifecycle = match state.palette_lifecycle.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        if *lifecycle == PaletteLifecycle::Closed {
            *lifecycle = PaletteLifecycle::Opening;
            drop(lifecycle);
            create_or_show_palette_window(app);
        }
    }
}

/// Public Tauri command wrapping [`close_palette_window`]. Called from
/// the palette frontend's Esc and focus-lost handlers so the state
/// machine stays consistent no matter which event triggered the close.
#[tauri::command]
pub fn palette_close(app: tauri::AppHandle) {
    close_palette_window(&app);
}

fn create_or_show_palette_window(app: &tauri::AppHandle) {
    // Wrap the entire create-or-show path in `catch_unwind` so a panic
    // inside `WebviewWindowBuilder::build()` (or any of the helper
    // calls below) cannot leave `palette_lifecycle` stuck in `Opening`
    // forever. This was codex pass 2 P2 #5: the only reset path used
    // to be the explicit `Err` arm after `.build()`, so an unwinding
    // panic would skip the reset and the user could never reopen the
    // palette without restarting the app.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        create_or_show_palette_window_inner(app)
    }));
    if let Err(panic) = result {
        eprintln!("[palette] window creation panicked: {:?}", panic);
        let state = app.state::<AppState>();
        if let Ok(mut lifecycle) = state.palette_lifecycle.lock() {
            *lifecycle = PaletteLifecycle::Closed;
        };
    }
}

fn create_or_show_palette_window_inner(app: &tauri::AppHandle) {
    use tauri::WebviewUrl;

    // Singleton: a stale window from a previous toggle should be reused,
    // not duplicated. `get_webview_window` is cheap.
    if let Some(win) = app.get_webview_window("palette") {
        // The lifecycle says we are opening, but a window already exists.
        // Show + focus it instead of spawning a duplicate.
        if let Err(e) = win.show() {
            eprintln!("[palette] show failed: {}", e);
        }
        if let Err(e) = win.set_focus() {
            eprintln!("[palette] focus failed: {}", e);
        }
        finalize_palette_open(app);
        return;
    }

    // Position: center of the primary monitor. Tauri's `center()` builder
    // option handles multi-monitor setups correctly.
    let width = 640.0_f64;
    let height = 420.0_f64;

    let build_result = tauri::WebviewWindowBuilder::new(
        app,
        "palette",
        WebviewUrl::App("palette/index.html".into()),
    )
    .title("Minutes Palette")
    .inner_size(width, height)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(true)
    .always_on_top(true)
    .center()
    .focused(true)
    .skip_taskbar(true)
    .content_protected(true)
    .build();

    match build_result {
        Ok(_) => finalize_palette_open(app),
        Err(e) => {
            eprintln!("[palette] failed to build palette window: {}", e);
            let state = app.state::<AppState>();
            if let Ok(mut lifecycle) = state.palette_lifecycle.lock() {
                *lifecycle = PaletteLifecycle::Closed;
            };
        }
    }
}

fn finalize_palette_open(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(mut lifecycle) = state.palette_lifecycle.lock() {
        *lifecycle = PaletteLifecycle::Open;
    }

    // Capability smoke test (D4 of PLAN.md.command-palette-slice-2).
    //
    // Fire a `palette:ping` event to the palette window on a short
    // delay so the webview's `listen()` subscription has time to mount.
    // The palette HTML shows a green "events ok" indicator when it
    // receives this. If the indicator never turns green in dev-app
    // testing, capabilities/default.json needs an explicit `palette`
    // entry per the D4 fallback.
    let app_clone = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(120));
        if let Err(e) =
            app_clone.emit_to("palette", "palette:ping", serde_json::json!({ "ok": true }))
        {
            eprintln!("[palette] palette:ping emit failed: {}", e);
        }
    });
}

/// Read the assistant workspace's `CURRENT_MEETING.md` breadcrumb and
/// return the absolute path of the meeting the user is currently
/// discussing. Returns `None` if the file is missing, unreadable, or
/// does not reference a resolvable meeting path.
///
/// The palette webview calls this right before `palette_list` and
/// `palette_execute` so `PaletteUiContext.current_meeting` can be
/// populated for meeting-scoped commands (copy markdown, rename, etc.).
///
/// **Side-effect-free**: this command intentionally does NOT call
/// `crate::context::create_workspace` because that function does
/// `create_dir_all`, creates a `meetings` symlink, and runs `git init`.
/// Just opening the palette must not mutate `~/.minutes/assistant`.
/// Instead we use `workspace_dir()` (a pure path computation) and only
/// read the marker file if the workspace already exists. See codex
/// pass 2 P2 #3.
#[tauri::command]
pub fn palette_current_meeting() -> Option<PathBuf> {
    let workspace_root = crate::context::workspace_dir();
    if !workspace_root.exists() {
        return None;
    }
    let marker = workspace_root.join(crate::context::ACTIVE_MEETING_FILE);
    let contents = std::fs::read_to_string(&marker).ok()?;

    // CURRENT_MEETING.md stores a link or raw path to the current meeting
    // markdown. Accepted forms (pick the first matching line):
    //   1. Markdown link: `[title](/abs/path.md)`
    //   2. Bare path line: `/abs/path.md`
    //   3. `path: /abs/path.md` frontmatter-ish line
    // Anything else → `None`.
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(path) = extract_current_meeting_path(trimmed) {
            let candidate = PathBuf::from(path);
            if candidate.exists() && candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Parse a single line of `CURRENT_MEETING.md` looking for a path. Kept
/// private and tested directly so the accepted forms are documented.
fn extract_current_meeting_path(line: &str) -> Option<&str> {
    // Markdown link form: `[label](path)`
    if let Some(start) = line.find("](") {
        let rest = &line[start + 2..];
        if let Some(end) = rest.find(')') {
            let path = &rest[..end];
            if path.ends_with(".md") {
                return Some(path);
            }
        }
    }
    // `path: /abs/path.md` form
    if let Some(rest) = line.strip_prefix("path:") {
        let trimmed = rest.trim().trim_matches('"').trim_matches('\'');
        if trimmed.ends_with(".md") {
            return Some(trimmed);
        }
    }
    // Bare path form
    if line.ends_with(".md") && line.starts_with('/') {
        return Some(line);
    }
    None
}

#[cfg(test)]
mod palette_window_tests {
    use super::*;

    #[test]
    fn extracts_markdown_link_path() {
        assert_eq!(
            extract_current_meeting_path("[Team Sync](/Users/x/meetings/2026-04-07-team-sync.md)"),
            Some("/Users/x/meetings/2026-04-07-team-sync.md")
        );
    }

    #[test]
    fn extracts_path_prefix_form() {
        assert_eq!(
            extract_current_meeting_path("path: /Users/x/meetings/call.md"),
            Some("/Users/x/meetings/call.md")
        );
        assert_eq!(
            extract_current_meeting_path(r#"path: "/Users/x/meetings/call.md""#),
            Some("/Users/x/meetings/call.md")
        );
    }

    #[test]
    fn extracts_bare_absolute_path() {
        assert_eq!(
            extract_current_meeting_path("/Users/x/meetings/call.md"),
            Some("/Users/x/meetings/call.md")
        );
    }

    #[test]
    fn rejects_non_md_and_relative_paths() {
        assert_eq!(extract_current_meeting_path("relative/path.md"), None);
        assert_eq!(extract_current_meeting_path("/abs/path.txt"), None);
        assert_eq!(extract_current_meeting_path("just a sentence"), None);
    }
}
