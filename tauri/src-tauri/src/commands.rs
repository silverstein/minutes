use crate::call_capture;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures_util::StreamExt;
use minisign_verify::{PublicKey, Signature};
use minutes_core::capture::RecordingIntent;
use minutes_core::config::VALID_PARAKEET_MODELS;
use minutes_core::{CaptureMode, Config, ContentType};
use reqwest::header::{ACCEPT, CONTENT_LENGTH};
use std::cmp::Reverse;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
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
    pub activation_progress: Arc<Mutex<ActivationProgress>>,
    pub call_capture_health: Arc<Mutex<Option<crate::call_capture::CallSourceHealth>>>,
    pub completion_notifications_enabled: Arc<AtomicBool>,
    pub screen_share_hidden: Arc<AtomicBool>,
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
    pub update_install_running: Arc<AtomicBool>,
    pub update_install_cancel: Arc<AtomicBool>,
    pub update_install_state: Arc<Mutex<UpdateUiState>>,
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
    /// Staged payloads for meeting-prompt overlays, keyed by an opaque token
    /// passed via URL query (`?t=<token>`). Each overlay consumes exactly one
    /// entry on load. Keyed rather than single-slot to avoid a race when a
    /// second prompt fires before the first overlay's JS has consumed its
    /// payload — see `show_meeting_prompt` in main.rs.
    pub pending_meeting_prompts: Arc<Mutex<HashMap<u64, MeetingPromptData>>>,
    /// `true` iff the currently-active recording was started by a user click
    /// on the call detection banner. Scopes `stop_when_call_ends` so manual
    /// `cmd_start_recording` sessions are never auto-stopped.
    pub recording_started_by_call_detect: Arc<AtomicBool>,
    /// Set by the frontend's "Keep recording" button during an auto-stop
    /// countdown. Read by the countdown thread in `call_detect.rs` to bail
    /// out before calling stop.
    pub call_end_countdown_cancel: Arc<AtomicBool>,
    /// `true` while a call-end auto-stop countdown is running. Keeps repeat
    /// call-end transitions from spawning parallel countdown threads.
    pub call_end_countdown_active: Arc<AtomicBool>,
}

type ParakeetStatusView = minutes_core::transcription_coordinator::ParakeetBackendStatus;

fn parakeet_status_view(config: &Config) -> ParakeetStatusView {
    minutes_core::transcription_coordinator::parakeet_backend_status(config)
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
    pub download_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
enum UpdatePhase {
    Available,
    Checking,
    Downloading,
    Verifying,
    Installing,
    Ready,
    Error,
}

impl UpdatePhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Checking => "checking",
            Self::Downloading => "downloading",
            Self::Verifying => "verifying",
            Self::Installing => "installing",
            Self::Ready => "ready",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUiState {
    phase: UpdatePhase,
    version: Option<String>,
    total_bytes: Option<u64>,
    downloaded_bytes: u64,
    bytes_per_sec: Option<f64>,
    eta_seconds: Option<u64>,
    error_message: Option<String>,
    recoverable: bool,
    can_cancel: bool,
}

impl Default for UpdateUiState {
    fn default() -> Self {
        Self {
            phase: UpdatePhase::Available,
            version: None,
            total_bytes: None,
            downloaded_bytes: 0,
            bytes_per_sec: None,
            eta_seconds: None,
            error_message: None,
            recoverable: false,
            can_cancel: false,
        }
    }
}

impl UpdateUiState {
    fn available(version: impl Into<String>, total_bytes: Option<u64>) -> Self {
        Self {
            phase: UpdatePhase::Available,
            version: Some(version.into()),
            total_bytes,
            ..Self::default()
        }
    }

    fn checking(&self) -> Self {
        Self {
            phase: UpdatePhase::Checking,
            version: self.version.clone(),
            total_bytes: self.total_bytes,
            can_cancel: true,
            ..Self::default()
        }
    }

    fn downloading(&self, total_bytes: Option<u64>) -> Self {
        Self {
            phase: UpdatePhase::Downloading,
            version: self.version.clone(),
            total_bytes,
            can_cancel: true,
            ..Self::default()
        }
    }

    fn with_progress(
        &self,
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
        bytes_per_sec: Option<f64>,
        eta_seconds: Option<u64>,
    ) -> Self {
        Self {
            phase: UpdatePhase::Downloading,
            version: self.version.clone(),
            total_bytes,
            downloaded_bytes,
            bytes_per_sec,
            eta_seconds,
            can_cancel: true,
            ..Self::default()
        }
    }

    fn verifying(&self, downloaded_bytes: u64, total_bytes: Option<u64>) -> Self {
        Self {
            phase: UpdatePhase::Verifying,
            version: self.version.clone(),
            total_bytes,
            downloaded_bytes,
            can_cancel: false,
            ..Self::default()
        }
    }

    fn installing(&self, downloaded_bytes: u64, total_bytes: Option<u64>) -> Self {
        Self {
            phase: UpdatePhase::Installing,
            version: self.version.clone(),
            total_bytes,
            downloaded_bytes,
            can_cancel: false,
            ..Self::default()
        }
    }

    fn ready(&self, downloaded_bytes: u64, total_bytes: Option<u64>) -> Self {
        Self {
            phase: UpdatePhase::Ready,
            version: self.version.clone(),
            total_bytes,
            downloaded_bytes,
            can_cancel: false,
            ..Self::default()
        }
    }

    fn failed(&self, message: impl Into<String>, recoverable: bool) -> Self {
        Self {
            phase: UpdatePhase::Error,
            version: self.version.clone(),
            total_bytes: self.total_bytes,
            downloaded_bytes: self.downloaded_bytes,
            bytes_per_sec: self.bytes_per_sec,
            eta_seconds: self.eta_seconds,
            error_message: Some(message.into()),
            recoverable,
            can_cancel: false,
        }
    }
}

#[derive(Debug)]
enum UpdateInstallError {
    Cancelled,
    Message(String),
}

impl From<String> for UpdateInstallError {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<&str> for UpdateInstallError {
    fn from(value: &str) -> Self {
        Self::Message(value.to_string())
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingPromptData {
    pub title: String,
    pub minutes_until: i64,
    pub url: Option<String>,
}

/// Returns the pending meeting-prompt payload for the given token (and clears
/// it). Called by the overlay window on load. Returning `None` means the
/// token was already consumed, the staging path failed, or the window was
/// opened without a matching token — the overlay should close rather than
/// render a phantom "Meeting" prompt with no context.
#[tauri::command]
pub fn cmd_get_meeting_prompt(
    token: u64,
    state: tauri::State<'_, AppState>,
) -> Option<MeetingPromptData> {
    match state.pending_meeting_prompts.lock() {
        Ok(mut map) => map.remove(&token),
        Err(e) => {
            eprintln!("[calendar] pending_meeting_prompts mutex poisoned: {}", e);
            None
        }
    }
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
        emit_update_ready(app, &update);
    }
}

fn emit_update_ready(app: &tauri::AppHandle, update: &PendingUpdate) {
    let _ = app.emit(
        "update-ready",
        serde_json::json!({
            "version": update.version,
            "body": update.body,
            "downloadBytes": update.download_bytes,
        }),
    );
}

fn set_update_ui_state(
    app: &tauri::AppHandle,
    state: &AppState,
    next: UpdateUiState,
) -> Result<(), String> {
    {
        let mut guard = state
            .update_install_state
            .lock()
            .map_err(|_| "update state lock poisoned".to_string())?;
        *guard = next.clone();
    }

    let _ = app.emit(
        "update://phase",
        serde_json::json!({
            "phase": next.phase.as_str(),
            "version": next.version,
            "totalBytes": next.total_bytes,
            "downloadedBytes": next.downloaded_bytes,
            "canCancel": next.can_cancel,
        }),
    );

    if next.phase == UpdatePhase::Downloading {
        let _ = app.emit(
            "update://progress",
            serde_json::json!({
                "downloadedBytes": next.downloaded_bytes,
                "totalBytes": next.total_bytes,
                "bytesPerSec": next.bytes_per_sec,
                "etaSeconds": next.eta_seconds,
            }),
        );
    }

    if next.phase == UpdatePhase::Error {
        let _ = app.emit(
            "update://error",
            serde_json::json!({
                "message": next.error_message,
                "recoverable": next.recoverable,
            }),
        );
    }

    Ok(())
}

pub(crate) async fn fetch_update_download_size(url: &reqwest::Url) -> Option<u64> {
    let client = reqwest::Client::builder()
        .user_agent("minutes-updater")
        .build()
        .ok()?;
    let response = client.head(url.clone()).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

fn updater_pubkey() -> Result<String, String> {
    let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))
        .map_err(|e| format!("Failed to parse tauri.conf.json: {}", e))?;
    config
        .get("plugins")
        .and_then(|plugins| plugins.get("updater"))
        .and_then(|updater| updater.get("pubkey"))
        .and_then(|pubkey| pubkey.as_str())
        .map(|value| value.to_string())
        .ok_or_else(|| "Updater pubkey missing from tauri.conf.json".to_string())
}

fn verify_update_signature(
    bytes: &[u8],
    release_signature: &str,
    pub_key: &str,
) -> Result<(), String> {
    let pubkey_decoded = String::from_utf8(
        BASE64_STANDARD
            .decode(pub_key)
            .map_err(|e| format!("Failed to decode updater pubkey: {}", e))?,
    )
    .map_err(|e| format!("Failed to parse updater pubkey: {}", e))?;
    let signature_decoded = String::from_utf8(
        BASE64_STANDARD
            .decode(release_signature)
            .map_err(|e| format!("Failed to decode release signature: {}", e))?,
    )
    .map_err(|e| format!("Failed to parse release signature: {}", e))?;

    let public_key = PublicKey::decode(&pubkey_decoded)
        .map_err(|e| format!("Failed to load updater pubkey: {}", e))?;
    let signature = Signature::decode(&signature_decoded)
        .map_err(|e| format!("Failed to load release signature: {}", e))?;

    public_key
        .verify(bytes, &signature, true)
        .map_err(|e| format!("Signature verification failed: {}", e))?;
    Ok(())
}

async fn download_update_bytes(
    update: &tauri_plugin_updater::Update,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, Option<u64>, Option<f64>, Option<u64>) + Send,
) -> Result<Vec<u8>, UpdateInstallError> {
    let response = reqwest::Client::builder()
        .user_agent("minutes-updater")
        .build()
        .map_err(|e| format!("Failed to build update client: {}", e))?
        .get(update.download_url.clone())
        .header(ACCEPT, "application/octet-stream")
        .send()
        .await
        .map_err(|e| format!("Update download failed: {}", e))?;

    if !response.status().is_success() {
        return Err(UpdateInstallError::Message(format!(
            "Update download failed with status {}",
            response.status()
        )));
    }

    let total_bytes = response.content_length();
    let mut downloaded_bytes = 0_u64;
    let started = Instant::now();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            return Err(UpdateInstallError::Cancelled);
        }
        let chunk = chunk.map_err(|e| format!("Update download failed: {}", e))?;
        downloaded_bytes += chunk.len() as u64;
        bytes.extend_from_slice(&chunk);
        let elapsed_secs = started.elapsed().as_secs_f64();
        let bytes_per_sec = if elapsed_secs > 0.0 {
            Some(downloaded_bytes as f64 / elapsed_secs)
        } else {
            None
        };
        let eta_seconds = match (total_bytes, bytes_per_sec) {
            (Some(total), Some(rate)) if rate > 0.0 && downloaded_bytes < total => {
                Some(((total - downloaded_bytes) as f64 / rate).ceil() as u64)
            }
            _ => None,
        };
        on_progress(downloaded_bytes, total_bytes, bytes_per_sec, eta_seconds);
    }

    if cancel.load(Ordering::Relaxed) {
        return Err(UpdateInstallError::Cancelled);
    }

    Ok(bytes)
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
pub struct MeetingReferenceView {
    pub path: String,
    pub title: String,
    pub date: String,
    pub content_type: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RelatedCommitmentView {
    pub path: String,
    pub title: String,
    pub what: String,
    pub who: Option<String>,
    pub by_date: Option<String>,
}

struct RelatedContextView {
    related_people: Vec<String>,
    related_topics: Vec<String>,
    related_meetings: Vec<MeetingReferenceView>,
    related_commitments: Vec<RelatedCommitmentView>,
    adjacent_artifacts: Vec<RecentArtifactView>,
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
    pub related_people: Vec<String>,
    pub related_topics: Vec<String>,
    pub related_meetings: Vec<MeetingReferenceView>,
    pub related_commitments: Vec<RelatedCommitmentView>,
    pub adjacent_artifacts: Vec<RecentArtifactView>,
    pub sections: Vec<MeetingSection>,
    pub speaker_map: Vec<SpeakerAttributionView>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDraft {
    pub path: String,
    pub title: String,
    pub template_kind: String,
    pub content: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFileAccess {
    pub path: String,
    pub editable: bool,
    pub kind: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFileReview {
    pub available: bool,
    pub snapshot_label: Option<String>,
    pub before_preview: Option<String>,
    pub current_preview: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RecentArtifactEntry {
    path: String,
    opened_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentArtifactView {
    pub path: String,
    pub filename: String,
    pub kind: String,
    pub editable: bool,
    pub opened_at: String,
    pub review_available: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecallWorkspaceState {
    pub recall_expanded: bool,
    pub recall_phase: String,
    pub recall_ratio: f64,
    pub current_meeting_path: Option<String>,
    pub open_artifact_path: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OutputNotice {
    pub kind: String,
    pub title: String,
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivationProgress {
    pub desktop_opened_at: Option<String>,
    pub model_ready_at: Option<String>,
    pub first_recording_started_at: Option<String>,
    pub first_artifact_saved_at: Option<String>,
    pub first_artifact_path: Option<String>,
    pub next_step_nudge_shown_at: Option<String>,
    pub next_step_nudge_kind: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationStatusView {
    pub phase: String,
    pub next_action: String,
    pub has_model: bool,
    pub has_saved_artifact: bool,
    pub first_artifact_path: Option<String>,
    pub milestones: ActivationProgress,
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

fn activation_state_path() -> PathBuf {
    Config::minutes_dir().join("activation-state.json")
}

fn recent_artifacts_state_path() -> PathBuf {
    Config::minutes_dir().join("recent-artifacts.json")
}

fn recall_workspace_state_path() -> PathBuf {
    Config::minutes_dir().join("recall-workspace.json")
}

fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339()
}

fn system_time_to_rfc3339(value: SystemTime) -> Option<String> {
    let dt: chrono::DateTime<chrono::Local> = value.into();
    Some(dt.to_rfc3339())
}

fn path_timestamp(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    metadata
        .created()
        .ok()
        .and_then(system_time_to_rfc3339)
        .or_else(|| metadata.modified().ok().and_then(system_time_to_rfc3339))
}

fn persist_activation_progress(progress: &ActivationProgress) {
    let path = activation_state_path();
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let Ok(json) = serde_json::to_string_pretty(progress) else {
        return;
    };
    let _ = std::fs::write(path, json);
}

fn load_recent_artifacts_from(path: &Path) -> Vec<RecentArtifactEntry> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<RecentArtifactEntry>>(&raw).ok())
        .unwrap_or_default()
}

fn persist_recent_artifacts_to(path: &Path, entries: &[RecentArtifactEntry]) {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let Ok(json) = serde_json::to_string_pretty(entries) else {
        return;
    };
    let _ = std::fs::write(path, json);
}

fn load_recall_workspace_state_from(path: &Path) -> RecallWorkspaceState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<RecallWorkspaceState>(&raw).ok())
        .unwrap_or_else(|| RecallWorkspaceState {
            recall_expanded: false,
            recall_phase: "recall".into(),
            recall_ratio: 0.5,
            current_meeting_path: None,
            open_artifact_path: None,
        })
}

fn persist_recall_workspace_state_to(path: &Path, state: &RecallWorkspaceState) {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let Ok(json) = serde_json::to_string_pretty(state) else {
        return;
    };
    let _ = std::fs::write(path, json);
}

fn record_recent_artifact_path(path: &Path) {
    const MAX_RECENT_ARTIFACTS: usize = 12;
    record_recent_artifact_path_with_limit(
        path,
        MAX_RECENT_ARTIFACTS,
        &recent_artifacts_state_path(),
    );
}

fn record_recent_artifact_path_with_limit(path: &Path, max_items: usize, state_path: &Path) {
    let canonical = match validate_text_file_path(path) {
        Ok(path) => path,
        Err(_) => return,
    };
    record_recent_artifact_canonical_with_limit(&canonical, max_items, state_path);
}

fn record_recent_artifact_canonical_with_limit(
    canonical: &Path,
    max_items: usize,
    state_path: &Path,
) {
    let canonical_string = canonical.display().to_string();
    let mut entries = load_recent_artifacts_from(state_path);
    entries.retain(|entry| entry.path != canonical_string);
    entries.insert(
        0,
        RecentArtifactEntry {
            path: canonical_string,
            opened_at: now_rfc3339(),
        },
    );
    entries.retain(|entry| Path::new(&entry.path).exists());
    entries.truncate(max_items);
    persist_recent_artifacts_to(state_path, &entries);
}

fn recent_artifact_views(
    config: &Config,
    limit: usize,
    exclude_path: Option<&Path>,
) -> Vec<RecentArtifactView> {
    let state_path = recent_artifacts_state_path();
    let exclude = exclude_path.map(|path| path.display().to_string());
    let mut views = Vec::new();

    for entry in load_recent_artifacts_from(&state_path).into_iter() {
        if views.len() >= limit {
            break;
        }
        if exclude.as_deref() == Some(entry.path.as_str()) {
            continue;
        }
        let canonical = match validate_text_file_path(Path::new(&entry.path)) {
            Ok(path) => path,
            Err(_) => continue,
        };
        let filename = canonical
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Artifact")
            .to_string();
        let kind = text_file_kind(&canonical).unwrap_or("text").to_string();
        let editable = is_editable_text_file_path(&canonical, config);
        let review_available = latest_snapshot_for_path(&canonical)
            .ok()
            .flatten()
            .is_some();

        views.push(RecentArtifactView {
            path: canonical.display().to_string(),
            filename,
            kind,
            editable,
            opened_at: entry.opened_at,
            review_available,
        });
    }

    views
}

fn update_activation_progress<F>(state: &Arc<Mutex<ActivationProgress>>, mutate: F)
where
    F: FnOnce(&mut ActivationProgress) -> bool,
{
    let mut snapshot = None;
    if let Ok(mut progress) = state.lock() {
        if mutate(&mut progress) {
            snapshot = Some(progress.clone());
        }
    }
    if let Some(progress) = snapshot {
        persist_activation_progress(&progress);
    }
}

fn mark_activation_model_ready(state: &Arc<Mutex<ActivationProgress>>, model_file: &Path) {
    let inferred = path_timestamp(model_file).unwrap_or_else(now_rfc3339);
    update_activation_progress(state, |progress| {
        if progress.model_ready_at.is_none() {
            progress.model_ready_at = Some(inferred);
            return true;
        }
        false
    });
}

fn mark_activation_first_recording_started(state: &Arc<Mutex<ActivationProgress>>) {
    update_activation_progress(state, |progress| {
        if progress.first_recording_started_at.is_none() {
            progress.first_recording_started_at = Some(now_rfc3339());
            return true;
        }
        false
    });
}

fn mark_activation_first_artifact_saved(
    state: &Arc<Mutex<ActivationProgress>>,
    artifact_path: &Path,
) {
    let inferred = path_timestamp(artifact_path).unwrap_or_else(now_rfc3339);
    let path_string = artifact_path.display().to_string();
    update_activation_progress(state, |progress| {
        let mut changed = false;
        if progress.first_artifact_saved_at.is_none() {
            progress.first_artifact_saved_at = Some(inferred.clone());
            changed = true;
        }
        if progress.first_artifact_path.is_none() {
            progress.first_artifact_path = Some(path_string.clone());
            changed = true;
        }
        changed
    });
}

fn mark_activation_next_step_nudge_shown(
    state: &Arc<Mutex<ActivationProgress>>,
    kind: Option<&str>,
) {
    let kind = kind
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from);
    update_activation_progress(state, |progress| {
        let mut changed = false;
        if progress.next_step_nudge_shown_at.is_none() {
            progress.next_step_nudge_shown_at = Some(now_rfc3339());
            changed = true;
        }
        if progress.next_step_nudge_kind.is_none() {
            if let Some(kind) = kind.clone() {
                progress.next_step_nudge_kind = Some(kind);
                changed = true;
            }
        }
        changed
    });
}

fn model_file_for_config(config: &Config) -> PathBuf {
    config
        .transcription
        .model_path
        .join(format!("ggml-{}.bin", config.transcription.model))
}

fn latest_saved_artifact_from_search(config: &Config) -> Option<PathBuf> {
    let filters = minutes_core::search::SearchFilters {
        content_type: None,
        since: None,
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
    };
    minutes_core::search::search("", config, &filters)
        .ok()?
        .into_iter()
        .next()
        .map(|item| item.path)
}

fn backfill_activation_from_paths(
    progress: &mut ActivationProgress,
    model_file: &Path,
    latest_artifact: Option<&Path>,
) -> bool {
    let mut changed = false;
    if progress.model_ready_at.is_none() && model_file.exists() {
        progress.model_ready_at = Some(path_timestamp(model_file).unwrap_or_else(now_rfc3339));
        changed = true;
    }

    if progress.first_artifact_saved_at.is_none() {
        if let Some(path) = latest_artifact {
            progress.first_artifact_saved_at =
                Some(path_timestamp(path).unwrap_or_else(now_rfc3339));
            if progress.first_artifact_path.is_none() {
                progress.first_artifact_path = Some(path.display().to_string());
            }
            changed = true;
        }
    }
    changed
}

pub fn load_activation_progress(config: &Config) -> Arc<Mutex<ActivationProgress>> {
    let path = activation_state_path();
    let mut progress = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<ActivationProgress>(&raw).ok())
        .unwrap_or_default();

    let mut changed = false;

    if progress.desktop_opened_at.is_none() {
        progress.desktop_opened_at = Some(now_rfc3339());
        changed = true;
    }

    let model_file = model_file_for_config(config);
    let latest_artifact = latest_saved_artifact_from_search(config);
    changed |=
        backfill_activation_from_paths(&mut progress, &model_file, latest_artifact.as_deref());

    if changed {
        persist_activation_progress(&progress);
    }

    Arc::new(Mutex::new(progress))
}

fn activation_phase(
    engine: &str,
    progress: &ActivationProgress,
    has_model: bool,
    has_saved_artifact: bool,
    recording: bool,
    processing: bool,
) -> (&'static str, &'static str) {
    if !has_model {
        return (
            "needs-model",
            if engine == "parakeet" {
                "setup-parakeet"
            } else {
                "download-model"
            },
        );
    }
    if progress.first_recording_started_at.is_none() {
        return ("ready-for-first-recording", "start-first-recording");
    }
    if !has_saved_artifact {
        if recording {
            return ("recording-first-artifact", "keep-recording");
        }
        if processing {
            return ("processing-first-artifact", "wait-for-first-artifact");
        }
        return ("ready-for-first-artifact", "start-first-recording");
    }
    if progress.next_step_nudge_shown_at.is_none() {
        return ("first-artifact-saved", "show-next-step");
    }
    ("activated", "explore-minutes")
}

fn activation_status_view(
    engine: &str,
    progress: &ActivationProgress,
    has_model: bool,
    has_saved_artifact: bool,
    recording: bool,
    processing: bool,
) -> ActivationStatusView {
    let (phase, next_action) = activation_phase(
        engine,
        progress,
        has_model,
        has_saved_artifact,
        recording,
        processing,
    );
    ActivationStatusView {
        phase: phase.into(),
        next_action: next_action.into(),
        has_model,
        has_saved_artifact,
        first_artifact_path: progress.first_artifact_path.clone(),
        milestones: progress.clone(),
    }
}

fn build_related_context(
    config: &Config,
    current_path: &Path,
    frontmatter: &minutes_core::markdown::Frontmatter,
) -> RelatedContextView {
    let mut related_people = frontmatter.attendees.clone();
    for person in &frontmatter.people {
        if !related_people.iter().any(|existing| existing == person) {
            related_people.push(person.clone());
        }
    }
    for entity in &frontmatter.entities.people {
        if !related_people
            .iter()
            .any(|existing| existing == &entity.label)
        {
            related_people.push(entity.label.clone());
        }
    }

    let mut related_topics = Vec::new();
    for decision in &frontmatter.decisions {
        if let Some(topic) = decision
            .topic
            .as_ref()
            .filter(|topic| !topic.trim().is_empty())
        {
            if !related_topics
                .iter()
                .any(|existing: &String| existing == topic)
            {
                related_topics.push(topic.clone());
            }
        }
    }

    let mut related_meetings = Vec::new();
    let mut related_meeting_paths = std::collections::HashSet::new();
    let mut related_commitments = Vec::new();

    for person in related_people.iter().take(3) {
        if let Ok(profile) = minutes_core::search::person_profile(config, person) {
            for meeting in profile.recent_meetings.into_iter().take(3) {
                let meeting_path = meeting.path.display().to_string();
                if meeting.path == current_path {
                    continue;
                }
                if related_meeting_paths.insert(meeting_path.clone()) {
                    related_meetings.push(MeetingReferenceView {
                        path: meeting_path,
                        title: meeting.title,
                        date: meeting.date,
                        content_type: meeting.content_type,
                    });
                }
            }

            for intent in profile.open_intents.into_iter().take(3) {
                related_commitments.push(RelatedCommitmentView {
                    path: intent.path.display().to_string(),
                    title: intent.title,
                    what: intent.what,
                    who: intent.who,
                    by_date: intent.by_date,
                });
            }
        }
    }

    for topic in related_topics.iter().take(2) {
        let filters = minutes_core::search::SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: None,
        };
        if let Ok(report) = minutes_core::search::cross_meeting_research(topic, config, &filters) {
            for meeting in report.recent_meetings.into_iter().take(3) {
                let meeting_path = meeting.path.display().to_string();
                if meeting.path == current_path {
                    continue;
                }
                if related_meeting_paths.insert(meeting_path.clone()) {
                    related_meetings.push(MeetingReferenceView {
                        path: meeting_path,
                        title: meeting.title,
                        date: meeting.date,
                        content_type: meeting.content_type,
                    });
                }
            }

            for intent in report.related_open_intents.into_iter().take(3) {
                related_commitments.push(RelatedCommitmentView {
                    path: intent.path.display().to_string(),
                    title: intent.title,
                    what: intent.what,
                    who: intent.who,
                    by_date: intent.by_date,
                });
            }
        }
    }

    related_meetings.truncate(6);
    related_commitments.truncate(6);

    RelatedContextView {
        related_people,
        related_topics,
        related_meetings,
        related_commitments,
        adjacent_artifacts: recent_artifact_views(config, 4, Some(current_path)),
    }
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

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeeklySummaryView {
    pub markdown: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProactiveContextBundleView {
    pub summary: String,
    pub markdown: String,
    pub recent_meeting_count: usize,
    pub recent_memo_count: usize,
    pub stale_commitment_count: usize,
    pub losing_touch_count: usize,
}

fn build_weekly_summary_markdown(
    meetings_count: usize,
    recent_titles: &str,
    decision_conflicts: &str,
    stale_commitments: &str,
    open_actions_block: &str,
) -> String {
    format!(
        "# Weekly Summary\n\n## Volume\n\n- {meetings_count} meeting or memo artifact(s) in the last 7 days.\n\n## Recent Meetings\n\n{recent_titles}\n\n## Decision Arcs\n\n{decision_conflicts}\n\n## Stale Commitments\n\n{stale_commitments}\n\n## Open Actions\n\n{open_actions_block}\n\n## Monday Brief\n\n- Confirm the highest-risk open commitment.\n- Review the most important decision conflict before the next meeting.\n- Turn the most important meeting into a durable artifact if it is still only in transcript form.\n"
    )
}

fn build_proactive_context_markdown(
    recent_meetings: &[String],
    recent_memos: &[String],
    stale_commitments: &[String],
    losing_touch: &[String],
) -> String {
    let meetings_block = if recent_meetings.is_empty() {
        "- No recent meetings.".to_string()
    } else {
        recent_meetings
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let memos_block = if recent_memos.is_empty() {
        "- No recent memos.".to_string()
    } else {
        recent_memos
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let stale_block = if stale_commitments.is_empty() {
        "- No stale commitments.".to_string()
    } else {
        stale_commitments
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let touch_block = if losing_touch.is_empty() {
        "- No losing-touch alerts.".to_string()
    } else {
        losing_touch
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "# Proactive Context\n\n## Recent Meetings\n\n{meetings_block}\n\n## Recent Memos\n\n{memos_block}\n\n## Stale Commitments\n\n{stale_block}\n\n## Losing Touch\n\n{touch_block}\n"
    )
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

fn artifact_slug(text: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for ch in text.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            slug.push(lower);
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

fn text_file_kind(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("md") | Some("markdown") => Some("markdown"),
        Some("txt") => Some("text"),
        Some("json") => Some("json"),
        _ => None,
    }
}

fn resolve_unique_path(dir: &Path, stem: &str, extension: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{stem}.{extension}"));
    let mut suffix = 2u32;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}-{suffix}.{extension}"));
        suffix += 1;
    }
    candidate
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
const LIVE_SHORTCUT_CHOICES: [(&str, &str); 3] = [
    ("CmdOrCtrl+Shift+L", "Cmd/Ctrl + Shift + L"),
    ("CmdOrCtrl+Alt+L", "Cmd/Ctrl + Option/Alt + L"),
    ("CmdOrCtrl+Shift+T", "Cmd/Ctrl + Shift + T"),
];
const DICTATION_SHORTCUT_CHOICES: [(&str, &str); 3] = [
    ("CmdOrCtrl+Shift+Space", "Cmd/Ctrl + Shift + Space"),
    ("CmdOrCtrl+Alt+Space", "Cmd/Ctrl + Option/Alt + Space"),
    ("CmdOrCtrl+Shift+D", "Cmd/Ctrl + Shift + D"),
];
// Codex pass 3 + claude pass 3 P2: dropped `Cmd+Shift+P` from this
// dropdown because it actively conflicts with VS Code's Command
// Palette — offering it as a default-list choice would encourage
// users to break their IDE binding. `Cmd+Alt+Space` is also removed
// because it's the second slot in `DICTATION_SHORTCUT_CHOICES` and
// dual-claiming would silently fail one of the two registrations.
//
// Choices below are checked against `HOTKEY_CHOICES` and
// `DICTATION_SHORTCUT_CHOICES` so we don't reintroduce a collision in
// either direction. Users who want a non-default chord can edit
// `~/.config/minutes/config.toml` directly — the startup register path
// accepts arbitrary accelerator strings.
const PALETTE_SHORTCUT_CHOICES: [(&str, &str); 3] = [
    ("CmdOrCtrl+Shift+K", "Cmd/Ctrl + Shift + K"),
    ("CmdOrCtrl+Shift+O", "Cmd/Ctrl + Shift + O"),
    ("CmdOrCtrl+Shift+U", "Cmd/Ctrl + Shift + U"),
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

pub fn default_palette_shortcut() -> &'static str {
    PALETTE_SHORTCUT_CHOICES[0].0
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

fn palette_shortcut_choices() -> Vec<HotkeyChoice> {
    shortcut_choices(&PALETTE_SHORTCUT_CHOICES)
}

fn live_shortcut_choices() -> Vec<HotkeyChoice> {
    shortcut_choices(&LIVE_SHORTCUT_CHOICES)
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

fn validate_live_shortcut(shortcut: &str) -> Result<String, String> {
    validate_shortcut(shortcut, &LIVE_SHORTCUT_CHOICES)
}

fn validate_download_model_name(model: &str) -> Result<&str, String> {
    const ALLOWED_MODELS: [&str; 5] = ["tiny", "base", "small", "medium", "large-v3"];
    if ALLOWED_MODELS.contains(&model) {
        Ok(model)
    } else {
        Err(format!(
            "Unsupported model: {}. Choose one of: {}",
            model,
            ALLOWED_MODELS.join(", ")
        ))
    }
}

fn validate_palette_shortcut(shortcut: &str) -> Result<String, String> {
    validate_shortcut(shortcut, &PALETTE_SHORTCUT_CHOICES)
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
    activation_progress: &Arc<Mutex<ActivationProgress>>,
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
                activation_progress.clone(),
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
                    if minutes_core::desktop_control::desktop_app_owns_pid(pid) {
                        eprintln!(
                            "recording PID {} is owned by the desktop app; using sentinel-only stop",
                            pid
                        );
                    } else {
                        let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                        if rc != 0 {
                            let err = std::io::Error::last_os_error();
                            eprintln!(
                                "SIGTERM failed (PID {}): {} — sentinel file will stop recording",
                                pid, err
                            );
                        }
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

fn pipeline_stage_label(stage: Option<&str>, mode: Option<CaptureMode>) -> Option<&'static str> {
    let stage = stage?;
    let mode = mode?;
    let parsed = match stage {
        "transcribing" => minutes_core::pipeline::PipelineStage::Transcribing,
        "diarizing" => minutes_core::pipeline::PipelineStage::Diarizing,
        "summarizing" => minutes_core::pipeline::PipelineStage::Summarizing,
        "saving" => minutes_core::pipeline::PipelineStage::Saving,
        _ => return None,
    };
    Some(stage_label(parsed, mode))
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
    activation_progress: Arc<Mutex<ActivationProgress>>,
    completion_notifications_enabled: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let config = Config::load();
        let result = minutes_core::jobs::process_pending_jobs(&config, |job| {
            sync_processing_indicator(&processing, &processing_stage);

            if let Some(notice) = output_notice_from_job(job) {
                set_latest_output(&latest_output, Some(notice.clone()));
                if notice.kind == "saved" {
                    mark_activation_first_artifact_saved(
                        &activation_progress,
                        Path::new(&notice.path),
                    );
                }
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

fn find_section_content<'a>(sections: &'a [MeetingSection], heading: &str) -> Option<&'a str> {
    sections
        .iter()
        .find(|section| section.heading.eq_ignore_ascii_case(heading))
        .map(|section| section.content.as_str())
        .filter(|content| !content.trim().is_empty())
}

fn artifact_directory(config: &Config) -> Result<PathBuf, String> {
    let workspace = crate::context::create_workspace(config)?;
    let artifacts = workspace.join("artifacts");
    std::fs::create_dir_all(&artifacts).map_err(|e| {
        format!(
            "Failed to create artifact directory {}: {}",
            artifacts.display(),
            e
        )
    })?;
    Ok(artifacts)
}

fn is_editable_text_file_path(path: &Path, config: &Config) -> bool {
    let workspace = crate::context::workspace_dir();
    let trusted_roots = [config.output_dir.clone(), workspace.join("artifacts")];
    trusted_roots.iter().any(|root| path.starts_with(root))
}

const MAX_ARTIFACT_SNAPSHOTS_PER_FILE: usize = 20;

fn snapshot_identity_for_path(path: &Path) -> (String, String) {
    let base = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("md")
        .to_ascii_lowercase();
    let mut hasher = DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    let hash = format!("{:08x}", hasher.finish() as u32);
    (format!("{base}-{hash}"), extension)
}

fn prune_artifact_snapshots(
    snapshot_root: &Path,
    identity: &str,
    extension: &str,
) -> Result<(), String> {
    let matching = matching_snapshots(snapshot_root, identity, extension)?;
    if matching.len() <= MAX_ARTIFACT_SNAPSHOTS_PER_FILE {
        return Ok(());
    }

    let remove_count = matching.len() - MAX_ARTIFACT_SNAPSHOTS_PER_FILE;
    for path in matching.into_iter().take(remove_count) {
        std::fs::remove_file(&path)
            .map_err(|e| format!("Failed to prune old snapshot {}: {}", path.display(), e))?;
    }
    Ok(())
}

fn create_text_file_snapshot(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    let original = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot snapshot {}: {}", path.display(), e))?;
    let snapshot_root = Config::minutes_dir().join("artifact-snapshots");
    std::fs::create_dir_all(&snapshot_root).map_err(|e| {
        format!(
            "Failed to create artifact snapshot directory {}: {}",
            snapshot_root.display(),
            e
        )
    })?;

    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let (identity, extension) = snapshot_identity_for_path(path);
    let snapshot_path = snapshot_root.join(format!("{timestamp}-{identity}.{extension}"));
    std::fs::write(&snapshot_path, original).map_err(|e| {
        format!(
            "Failed to write artifact snapshot {}: {}",
            snapshot_path.display(),
            e
        )
    })?;
    prune_artifact_snapshots(&snapshot_root, &identity, &extension)
}

fn latest_snapshot_for_path(path: &Path) -> Result<Option<PathBuf>, String> {
    let snapshot_root = Config::minutes_dir().join("artifact-snapshots");
    if !snapshot_root.exists() {
        return Ok(None);
    }
    let (identity, extension) = snapshot_identity_for_path(path);
    let mut matching = matching_snapshots(&snapshot_root, &identity, &extension)?;
    Ok(matching.pop())
}

fn matching_snapshots(
    snapshot_root: &Path,
    identity: &str,
    extension: &str,
) -> Result<Vec<PathBuf>, String> {
    let suffix = format!("-{identity}.{extension}");
    let mut matching = std::fs::read_dir(snapshot_root)
        .map_err(|e| {
            format!(
                "Failed to read snapshot directory {}: {}",
                snapshot_root.display(),
                e
            )
        })?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|candidate| {
            candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&suffix))
        })
        .collect::<Vec<_>>();
    matching.sort();
    Ok(matching)
}

fn preview_text_for_review(text: &str, max_lines: usize, max_chars: usize) -> String {
    let mut joined = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if joined.chars().count() > max_chars {
        joined = joined.chars().take(max_chars).collect::<String>();
        joined.push_str("\n…");
    } else if text.lines().count() > max_lines {
        joined.push_str("\n…");
    }
    joined
}

fn review_preview_for_kind(kind: &str, text: &str, max_lines: usize, max_chars: usize) -> String {
    if kind == "json" {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
            if let Ok(pretty) = serde_json::to_string_pretty(&parsed) {
                return preview_text_for_review(&pretty, max_lines, max_chars);
            }
        }
    }
    preview_text_for_review(text, max_lines, max_chars)
}

fn write_text_file_atomic(path: &Path, content: &str) -> Result<(), String> {
    if content.len() > 1_048_576 {
        return Err("Refusing to save a text file larger than 1 MB.".into());
    }
    create_text_file_snapshot(path)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid file name: {}", path.display()))?;
    let temp_path = path.with_file_name(format!(".{}.tmp", file_name));
    std::fs::write(&temp_path, content)
        .map_err(|e| format!("Failed to write temp file {}: {}", temp_path.display(), e))?;
    std::fs::rename(&temp_path, path).map_err(|e| {
        format!(
            "Failed to atomically replace temp file {} with destination {}: {}",
            temp_path.display(),
            path.display(),
            e
        )
    })
}

fn meeting_section_bullets(content: Option<&str>, empty: &str) -> String {
    content
        .map(|text| {
            text.lines()
                .filter_map(|line| {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| empty.to_string())
}

fn build_artifact_template(
    frontmatter: &minutes_core::markdown::Frontmatter,
    sections: &[MeetingSection],
    meeting_path: &Path,
    kind: &str,
) -> Result<(String, String), String> {
    let meeting_title = frontmatter.title.trim();
    let slug = artifact_slug(meeting_title);
    let title = match kind {
        "follow-up-email" => format!("Follow-up Email - {}", meeting_title),
        "meeting-brief" => format!("Meeting Brief - {}", meeting_title),
        "debrief-memo" => format!("Debrief Memo - {}", meeting_title),
        "decision-memo" => format!("Decision Memo - {}", meeting_title),
        other => {
            return Err(format!(
            "Unknown artifact template '{}'. Use follow-up-email, meeting-brief, debrief-memo, or decision-memo.",
            other
        ))
        }
    };

    let summary = meeting_section_bullets(
        find_section_content(sections, "Summary"),
        "- Add a concise recap of what happened.\n- Pull the strongest 2-3 moments from the meeting.",
    );
    let decisions = if frontmatter.decisions.is_empty() {
        "- Add any decisions that should carry forward.".to_string()
    } else {
        frontmatter
            .decisions
            .iter()
            .map(|decision| format!("- {}", decision.text))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let action_items = if frontmatter.action_items.is_empty() {
        "- Add the next actions and owners.".to_string()
    } else {
        frontmatter
            .action_items
            .iter()
            .map(|item| {
                let due = item
                    .due
                    .as_ref()
                    .map(|value| format!(" (due {})", value))
                    .unwrap_or_default();
                format!("- {}: {}{}", item.assignee, item.task, due)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let open_questions = meeting_section_bullets(
        find_section_content(sections, "Open Questions"),
        "- Add unresolved questions worth carrying into the next conversation.",
    );
    let attendees = if frontmatter.attendees.is_empty() {
        "_Add attendees if needed._".to_string()
    } else {
        frontmatter.attendees.join(", ")
    };

    let frontmatter_block = format!(
        "---\ntitle: {}\nartifact_type: {}\nsource_meeting: {}\nsource_title: {}\nsource_date: {}\nlinked_slug: {}\n---\n\n",
        title,
        kind,
        meeting_path.display(),
        meeting_title,
        frontmatter.date.to_rfc3339(),
        slug
    );

    let body = match kind {
        "follow-up-email" => format!(
            "# Subject\n\nFollow-up: {meeting_title}\n\n# Email Draft\n\nHi team,\n\nThanks again for the conversation today. Here is the clean follow-up from the meeting.\n\n## Key Points\n\n{summary}\n\n## Decisions\n\n{decisions}\n\n## Action Items\n\n{action_items}\n\n## Open Questions\n\n{open_questions}\n\nBest,\n\n[Your name]\n"
        ),
        "meeting-brief" => format!(
            "# Objective\n\nState what this next meeting needs to accomplish.\n\n## Context\n\n- Source meeting: [{meeting_title}]({})\n- Attendees: {attendees}\n\n## What Happened Last Time\n\n{summary}\n\n## Decisions Already Made\n\n{decisions}\n\n## Open Questions\n\n{open_questions}\n\n## Suggested Agenda\n\n- Start with the highest-stakes question\n- Confirm any blocked action items\n- End with explicit owners and dates\n\n## Notes\n\n- Add prep notes here.\n",
            meeting_path.display()
        ),
        "debrief-memo" => format!(
            "# Summary\n\n{summary}\n\n## Decisions\n\n{decisions}\n\n## Action Items\n\n{action_items}\n\n## Open Questions\n\n{open_questions}\n\n## Next Move\n\n- Write the next action the team should take from this conversation.\n"
        ),
        "decision-memo" => format!(
            "# Decision\n\nWrite the one decision this memo is locking in.\n\n## Why This Decision\n\n{summary}\n\n## Decision Details\n\n{decisions}\n\n## Implications\n\n- Add the operational, product, or relationship implications of this decision.\n\n## Action Items\n\n{action_items}\n\n## Open Questions / Risks\n\n{open_questions}\n"
        ),
        _ => unreachable!(),
    };

    Ok((title, format!("{frontmatter_block}{body}")))
}

fn model_status(config: &Config) -> ReadinessItem {
    if config.transcription.engine == "parakeet" {
        let status = parakeet_status_view(config);

        return ReadinessItem {
            label: "Speech model".into(),
            state: if status.ready { "ready" } else { "attention" }.into(),
            detail: if status.ready {
                let tokenizer_label = status
                    .tokenizer_label
                    .unwrap_or_else(|| "unknown".to_string());
                format!(
                    "Parakeet backend ready. Model: {}. Tokenizer: {}. Warm: {}.",
                    status.model,
                    tokenizer_label,
                    if status.warm { "yes" } else { "no" }
                )
            } else {
                format!(
                    "Parakeet backend needs setup: {}. Run: minutes setup --parakeet",
                    status.issues.join(", ")
                )
            },
            optional: false,
        };
    }

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

/// Handles that `start_recording` clears at the end of a session. Keeps the
/// auto-stop state tied to a single recording: if the user started this
/// recording via the call detection banner, these flags live until the
/// recording ends; after that, a subsequent manual `minutes record` must not
/// be treated as call-detection-started.
#[derive(Clone)]
pub struct CallDetectSessionHandles {
    pub started_by_call_detect: Arc<AtomicBool>,
    pub countdown_active: Arc<AtomicBool>,
    pub countdown_cancel: Arc<AtomicBool>,
}

/// RAII guard that clears the call-detect session flags when dropped.
/// Used to keep every exit path in `start_recording` / `start_native_call_recording`
/// (including early returns on capture failure) from leaving stale state.
pub struct CallDetectSessionGuard {
    handles: CallDetectSessionHandles,
}

impl CallDetectSessionGuard {
    pub fn new(handles: CallDetectSessionHandles) -> Self {
        Self { handles }
    }
}

impl Drop for CallDetectSessionGuard {
    fn drop(&mut self) {
        self.handles
            .started_by_call_detect
            .store(false, Ordering::Relaxed);
        self.handles
            .countdown_active
            .store(false, Ordering::Relaxed);
        self.handles.countdown_cancel.store(true, Ordering::Relaxed);
    }
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
    activation_progress: Arc<Mutex<ActivationProgress>>,
    call_capture_health: Arc<Mutex<Option<crate::call_capture::CallSourceHealth>>>,
    completion_notifications_enabled: Arc<AtomicBool>,
    hotkey_runtime: Option<Arc<Mutex<HotkeyRuntime>>>,
    discard_short_hotkey_capture: Option<Arc<AtomicBool>>,
    call_detect_session: CallDetectSessionHandles,
    mode: CaptureMode,
    requested_intent: Option<RecordingIntent>,
    allow_degraded: bool,
    requested_title: Option<String>,
    language_override: Option<String>,
) {
    // Drop on any exit path (early returns, panic, normal exit) clears the
    // session flags so a subsequent manual recording isn't auto-stopped.
    let _session_guard = CallDetectSessionGuard::new(call_detect_session);
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
            &activation_progress,
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
                            activation_progress.clone(),
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
    let activation_progress = state.activation_progress.clone();
    let call_capture_health = state.call_capture_health.clone();
    let completion_notifications_enabled = state.completion_notifications_enabled.clone();
    let call_detect_session = CallDetectSessionHandles {
        started_by_call_detect: state.recording_started_by_call_detect.clone(),
        countdown_active: state.call_end_countdown_active.clone(),
        countdown_cancel: state.call_end_countdown_cancel.clone(),
    };
    let app_done = app.clone();
    mark_activation_first_recording_started(&activation_progress);

    std::thread::spawn(move || {
        start_recording(
            app,
            rec,
            starting,
            stop,
            processing,
            processing_stage,
            latest_output,
            activation_progress,
            call_capture_health,
            completion_notifications_enabled,
            hotkey_runtime,
            discard_short_hotkey_capture,
            call_detect_session,
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
#[allow(clippy::too_many_arguments)]
pub fn cmd_start_recording(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    mode: Option<String>,
    intent: Option<String>,
    allow_degraded: Option<bool>,
    title: Option<String>,
    language: Option<String>,
    source: Option<String>,
) -> Result<(), String> {
    let capture_mode = parse_capture_mode(mode.as_deref())?;
    let requested_intent = parse_recording_intent(intent.as_deref())?;

    // Session-level flag that scopes the stop_when_call_ends auto-stop only
    // to recordings started via the call detection banner. Manual starts
    // never get auto-stopped, even when the config flag is on.
    let from_call_detect = source.as_deref() == Some("call_detect");
    state
        .recording_started_by_call_detect
        .store(from_call_detect, Ordering::Relaxed);
    // Starting a fresh recording always cancels any in-flight countdown so
    // the UI doesn't auto-stop a session the user has already moved past.
    cancel_call_end_countdown(&state);

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

/// Clear countdown state — both the active flag and (as a no-op safety net)
/// the cancel flag. Used when a new recording starts, when the countdown
/// elapses, and when the user cancels via "Keep recording".
pub fn cancel_call_end_countdown(state: &AppState) {
    state
        .call_end_countdown_cancel
        .store(true, Ordering::Relaxed);
    state
        .call_end_countdown_active
        .store(false, Ordering::Relaxed);
}

#[tauri::command]
pub fn cmd_cancel_call_end_countdown(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    cancel_call_end_countdown(&state);
    // Tell the UI to hide the banner immediately; the countdown thread will
    // observe the cancel flag on its next tick and exit without stopping.
    app.emit("call:end-countdown:cancelled", ()).ok();
    Ok(())
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
    let processing_stage_label =
        pipeline_stage_label(processing_stage.as_deref(), status.recording_mode);
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
    let update_state = state
        .update_install_state
        .lock()
        .ok()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let config = Config::load();
    let has_model = if config.transcription.engine == "parakeet" {
        let parakeet = parakeet_status_view(&config);
        if parakeet.ready {
            if let Some(model_path) = parakeet.model_path.as_ref() {
                mark_activation_model_ready(&state.activation_progress, Path::new(model_path));
            }
        }
        parakeet.ready
    } else {
        let model_file = model_file_for_config(&config);
        if model_file.exists() {
            mark_activation_model_ready(&state.activation_progress, &model_file);
        }
        model_file.exists()
    };
    let activation_progress = state
        .activation_progress
        .lock()
        .ok()
        .map(|progress| progress.clone())
        .unwrap_or_default();
    let has_saved_artifact = activation_progress.first_artifact_saved_at.is_some();
    let activation = activation_status_view(
        &config.transcription.engine,
        &activation_progress,
        has_model,
        has_saved_artifact,
        recording || (status.recording && !processing),
        processing,
    );

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
        "processingStageLabel": processing_stage_label,
        "processingTitle": status.processing_title,
        "processingJobId": status.processing_job_id,
        "processingJobCount": status.processing_job_count,
        "processingJobs": processing_jobs,
        "updateState": update_state,
        "latestOutput": latest_output,
        "activation": activation,
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
        state.activation_progress.clone(),
        state.completion_notifications_enabled.clone(),
    );
    Ok(())
}

#[tauri::command]
pub fn cmd_weekly_summary() -> Result<WeeklySummaryView, String> {
    let config = Config::load();
    let since = (chrono::Local::now() - chrono::Duration::days(7)).to_rfc3339();
    let filters = minutes_core::search::SearchFilters {
        content_type: None,
        since: Some(since.clone()),
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
    };

    let meetings =
        minutes_core::search::search("", &config, &filters).map_err(|e| e.to_string())?;
    let consistency =
        minutes_core::search::consistency_report(&config, None, 7).map_err(|e| e.to_string())?;
    let open_actions =
        minutes_core::search::find_open_actions(&config, None).map_err(|e| e.to_string())?;

    let meetings_count = meetings.len();
    let recent_titles = if meetings.is_empty() {
        "- No meetings or memos in the last 7 days.".to_string()
    } else {
        meetings
            .iter()
            .take(6)
            .map(|meeting| format!("- {} ({})", meeting.title, meeting.date))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let decision_conflicts = if consistency.decision_conflicts.is_empty() {
        "- No conflicting decision arcs detected.".to_string()
    } else {
        consistency
            .decision_conflicts
            .iter()
            .take(5)
            .map(|conflict| format!("- {} -> {}", conflict.topic, conflict.latest.what))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let stale_commitments = if consistency.stale_commitments.is_empty() {
        "- No stale commitments detected.".to_string()
    } else {
        consistency
            .stale_commitments
            .iter()
            .take(5)
            .map(|item| {
                format!(
                    "- {}{}",
                    item.entry.what,
                    item.entry
                        .who
                        .as_ref()
                        .map(|who| format!(" ({})", who))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let open_actions_block = if open_actions.is_empty() {
        "- No open action items found.".to_string()
    } else {
        open_actions
            .iter()
            .take(6)
            .map(|item| {
                format!(
                    "- {}: {}{}",
                    item.assignee,
                    item.task,
                    item.due
                        .as_ref()
                        .map(|due| format!(" (due {})", due))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let markdown = build_weekly_summary_markdown(
        meetings_count,
        &recent_titles,
        &decision_conflicts,
        &stale_commitments,
        &open_actions_block,
    );

    Ok(WeeklySummaryView { markdown })
}

#[tauri::command]
pub fn cmd_proactive_context_bundle() -> Result<ProactiveContextBundleView, String> {
    let config = Config::load();
    let since = (chrono::Local::now() - chrono::Duration::days(7)).to_rfc3339();
    let filters = minutes_core::search::SearchFilters {
        content_type: None,
        since: Some(since),
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
    };

    let recent_results =
        minutes_core::search::search("", &config, &filters).map_err(|e| e.to_string())?;
    let recent_meetings: Vec<String> = recent_results
        .iter()
        .filter(|item| item.content_type != "memo")
        .take(4)
        .map(|item| format!("{} ({})", item.title, item.date))
        .collect();
    let recent_memos: Vec<String> = recent_results
        .iter()
        .filter(|item| item.content_type == "memo")
        .take(4)
        .map(|item| format!("{} ({})", item.title, item.date))
        .collect();

    let consistency =
        minutes_core::search::consistency_report(&config, None, 7).map_err(|e| e.to_string())?;
    let stale_commitments: Vec<String> = consistency
        .stale_commitments
        .iter()
        .take(4)
        .map(|item| {
            format!(
                "{}{}",
                item.entry.what,
                item.entry
                    .who
                    .as_ref()
                    .map(|who| format!(" ({who})"))
                    .unwrap_or_default()
            )
        })
        .collect();

    let losing_touch = minutes_core::graph::relationship_map(&config)
        .map(|people| {
            people
                .into_iter()
                .filter(|person| person.losing_touch)
                .take(4)
                .map(|person| {
                    format!(
                        "{} (last {}d ago)",
                        person.name,
                        person.days_since.round() as i64
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let summary = format!(
        "{} meetings · {} memos · {} stale commitments · {} losing-touch alerts",
        recent_meetings.len(),
        recent_memos.len(),
        stale_commitments.len(),
        losing_touch.len()
    );
    let markdown = build_proactive_context_markdown(
        &recent_meetings,
        &recent_memos,
        &stale_commitments,
        &losing_touch,
    );

    Ok(ProactiveContextBundleView {
        summary,
        markdown,
        recent_meeting_count: recent_meetings.len(),
        recent_memo_count: recent_memos.len(),
        stale_commitment_count: stale_commitments.len(),
        losing_touch_count: losing_touch.len(),
    })
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
    let entries = minutes_core::capture::list_input_devices_detailed();
    // Back-compat: preserve the decorated label list for any caller that
    // still reads `devices`, while exposing structured entries so pickers
    // can store the canonical name instead of the label.
    let legacy_labels: Vec<String> = entries.iter().map(|e| e.label.clone()).collect();
    serde_json::json!({
        "devices": legacy_labels,
        "entries": entries,
        "configured_device": configured_device,
    })
}

#[tauri::command]
pub fn cmd_delete_meeting(
    app: tauri::AppHandle,
    path: String,
    with_audio: bool,
    force: bool,
) -> Result<String, String> {
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
        // Show native confirmation dialog and wait for user response
        let confirmed = app
            .dialog()
            .message(format!(
                "Archive \"{}\" and its audio recording?\nThey will be moved to the archive folder.",
                title
            ))
            .title("Archive Meeting")
            .kind(MessageDialogKind::Warning)
            .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancel)
            .blocking_show();

        if !confirmed {
            return Ok("Cancelled".into());
        }

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

fn validate_text_file_path(path: &Path) -> Result<PathBuf, String> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| format!("Cannot resolve {}: {}", path.display(), e))?;
    let path_str = canonical.to_string_lossy();

    // Only allow reads under the user's home directory
    let home = dirs::home_dir().ok_or("Cannot determine home directory")?;
    if !canonical.starts_with(&home) {
        return Err(format!(
            "Access denied: {} is outside home directory",
            path_str
        ));
    }

    let meta =
        std::fs::metadata(&canonical).map_err(|e| format!("Cannot stat {}: {}", path_str, e))?;
    if !meta.is_file() {
        return Err(format!("Not a file: {}", path_str));
    }

    // Cap at 1MB to prevent OOM on huge files
    if meta.len() > 1_048_576 {
        return Err(format!(
            "File too large: {} ({} bytes, max 1MB)",
            path_str,
            meta.len()
        ));
    }

    let extension = canonical
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .ok_or_else(|| format!("Unsupported text file: {}", path_str))?;
    if !matches!(extension.as_str(), "md" | "markdown" | "txt" | "json") {
        return Err(format!(
            "Unsupported text file: {} (expected .md, .markdown, .txt, or .json)",
            path_str
        ));
    }

    Ok(canonical)
}

fn write_notice_prompt(command: &str, plain_text: &str) -> String {
    if is_shell_command(command) {
        format!("cat <<'__MINUTES__'\n{plain_text}\n__MINUTES__\n")
    } else {
        format!("{plain_text}\n")
    }
}

fn artifact_switch_prompt(command: &str, artifact_name: Option<&str>) -> String {
    let plain_text = match artifact_name {
        Some(name) => format!(
            "Minutes opened artifact {name}. Read CURRENT_ARTIFACT.md and CLAUDE.md. The user has this file open in the left pane and may want help editing it. If you update it on disk, the viewer will refresh live."
        ),
        None => "Minutes cleared the open artifact focus. Ignore CURRENT_ARTIFACT.md unless it reappears. If CURRENT_MEETING.md exists, prioritize it; otherwise continue in general assistant mode."
            .into(),
    };
    write_notice_prompt(command, &plain_text)
}

fn notify_assistant_artifact_focus(
    state: &tauri::State<AppState>,
    artifact_name: Option<&str>,
) -> Result<(), String> {
    let mut manager = state
        .pty_manager
        .lock()
        .map_err(|_| "PTY manager lock failed")?;
    if manager.assistant_session_id().is_some() {
        if let Some(command) = manager.session_command(crate::pty::ASSISTANT_SESSION_ID) {
            let prompt = artifact_switch_prompt(&command, artifact_name);
            manager.write_input(crate::pty::ASSISTANT_SESSION_ID, prompt.as_bytes())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn cmd_read_text_file(path: String) -> Result<String, String> {
    let canonical = validate_text_file_path(Path::new(&path))?;
    let path_str = canonical.to_string_lossy();
    std::fs::read_to_string(&canonical).map_err(|e| format!("Cannot read {}: {}", path_str, e))
}

#[tauri::command]
pub fn cmd_get_text_file_access(path: String) -> Result<TextFileAccess, String> {
    let canonical = validate_text_file_path(Path::new(&path))?;
    let config = Config::load();
    Ok(TextFileAccess {
        path: canonical.display().to_string(),
        editable: is_editable_text_file_path(&canonical, &config),
        kind: text_file_kind(&canonical).unwrap_or("text").to_string(),
    })
}

#[tauri::command]
pub fn cmd_get_text_file_review(path: String) -> Result<TextFileReview, String> {
    let canonical = validate_text_file_path(Path::new(&path))?;
    let Some(snapshot) = latest_snapshot_for_path(&canonical)? else {
        return Ok(TextFileReview {
            available: false,
            snapshot_label: None,
            before_preview: None,
            current_preview: None,
        });
    };
    let before = std::fs::read_to_string(&snapshot)
        .map_err(|e| format!("Cannot read snapshot {}: {}", snapshot.display(), e))?;
    let current = std::fs::read_to_string(&canonical)
        .map_err(|e| format!("Cannot read {}: {}", canonical.display(), e))?;
    let kind = text_file_kind(&canonical).unwrap_or("text");
    let snapshot_label = snapshot
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_string());
    Ok(TextFileReview {
        available: true,
        snapshot_label,
        before_preview: Some(review_preview_for_kind(kind, &before, 80, 4000)),
        current_preview: Some(review_preview_for_kind(kind, &current, 80, 4000)),
    })
}

#[tauri::command]
pub fn cmd_recent_artifacts(limit: Option<usize>) -> serde_json::Value {
    let config = Config::load();
    let views = recent_artifact_views(&config, limit.unwrap_or(8), None);
    serde_json::to_value(views).unwrap_or_else(|_| serde_json::json!([]))
}

#[tauri::command]
pub fn cmd_get_recall_workspace_state() -> serde_json::Value {
    serde_json::to_value(load_recall_workspace_state_from(
        &recall_workspace_state_path(),
    ))
    .unwrap_or_else(|_| serde_json::json!({}))
}

#[tauri::command]
pub fn cmd_set_recall_workspace_state(
    recall_expanded: Option<bool>,
    recall_phase: Option<String>,
    recall_ratio: Option<f64>,
    current_meeting_path: Option<String>,
    open_artifact_path: Option<String>,
) -> Result<(), String> {
    let state_path = recall_workspace_state_path();
    let mut state = load_recall_workspace_state_from(&state_path);

    if let Some(value) = recall_expanded {
        state.recall_expanded = value;
    }
    if let Some(value) = recall_phase {
        state.recall_phase = if value.trim().is_empty() {
            "recall".into()
        } else {
            value
        };
    }
    if let Some(value) = recall_ratio {
        state.recall_ratio = value.clamp(0.25, 0.75);
    }
    if let Some(value) = current_meeting_path {
        state.current_meeting_path = if value.trim().is_empty() {
            None
        } else {
            Some(value)
        };
    }
    if let Some(value) = open_artifact_path {
        state.open_artifact_path = if value.trim().is_empty() {
            None
        } else {
            Some(value)
        };
    }

    persist_recall_workspace_state_to(&state_path, &state);
    Ok(())
}

#[tauri::command]
pub fn cmd_set_open_artifact(state: tauri::State<AppState>, path: String) -> Result<(), String> {
    let canonical = validate_text_file_path(Path::new(&path))?;
    record_recent_artifact_path(&canonical);
    let config = Config::load();
    let workspace = crate::context::create_workspace(&config)?;
    crate::context::write_assistant_context(&workspace, &config)?;
    crate::context::write_active_artifact_context(&workspace, &canonical)?;
    let artifact_name = canonical.file_name().and_then(|name| name.to_str());
    notify_assistant_artifact_focus(&state, artifact_name)
}

#[tauri::command]
pub fn cmd_clear_open_artifact(state: tauri::State<AppState>) -> Result<(), String> {
    let workspace = crate::context::workspace_dir();
    if workspace.exists() {
        crate::context::clear_active_artifact_context(&workspace)?;
    }
    notify_assistant_artifact_focus(&state, None)
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

    let related = build_related_context(&config, &meeting_path, &frontmatter);

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
        related_people: related.related_people,
        related_topics: related.related_topics,
        related_meetings: related.related_meetings,
        related_commitments: related.related_commitments,
        adjacent_artifacts: related.adjacent_artifacts,
        sections: parse_sections(body),
        speaker_map,
    })
}

#[tauri::command]
pub fn cmd_write_text_file(path: String, content: String) -> Result<String, String> {
    let canonical = validate_text_file_path(Path::new(&path))?;
    let config = Config::load();
    if !is_editable_text_file_path(&canonical, &config) {
        return Err(format!(
            "{} is view-only in Minutes. Make an editable copy first.",
            canonical.display()
        ));
    }
    let normalized = if text_file_kind(&canonical) == Some("json") {
        let parsed: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| format!("Invalid JSON: {}", e))?;
        serde_json::to_string_pretty(&parsed)
            .map_err(|e| format!("Could not format JSON: {}", e))?
    } else {
        content
    };
    write_text_file_atomic(&canonical, &normalized)?;
    Ok(format!("Saved {}", canonical.display()))
}

#[tauri::command]
pub fn cmd_restore_text_file_snapshot(path: String) -> Result<String, String> {
    let canonical = validate_text_file_path(Path::new(&path))?;
    let config = Config::load();
    if !is_editable_text_file_path(&canonical, &config) {
        return Err(format!(
            "{} is view-only in Minutes. Make an editable copy first.",
            canonical.display()
        ));
    }
    let Some(snapshot) = latest_snapshot_for_path(&canonical)? else {
        return Err(format!(
            "No snapshot available yet for {}",
            canonical.display()
        ));
    };
    let content = std::fs::read_to_string(&snapshot)
        .map_err(|e| format!("Cannot read snapshot {}: {}", snapshot.display(), e))?;
    let file_name = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid file name: {}", canonical.display()))?;
    let temp_path = canonical.with_file_name(format!(".{}.restore.tmp", file_name));
    std::fs::write(&temp_path, content).map_err(|e| {
        format!(
            "Failed to write restore temp file {}: {}",
            temp_path.display(),
            e
        )
    })?;
    std::fs::rename(&temp_path, &canonical).map_err(|e| {
        format!(
            "Failed to restore {} from snapshot {}: {}",
            canonical.display(),
            snapshot.display(),
            e
        )
    })?;
    Ok(format!(
        "Restored {} from {}",
        canonical.display(),
        snapshot.display()
    ))
}

#[tauri::command]
pub fn cmd_promote_text_file_to_artifact(path: String) -> Result<ArtifactDraft, String> {
    let canonical = validate_text_file_path(Path::new(&path))?;
    let config = Config::load();
    let artifacts_dir = artifact_directory(&config)?;
    let source_content = std::fs::read_to_string(&canonical)
        .map_err(|e| format!("Cannot read {}: {}", canonical.display(), e))?;
    let title = canonical
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("Working Copy")
        .replace('-', " ");
    let stem = format!(
        "{}-working-copy-{}",
        chrono::Local::now().format("%Y-%m-%d"),
        artifact_slug(&title)
    );
    let extension = canonical
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("md");
    let artifact_path = resolve_unique_path(&artifacts_dir, &stem, extension);
    write_text_file_atomic(&artifact_path, &source_content)?;
    Ok(ArtifactDraft {
        path: artifact_path.display().to_string(),
        title,
        template_kind: "working-copy".into(),
        content: source_content,
    })
}

#[tauri::command]
pub fn cmd_create_artifact_from_meeting(
    meeting_path: String,
    kind: String,
) -> Result<ArtifactDraft, String> {
    let config = Config::load();
    let meeting_path = PathBuf::from(&meeting_path);
    minutes_core::notes::validate_meeting_path(&meeting_path, &config.output_dir)?;

    let content = std::fs::read_to_string(&meeting_path)
        .map_err(|e| format!("Cannot read meeting: {}", e))?;
    let (frontmatter_str, body) = minutes_core::markdown::split_frontmatter(&content);
    let frontmatter: minutes_core::markdown::Frontmatter =
        serde_yaml::from_str(frontmatter_str.trim())
            .map_err(|e| format!("Bad frontmatter: {}", e))?;
    let sections = parse_sections(body);

    let artifacts_dir = artifact_directory(&config)?;
    let template_kind = kind.trim().to_ascii_lowercase();
    let (title, artifact_content) =
        build_artifact_template(&frontmatter, &sections, &meeting_path, &template_kind)?;
    let stem = format!(
        "{}-{}-{}",
        chrono::Local::now().format("%Y-%m-%d"),
        template_kind,
        artifact_slug(&frontmatter.title)
    );
    let artifact_path = resolve_unique_path(&artifacts_dir, &stem, "md");
    write_text_file_atomic(&artifact_path, &artifact_content)?;

    Ok(ArtifactDraft {
        path: artifact_path.display().to_string(),
        title,
        template_kind,
        content: artifact_content,
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
pub fn cmd_needs_setup(state: tauri::State<AppState>) -> serde_json::Value {
    let config = Config::load();
    let model_name = if config.transcription.engine == "parakeet" {
        &config.transcription.parakeet_model
    } else {
        &config.transcription.model
    };
    let parakeet = if config.transcription.engine == "parakeet" {
        Some(parakeet_status_view(&config))
    } else {
        None
    };
    let has_model = if let Some(status) = parakeet.as_ref() {
        if status.ready {
            if let Some(model_path) = status.model_path.as_ref() {
                mark_activation_model_ready(&state.activation_progress, Path::new(model_path));
            }
        }
        status.ready
    } else {
        let model_file = model_file_for_config(&config);
        let exists = model_file.exists();
        if exists {
            mark_activation_model_ready(&state.activation_progress, &model_file);
        }
        exists
    };

    let meetings_dir = config.output_dir.clone();
    let has_meetings_dir = meetings_dir.exists();
    let activation_progress = state
        .activation_progress
        .lock()
        .ok()
        .map(|progress| progress.clone())
        .unwrap_or_default();

    serde_json::json!({
        "needsSetup": !has_model,
        "hasModel": has_model,
        "engine": config.transcription.engine,
        "modelName": model_name,
        "parakeet": parakeet,
        "hasMeetingsDir": has_meetings_dir,
        "activation": activation_status_view(
            &config.transcription.engine,
            &activation_progress,
            has_model,
            activation_progress.first_artifact_saved_at.is_some(),
            false,
            false,
        ),
    })
}

#[tauri::command]
pub async fn cmd_download_model(
    state: tauri::State<'_, AppState>,
    model: String,
) -> Result<String, String> {
    // Run in a blocking thread so the UI stays responsive during download
    let activation_progress = state.activation_progress.clone();
    tauri::async_runtime::spawn_blocking(move || {
        validate_download_model_name(&model)?;

        let config = Config::load();
        let model_dir = &config.transcription.model_path;
        let model_file = model_dir.join(format!("ggml-{}.bin", model));

        if model_file.exists() {
            mark_activation_model_ready(&activation_progress, &model_file);
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
        mark_activation_model_ready(&activation_progress, &model_file);

        Ok(format!("Downloaded '{}' model ({} MB)", model, size))
    })
    .await
    .map_err(|e| format!("Download task failed: {}", e))?
}

#[tauri::command]
pub fn cmd_mark_activation_nudge_shown(state: tauri::State<AppState>, kind: Option<String>) {
    mark_activation_next_step_nudge_shown(&state.activation_progress, kind.as_deref());
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
        home.join(".opencode/bin"),
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
const WELL_KNOWN_AGENTS: &[&str] = &["claude", "codex", "gemini", "opencode", "bash", "zsh"];

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
            "engine": config.transcription.engine,
            "model": config.transcription.model,
            "downloaded_models": downloaded_models,
            "language": config.transcription.language,
            "parakeet_model": config.transcription.parakeet_model,
            "parakeet_binary": config.transcription.parakeet_binary,
            "parakeet_sidecar_enabled": config.transcription.parakeet_sidecar_enabled,
            "parakeet_compiled": cfg!(feature = "parakeet"),
            "parakeet_status": parakeet_status_view(&config),
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
        "privacy": {
            "hide_from_screen_share": config.privacy.hide_from_screen_share,
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
pub async fn cmd_warm_parakeet() -> Result<serde_json::Value, String> {
    let config = Config::load();
    if config.transcription.engine != "parakeet" {
        return Ok(serde_json::json!({
            "status": "skipped",
            "reason": "parakeet not selected",
        }));
    }
    #[cfg(feature = "parakeet")]
    {
        let stats = tauri::async_runtime::spawn_blocking(move || {
            minutes_core::transcription_coordinator::warmup_active_backend(&config)
        })
        .await
        .map_err(|error| format!("warmup task failed: {}", error))?
        .map_err(|error| error.to_string())?;

        return Ok(serde_json::json!({
            "status": "ok",
            "backend_id": stats.backend_id,
            "model": stats.model,
            "elapsed_ms": stats.elapsed_ms,
            "used_gpu": stats.used_gpu,
        }));
    }
    #[cfg(not(feature = "parakeet"))]
    {
        Ok(serde_json::json!({
            "status": "skipped",
            "reason": "parakeet feature not compiled",
        }))
    }
}

#[tauri::command]
pub fn cmd_set_setting(section: String, key: String, value: String) -> Result<String, String> {
    let mut config = Config::load();

    match (section.as_str(), key.as_str()) {
        // Transcription
        ("transcription", "engine") => {
            if !["whisper", "parakeet"].contains(&value.as_str()) {
                return Err(format!(
                    "unknown transcription engine '{}'. Valid: whisper, parakeet",
                    value
                ));
            }
            config.transcription.engine = value.clone();
        }
        ("transcription", "model") => config.transcription.model = value.clone(),
        ("transcription", "parakeet_model") => {
            if !VALID_PARAKEET_MODELS.contains(&value.as_str()) {
                return Err(format!(
                    "unknown parakeet model '{}'. Valid: {}",
                    value,
                    VALID_PARAKEET_MODELS.join(", ")
                ));
            }
            config.transcription.parakeet_model = value.clone();
            config.transcription.parakeet_vocab = format!("{}.tokenizer.vocab", value);
        }
        ("transcription", "parakeet_sidecar_enabled") => {
            config.transcription.parakeet_sidecar_enabled = value == "true";
        }
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
pub fn cmd_set_screen_share_hidden(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    hidden: bool,
) -> Result<(), String> {
    let mut config = Config::load();
    config.privacy.hide_from_screen_share = hidden;
    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;

    state.screen_share_hidden.store(hidden, Ordering::Relaxed);
    for (_, window) in app.webview_windows() {
        window.set_content_protected(hidden).ok();
    }

    Ok(())
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
    fn activation_phase_guides_new_user_to_download_model_first() {
        let progress = ActivationProgress::default();
        let (phase, action) = activation_phase("whisper", &progress, false, false, false, false);

        assert_eq!(phase, "needs-model");
        assert_eq!(action, "download-model");
    }

    #[test]
    fn activation_phase_guides_parakeet_user_to_setup_flow_first() {
        let progress = ActivationProgress::default();
        let (phase, action) = activation_phase("parakeet", &progress, false, false, false, false);

        assert_eq!(phase, "needs-model");
        assert_eq!(action, "setup-parakeet");
    }

    #[test]
    fn activation_phase_guides_user_to_first_recording_after_model_ready() {
        let progress = ActivationProgress {
            model_ready_at: Some("2026-04-09T12:00:00-07:00".into()),
            ..ActivationProgress::default()
        };
        let (phase, action) = activation_phase("whisper", &progress, true, false, false, false);

        assert_eq!(phase, "ready-for-first-recording");
        assert_eq!(action, "start-first-recording");
    }

    #[test]
    fn activation_phase_reports_processing_until_first_artifact_finishes() {
        let progress = ActivationProgress {
            model_ready_at: Some("2026-04-09T12:00:00-07:00".into()),
            first_recording_started_at: Some("2026-04-09T12:01:00-07:00".into()),
            ..ActivationProgress::default()
        };
        let (phase, action) = activation_phase("whisper", &progress, true, false, false, true);

        assert_eq!(phase, "processing-first-artifact");
        assert_eq!(action, "wait-for-first-artifact");
    }

    #[test]
    fn activation_phase_requires_next_step_nudge_after_first_artifact() {
        let progress = ActivationProgress {
            model_ready_at: Some("2026-04-09T12:00:00-07:00".into()),
            first_recording_started_at: Some("2026-04-09T12:01:00-07:00".into()),
            first_artifact_saved_at: Some("2026-04-09T12:02:00-07:00".into()),
            first_artifact_path: Some("/tmp/demo.md".into()),
            ..ActivationProgress::default()
        };
        let (phase, action) = activation_phase("whisper", &progress, true, true, false, false);

        assert_eq!(phase, "first-artifact-saved");
        assert_eq!(action, "show-next-step");
    }

    #[test]
    fn parakeet_status_reports_missing_assets() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.transcription.engine = "parakeet".into();
        config.transcription.model_path = dir.path().to_path_buf();

        let status = parakeet_status_view(&config);
        assert!(!status.ready);
        assert!(!status.model_found);
        assert!(!status.tokenizer_found);
        assert!(
            status
                .issues
                .iter()
                .any(|issue| issue.contains("model assets")),
            "expected missing model issue, got {:?}",
            status.issues
        );
    }

    #[test]
    fn parakeet_status_reports_ready_with_metadata() {
        let dir = TempDir::new().unwrap();
        let mut config = Config::default();
        config.transcription.engine = "parakeet".into();
        config.transcription.model_path = dir.path().to_path_buf();
        config.transcription.parakeet_binary = if cfg!(windows) {
            "cmd".into()
        } else {
            "sh".into()
        };

        let install_dir = minutes_core::parakeet::install_dir(&config, "tdt-ctc-110m");
        std::fs::create_dir_all(&install_dir).unwrap();
        let model = install_dir.join("tdt-ctc-110m.safetensors");
        let tokenizer = install_dir.join("tdt-ctc-110m.tokenizer.vocab");
        std::fs::write(&model, b"model").unwrap();
        std::fs::write(&tokenizer, b"tokenizer").unwrap();
        minutes_core::parakeet::write_install_metadata(&config, "tdt-ctc-110m", &model, &tokenizer)
            .unwrap();

        let status = parakeet_status_view(&config);
        assert!(status.ready);
        assert!(status.model_found);
        assert!(status.tokenizer_found);
        assert!(status.metadata.is_some());
        assert_eq!(
            status.tokenizer_label.as_deref(),
            Some("tdt-ctc-110m.tokenizer.vocab")
        );
    }

    #[test]
    fn backfill_activation_from_paths_populates_missing_model_and_artifact_milestones() {
        let temp = TempDir::new().unwrap();
        let model = temp.path().join("ggml-small.bin");
        let artifact = temp.path().join("2026-04-09-demo.md");
        std::fs::write(&model, "model").unwrap();
        std::fs::write(&artifact, "---\ntitle: Demo\n---\n").unwrap();

        let mut progress = ActivationProgress::default();
        let changed = backfill_activation_from_paths(&mut progress, &model, Some(&artifact));
        let expected = artifact.display().to_string();

        assert!(changed);
        assert!(progress.model_ready_at.is_some());
        assert!(progress.first_artifact_saved_at.is_some());
        assert_eq!(
            progress.first_artifact_path.as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn build_artifact_template_includes_meeting_metadata() {
        let fm = minutes_core::markdown::Frontmatter {
            title: "Pricing Review".into(),
            r#type: ContentType::Meeting,
            date: chrono::Local::now(),
            duration: "30m".into(),
            source: None,
            status: Some(minutes_core::markdown::OutputStatus::Complete),
            tags: vec![],
            attendees: vec!["Mat".into(), "Alex".into()],
            attendees_raw: None,
            calendar_event: None,
            people: vec![],
            entities: minutes_core::markdown::EntityLinks::default(),
            device: None,
            captured_at: None,
            context: None,
            action_items: vec![minutes_core::markdown::ActionItem {
                assignee: "Mat".into(),
                task: "Send follow-up".into(),
                due: Some("Friday".into()),
                status: "open".into(),
            }],
            decisions: vec![minutes_core::markdown::Decision {
                text: "Ship the new pricing page".into(),
                topic: Some("pricing".into()),
            }],
            intents: vec![],
            recorded_by: None,
            visibility: None,
            speaker_map: vec![],
            filter_diagnosis: None,
        };
        let sections = vec![MeetingSection {
            heading: "Summary".into(),
            content: "- We aligned on pricing changes.".into(),
        }];

        let (title, body) = build_artifact_template(
            &fm,
            &sections,
            Path::new("/tmp/pricing-review.md"),
            "debrief-memo",
        )
        .unwrap();

        assert!(title.contains("Pricing Review"));
        assert!(body.contains("source_meeting: /tmp/pricing-review.md"));
        assert!(body.contains("Ship the new pricing page"));
        assert!(body.contains("Mat: Send follow-up"));
    }

    #[test]
    fn build_decision_memo_template_includes_decision_sections() {
        let fm = minutes_core::markdown::Frontmatter {
            title: "Pricing Review".into(),
            r#type: ContentType::Meeting,
            date: chrono::Local::now(),
            duration: "30m".into(),
            source: None,
            status: Some(minutes_core::markdown::OutputStatus::Complete),
            tags: vec![],
            attendees: vec!["Mat".into(), "Alex".into()],
            attendees_raw: None,
            calendar_event: None,
            people: vec![],
            entities: minutes_core::markdown::EntityLinks::default(),
            device: None,
            captured_at: None,
            context: None,
            action_items: vec![minutes_core::markdown::ActionItem {
                assignee: "Mat".into(),
                task: "Send follow-up".into(),
                due: Some("Friday".into()),
                status: "open".into(),
            }],
            decisions: vec![minutes_core::markdown::Decision {
                text: "Ship the new pricing page".into(),
                topic: Some("pricing".into()),
            }],
            intents: vec![],
            recorded_by: None,
            visibility: None,
            speaker_map: vec![],
            filter_diagnosis: None,
        };
        let sections = vec![MeetingSection {
            heading: "Summary".into(),
            content: "- We aligned on pricing changes.".into(),
        }];

        let (title, body) = build_artifact_template(
            &fm,
            &sections,
            Path::new("/tmp/pricing-review.md"),
            "decision-memo",
        )
        .unwrap();

        assert!(title.contains("Decision Memo"));
        assert!(body.contains("# Decision"));
        assert!(body.contains("## Decision Details"));
        assert!(body.contains("## Implications"));
        assert!(body.contains("Ship the new pricing page"));
    }

    #[test]
    fn starter_artifact_pack_templates_all_build() {
        let fm = minutes_core::markdown::Frontmatter {
            title: "Pricing Review".into(),
            r#type: ContentType::Meeting,
            date: chrono::Local::now(),
            duration: "30m".into(),
            source: None,
            status: Some(minutes_core::markdown::OutputStatus::Complete),
            tags: vec![],
            attendees: vec!["Mat".into(), "Alex".into()],
            attendees_raw: None,
            calendar_event: None,
            people: vec![],
            entities: minutes_core::markdown::EntityLinks::default(),
            device: None,
            captured_at: None,
            context: None,
            action_items: vec![],
            decisions: vec![],
            intents: vec![],
            recorded_by: None,
            visibility: None,
            speaker_map: vec![],
            filter_diagnosis: None,
        };
        let sections = vec![MeetingSection {
            heading: "Summary".into(),
            content: "- We aligned on pricing changes.".into(),
        }];

        for kind in [
            "debrief-memo",
            "follow-up-email",
            "meeting-brief",
            "decision-memo",
        ] {
            let (_title, body) =
                build_artifact_template(&fm, &sections, Path::new("/tmp/pricing-review.md"), kind)
                    .unwrap_or_else(|error| panic!("template {kind} failed: {error}"));
            assert!(body.contains("source_meeting: /tmp/pricing-review.md"));
        }
    }

    #[test]
    fn record_recent_artifact_path_keeps_latest_entry_first_and_deduplicated() {
        let temp = TempDir::new().unwrap();
        let a = temp.path().join("artifact-a.md");
        let b = temp.path().join("artifact-b.md");
        std::fs::write(&a, "# A").unwrap();
        std::fs::write(&b, "# B").unwrap();
        let state_path = temp.path().join(".minutes/recent-artifacts.json");

        record_recent_artifact_canonical_with_limit(&a, 4, &state_path);
        record_recent_artifact_canonical_with_limit(&b, 4, &state_path);
        record_recent_artifact_canonical_with_limit(&a, 4, &state_path);

        let entries = load_recent_artifacts_from(&state_path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, a.display().to_string());
        assert_eq!(entries[1].path, b.display().to_string());
    }

    #[test]
    fn record_recent_artifact_path_prunes_to_limit() {
        let temp = TempDir::new().unwrap();
        let state_path = temp.path().join(".minutes/recent-artifacts.json");

        for idx in 0..5 {
            let path = temp.path().join(format!("artifact-{idx}.md"));
            std::fs::write(&path, format!("# {idx}")).unwrap();
            record_recent_artifact_canonical_with_limit(&path, 3, &state_path);
        }

        let entries = load_recent_artifacts_from(&state_path);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].path.ends_with("artifact-4.md"));
        assert!(entries[2].path.ends_with("artifact-2.md"));
    }

    #[test]
    fn recall_workspace_state_round_trips() {
        let temp = TempDir::new().unwrap();
        let state_path = temp.path().join(".minutes/recall-workspace.json");
        let state = RecallWorkspaceState {
            recall_expanded: true,
            recall_phase: "debrief".into(),
            recall_ratio: 0.61,
            current_meeting_path: Some("/tmp/meeting.md".into()),
            open_artifact_path: Some("/tmp/artifact.md".into()),
        };

        persist_recall_workspace_state_to(&state_path, &state);
        let restored = load_recall_workspace_state_from(&state_path);

        assert_eq!(restored, state);
    }

    #[test]
    fn recall_workspace_state_defaults_when_missing() {
        let temp = TempDir::new().unwrap();
        let state_path = temp.path().join(".minutes/missing.json");

        let restored = load_recall_workspace_state_from(&state_path);

        assert!(!restored.recall_expanded);
        assert_eq!(restored.recall_phase, "recall");
        assert_eq!(restored.recall_ratio, 0.5);
        assert_eq!(restored.current_meeting_path, None);
        assert_eq!(restored.open_artifact_path, None);
    }

    #[test]
    fn build_related_context_collects_people_topics_meetings_and_commitments() {
        let temp = TempDir::new().unwrap();
        let meetings = temp.path().join("meetings");
        std::fs::create_dir_all(&meetings).unwrap();
        let current = meetings.join("2026-03-17-pricing-review.md");
        let followup = meetings.join("2026-03-20-follow-up.md");

        std::fs::write(
            &current,
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\nattendees: [Alex]\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at monthly billing per month\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        )
        .unwrap();
        std::fs::write(
            &followup,
            "---\ntitle: Follow-up\ntype: meeting\ndate: 2026-03-20T12:00:00-07:00\nduration: 30m\nstatus: complete\nattendees: [Alex]\npeople: []\naction_items: []\ndecisions: []\nintents:\n  - kind: commitment\n    what: Share revised pricing model\n    who: Alex\n    status: open\n    by_date: Tuesday\n---\n\n## Transcript\n\nWe followed up on pricing.\n",
        )
        .unwrap();

        let config = Config {
            output_dir: meetings.clone(),
            ..Config::default()
        };
        let frontmatter: minutes_core::markdown::Frontmatter = serde_yaml::from_str(
            "title: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\nattendees: [Alex]\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at monthly billing per month\n    topic: pricing\nintents: []\n",
        )
        .unwrap();

        let related = build_related_context(&config, &current, &frontmatter);

        assert!(related.related_people.iter().any(|person| person == "Alex"));
        assert!(related
            .related_topics
            .iter()
            .any(|topic| topic == "pricing"));
        assert!(related
            .related_meetings
            .iter()
            .any(|meeting| meeting.title == "Follow-up"));
        assert!(related
            .related_commitments
            .iter()
            .any(|commitment| commitment.what.contains("Share revised pricing model")));
    }

    #[test]
    fn build_related_context_links_memo_to_related_meeting() {
        let temp = TempDir::new().unwrap();
        let meetings = temp.path().join("meetings");
        let memos = meetings.join("memos");
        std::fs::create_dir_all(&memos).unwrap();
        let current = memos.join("2026-03-19-pricing-idea.md");
        let related_meeting = meetings.join("2026-03-20-pricing-review.md");

        std::fs::write(
            &current,
            "---\ntitle: Pricing Idea\ntype: memo\ndate: 2026-03-19T12:00:00-07:00\nduration: 2m\nstatus: complete\ntags:\n  - memo\n  - topic:pricing-strategy\nattendees: [Alex]\npeople: [Alex]\naction_items: []\ndecisions:\n  - text: Explore premium annual billing\n    topic: pricing strategy\nintents: []\n---\n\n## Transcript\n\nVoice memo about pricing.\n",
        )
        .unwrap();
        std::fs::write(
            &related_meeting,
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-20T12:00:00-07:00\nduration: 30m\nstatus: complete\nattendees: [Alex]\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at monthly billing per month\n    topic: pricing strategy\nintents: []\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        )
        .unwrap();

        let config = Config {
            output_dir: meetings.clone(),
            ..Config::default()
        };
        let frontmatter: minutes_core::markdown::Frontmatter = serde_yaml::from_str(
            "title: Pricing Idea\ntype: memo\ndate: 2026-03-19T12:00:00-07:00\nduration: 2m\nstatus: complete\ntags:\n  - memo\n  - topic:pricing-strategy\nattendees: [Alex]\npeople: [Alex]\naction_items: []\ndecisions:\n  - text: Explore premium annual billing\n    topic: pricing strategy\nintents: []\n",
        )
        .unwrap();

        let related = build_related_context(&config, &current, &frontmatter);

        assert!(related
            .related_meetings
            .iter()
            .any(|meeting| meeting.title == "Pricing Review"));
        assert!(related
            .related_topics
            .iter()
            .any(|topic| topic == "pricing strategy"));
    }

    #[test]
    fn build_weekly_summary_markdown_includes_core_sections() {
        let markdown = build_weekly_summary_markdown(
            3,
            "- Pricing Review\n- Follow-up",
            "- pricing -> Launch monthly billing",
            "- Share revised pricing model (Alex)",
            "- Alex: Send updated doc",
        );

        assert!(markdown.contains("# Weekly Summary"));
        assert!(markdown.contains("## Recent Meetings"));
        assert!(markdown.contains("## Decision Arcs"));
        assert!(markdown.contains("## Stale Commitments"));
        assert!(markdown.contains("## Open Actions"));
        assert!(markdown.contains("## Monday Brief"));
    }

    #[test]
    fn text_file_kind_detects_json() {
        assert_eq!(text_file_kind(Path::new("/tmp/test.json")), Some("json"));
        assert_eq!(text_file_kind(Path::new("/tmp/test.md")), Some("markdown"));
        assert_eq!(text_file_kind(Path::new("/tmp/test.txt")), Some("text"));
    }

    #[test]
    fn prune_artifact_snapshots_keeps_latest_per_file_identity() {
        let temp = TempDir::new().unwrap();
        for idx in 0..25 {
            let path = temp
                .path()
                .join(format!("20260409-120{idx:02}-artifact-abcd1234.md"));
            std::fs::write(path, "snapshot").unwrap();
        }

        prune_artifact_snapshots(temp.path(), "artifact-abcd1234", "md").unwrap();

        let remaining = std::fs::read_dir(temp.path()).unwrap().count();
        assert_eq!(remaining, MAX_ARTIFACT_SNAPSHOTS_PER_FILE);
    }

    #[test]
    fn latest_snapshot_for_path_returns_newest_snapshot() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("artifact.md");
        let snapshot_root = temp.path().join("snapshots");
        std::fs::create_dir_all(&snapshot_root).unwrap();
        let (identity, extension) = snapshot_identity_for_path(&path);
        let older = snapshot_root.join(format!("20260409-120000-{identity}.{extension}"));
        let newer = snapshot_root.join(format!("20260409-120100-{identity}.{extension}"));
        std::fs::write(&older, "old").unwrap();
        std::fs::write(&newer, "new").unwrap();

        let mut matching = matching_snapshots(&snapshot_root, &identity, &extension).unwrap();
        assert_eq!(matching.pop().unwrap(), newer);
    }

    #[test]
    fn review_preview_for_kind_pretty_prints_json() {
        let preview = review_preview_for_kind("json", "{\"b\":2,\"a\":1}", 80, 4000);
        assert!(preview.contains("\"a\": 1"));
        assert!(preview.contains("\"b\": 2"));
        assert!(preview.contains('\n'));
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
    fn validate_palette_shortcut_accepts_default_choices() {
        assert_eq!(
            validate_palette_shortcut("CmdOrCtrl+Shift+K").unwrap(),
            "CmdOrCtrl+Shift+K"
        );
        assert_eq!(
            validate_palette_shortcut("CmdOrCtrl+Shift+O").unwrap(),
            "CmdOrCtrl+Shift+O"
        );
        assert_eq!(
            validate_palette_shortcut("CmdOrCtrl+Shift+U").unwrap(),
            "CmdOrCtrl+Shift+U"
        );
    }

    #[test]
    fn validate_palette_shortcut_rejects_unknown() {
        assert!(validate_palette_shortcut("CmdOrCtrl+Shift+Z").is_err());
        assert!(validate_palette_shortcut("nonsense").is_err());
        // Codex pass 3: P (VS Code Command Palette conflict) and
        // Alt+Space (collides with DICTATION_SHORTCUT_CHOICES) were
        // dropped on purpose. Both should be rejected.
        assert!(validate_palette_shortcut("CmdOrCtrl+Shift+P").is_err());
        assert!(validate_palette_shortcut("CmdOrCtrl+Alt+Space").is_err());
    }

    #[test]
    fn validate_live_shortcut_accepts_known_values() {
        assert_eq!(
            validate_live_shortcut("CmdOrCtrl+Shift+L").unwrap(),
            "CmdOrCtrl+Shift+L"
        );
        assert_eq!(
            validate_live_shortcut("CmdOrCtrl+Alt+L").unwrap(),
            "CmdOrCtrl+Alt+L"
        );
    }

    #[test]
    fn validate_live_shortcut_rejects_unknown_values() {
        assert!(validate_live_shortcut("CmdOrCtrl+Shift+M").is_err());
        assert!(validate_live_shortcut("nonsense").is_err());
    }

    #[test]
    fn validate_download_model_name_rejects_path_like_input() {
        assert!(validate_download_model_name("../../.ssh/evil").is_err());
        assert!(validate_download_model_name("tiny").is_ok());
    }

    #[test]
    fn palette_shortcut_choices_do_not_collide_with_other_minutes_choices() {
        use std::collections::HashSet;
        let palette: HashSet<&str> = PALETTE_SHORTCUT_CHOICES.iter().map(|(v, _)| *v).collect();
        let hotkey: HashSet<&str> = HOTKEY_CHOICES.iter().map(|(v, _)| *v).collect();
        let dictation: HashSet<&str> = DICTATION_SHORTCUT_CHOICES.iter().map(|(v, _)| *v).collect();
        for chord in &palette {
            assert!(
                !hotkey.contains(chord),
                "{} appears in both PALETTE_SHORTCUT_CHOICES and HOTKEY_CHOICES",
                chord
            );
            assert!(
                !dictation.contains(chord),
                "{} appears in both PALETTE_SHORTCUT_CHOICES and DICTATION_SHORTCUT_CHOICES",
                chord
            );
        }
    }

    #[test]
    fn shortcut_collision_error_ignores_disabled_shortcuts() {
        let in_use = [
            ("dictation", false, Some("CmdOrCtrl+Shift+K".to_string())),
            (
                "live transcript",
                true,
                Some("CmdOrCtrl+Shift+O".to_string()),
            ),
        ];

        assert!(shortcut_collision_error("CmdOrCtrl+Shift+K", &in_use).is_ok());
        assert!(shortcut_collision_error("CmdOrCtrl+Shift+O", &in_use)
            .unwrap_err()
            .contains("live transcript"));
    }

    #[test]
    fn humanize_shortcut_renders_modifiers_as_glyphs() {
        assert_eq!(humanize_shortcut("CmdOrCtrl+Shift+K"), "⌘⇧K");
        assert_eq!(humanize_shortcut("CmdOrCtrl+Alt+Space"), "⌘⌥Space");
        assert_eq!(humanize_shortcut("CmdOrCtrl+Shift+O"), "⌘⇧O");
        // Unknown pieces fall through verbatim.
        assert_eq!(
            humanize_shortcut("CmdOrCtrl+Shift+Backspace"),
            "⌘⇧Backspace"
        );
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
    .content_protected(Config::load().privacy.hide_from_screen_share)
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
        "source": status.source,
        "diagnostic": status.diagnostic,
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
        choices: live_shortcut_choices(),
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

    let next_shortcut = validate_live_shortcut(&shortcut)?;
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

#[tauri::command]
pub fn cmd_palette_settings(state: tauri::State<AppState>) -> HotkeySettings {
    let enabled = state.palette_shortcut_enabled.load(Ordering::Relaxed);
    let shortcut = state
        .palette_shortcut
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|_| default_palette_shortcut().to_string());
    HotkeySettings {
        enabled,
        shortcut,
        choices: palette_shortcut_choices(),
    }
}

/// Reject a palette shortcut that collides with another Minutes
/// shortcut. The other dropdowns (quick-thought hotkey, dictation,
/// live transcript) all hand-out chord strings; if the user picks the
/// same chord for two of them, the second `register` call will
/// silently fail at the OS level and one of the two features stops
/// working with no surfaced error. This helper turns that into a
/// clear up-front rejection.
///
/// Codex pass 3 + claude pass 3 P2.
fn ensure_no_palette_shortcut_collision(state: &AppState, candidate: &str) -> Result<(), String> {
    let in_use = [
        (
            "dictation",
            state.dictation_shortcut_enabled.load(Ordering::Relaxed),
            state.dictation_shortcut.lock().ok().map(|s| s.clone()),
        ),
        (
            "live transcript",
            state.live_shortcut_enabled.load(Ordering::Relaxed),
            state.live_shortcut.lock().ok().map(|s| s.clone()),
        ),
        (
            "quick thought hotkey",
            state.global_hotkey_enabled.load(Ordering::Relaxed),
            state.global_hotkey_shortcut.lock().ok().map(|s| s.clone()),
        ),
    ];
    shortcut_collision_error(candidate, &in_use)
}

fn shortcut_collision_error(
    candidate: &str,
    in_use: &[(&str, bool, Option<String>)],
) -> Result<(), String> {
    for (name, enabled, value) in in_use {
        if *enabled && value.as_deref().is_some_and(|other| other == candidate) {
            return Err(format!(
                "{} is already used by the {} shortcut",
                candidate, name
            ));
        }
    }
    Ok(())
}

#[tauri::command]
pub fn cmd_set_palette_shortcut(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    enabled: bool,
    shortcut: String,
) -> Result<HotkeySettings, String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let next_shortcut = validate_palette_shortcut(&shortcut)?;
    if enabled {
        ensure_no_palette_shortcut_collision(&state, &next_shortcut)?;
    }
    let previous = cmd_palette_settings(state.clone());
    let manager = app.global_shortcut();

    if previous.enabled {
        // Codex pass 3 P2: treat unregister failure as fatal. The
        // previous code logged-and-continued, which left the OLD
        // chord still registered AND the new chord registered on top
        // of it. Subsequent presses of the old chord no longer
        // matched `palette_shortcut_id` (state was already updated)
        // and fell through to `handle_global_hotkey_event` — i.e.
        // the wrong feature fired. Better to refuse the rebind than
        // to leave the routing inconsistent.
        if let Err(e) = manager.unregister(previous.shortcut.as_str()) {
            return Err(format!(
                "Could not unregister previous palette shortcut {}: {}",
                previous.shortcut, e
            ));
        }
    }

    if enabled {
        if let Err(e) = manager.register(next_shortcut.as_str()) {
            // The new shortcut won't register — try to restore the
            // previous one so the user keeps a working palette
            // toggle. If the rollback ALSO fails, force-disable the
            // palette shortcut so the in-memory state matches the
            // empty OS registration. Claude pass 3 P2 #8: silent
            // dead palette is the worst failure mode.
            let mut rollback_failed = false;
            if previous.enabled {
                if let Err(rollback_err) = manager.register(previous.shortcut.as_str()) {
                    eprintln!(
                        "[palette-shortcut] rollback re-register of {} failed: {}",
                        previous.shortcut, rollback_err
                    );
                    rollback_failed = true;
                }
            }
            if rollback_failed {
                state
                    .palette_shortcut_enabled
                    .store(false, Ordering::Relaxed);
                cmd_set_setting("palette".into(), "shortcut_enabled".into(), "false".into()).ok();
                return Err(format!(
                    "Could not register {} and could not restore the previous shortcut. \
                     Palette shortcut is now disabled — set a different binding from \
                     Settings to re-enable.",
                    next_shortcut
                ));
            }
            return Err(format!(
                "Could not register {}. Another app may already be using it. ({})",
                next_shortcut, e
            ));
        }
    }

    state
        .palette_shortcut_enabled
        .store(enabled, Ordering::Relaxed);
    if let Ok(mut current) = state.palette_shortcut.lock() {
        *current = next_shortcut.clone();
    }

    // Persist to config.toml so the next launch picks up the user's
    // choice without re-running the migration.
    cmd_set_setting(
        "palette".into(),
        "shortcut_enabled".into(),
        enabled.to_string(),
    )
    .ok();
    cmd_set_setting("palette".into(), "shortcut".into(), next_shortcut).ok();

    Ok(cmd_palette_settings(state))
}

/// Marker file used to track whether the palette first-run notice has
/// been shown to the user. Stored as a sibling to `palette.json` in
/// `~/.minutes/` so it survives config rewrites and works across
/// processes (CLI vs desktop) without a config schema dance.
fn palette_first_run_marker() -> PathBuf {
    Config::minutes_dir().join("palette_first_run_shown")
}

/// Fire a one-shot system notification announcing the new command
/// palette. Called from `main.rs::setup` after the palette shortcut
/// is registered. The marker file ensures this only happens once per
/// machine, even across reinstalls — the only way to re-trigger it is
/// to delete the marker file manually.
///
/// **Why this exists**: the upgrade migration used to default the
/// shortcut to OFF specifically to avoid hijacking VS Code's
/// `Delete Line` and JetBrains' `Push...` chords without consent.
/// That made the feature undiscoverable. The current design defaults
/// ON for both fresh installs and upgrades, but fires this
/// notification on the first launch so users with a real conflict
/// hear about it immediately and can disable from the settings UI in
/// one click. See PLAN.md.command-palette-slice-2 D10 (post-fix).
pub fn maybe_show_palette_first_run_notice(app: &tauri::AppHandle) {
    let marker = palette_first_run_marker();
    if marker.exists() {
        return;
    }

    let state = app.state::<AppState>();
    if !state.palette_shortcut_enabled.load(Ordering::Relaxed) {
        // The user (or some other process) already opted out before
        // the notice ran. Don't show it.
        return;
    }
    let shortcut = state
        .palette_shortcut
        .lock()
        .map(|s| s.clone())
        .unwrap_or_else(|_| default_palette_shortcut().to_string());

    let body = format!(
        "Press {} to open the new command palette. \
         Disable in Settings if it conflicts with your other apps.",
        humanize_shortcut(&shortcut)
    );

    // Dispatch the notification FIRST. The marker is only written on
    // successful delivery so the next launch retries if delivery
    // failed (notification permission denied, Notification Center
    // unhealthy, etc.). Codex pass 3 P1 + Claude pass 3 P1 #4: the
    // earlier marker-before-show ordering meant a single failed
    // dispatch permanently suppressed the only consent surface for
    // the upgrade-on default. Retrying on every launch is mildly
    // annoying but strictly better than silently hijacking a chord
    // the user can't recover from.
    let delivery_result = app
        .notification()
        .builder()
        .title("Minutes command palette")
        .body(body)
        .show();

    match delivery_result {
        Ok(_) => {
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&marker, "shown\n") {
                eprintln!(
                    "[palette] could not write first-run marker {}: {}",
                    marker.display(),
                    e
                );
            }
        }
        Err(e) => {
            // Don't write the marker. The fallback consent surface is
            // the visible "Minutes Palette" branding inside the
            // overlay itself plus the dedicated Settings UI row that
            // landed in this same slice. A user who hits ⌘⇧K
            // expecting VS Code's Delete Line will at least see
            // "Minutes Palette" in the overlay header and can find
            // the toggle in Settings → Command Palette.
            eprintln!(
                "[palette] first-run notification failed: {} (will retry on next launch)",
                e
            );
        }
    }
}

/// Render an Accelerator-style shortcut string ("CmdOrCtrl+Shift+K")
/// as a more readable form ("⌘⇧K"). Used in the first-run notice so
/// the user can mentally match it to the symbol they'd hit on the
/// keyboard.
fn humanize_shortcut(shortcut: &str) -> String {
    shortcut
        .split('+')
        .map(|piece| match piece {
            "CmdOrCtrl" | "Cmd" | "Command" | "Meta" => "⌘".to_string(),
            "Shift" => "⇧".to_string(),
            "Alt" | "Option" | "Opt" => "⌥".to_string(),
            "Ctrl" | "Control" => "⌃".to_string(),
            "Space" => "Space".to_string(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join("")
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

    if state
        .update_install_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("An update is already in progress.".into());
    }
    state.update_install_cancel.store(false, Ordering::SeqCst);

    let initial_pending = state
        .pending_update
        .lock()
        .ok()
        .and_then(|guard| guard.clone());

    let initial_ui = initial_pending
        .as_ref()
        .map(|pending| UpdateUiState::available(pending.version.clone(), pending.download_bytes))
        .unwrap_or_default()
        .checking();
    let _ = set_update_ui_state(&app, &state, initial_ui);

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app_handle.state::<AppState>();
        let result = async {
            let updater = app_handle
                .updater()
                .map_err(|e| UpdateInstallError::Message(e.to_string()))?;
            let update = updater
                .check()
                .await
                .map_err(|e| UpdateInstallError::Message(e.to_string()))?
                .ok_or_else(|| UpdateInstallError::Message("No update available.".into()))?;

            let version = update.version.clone();
            let pending = PendingUpdate {
                version: version.clone(),
                body: update.body.clone().unwrap_or_default(),
                download_bytes: fetch_update_download_size(&update.download_url).await,
            };
            if let Ok(mut guard) = state.pending_update.lock() {
                *guard = Some(pending.clone());
            }
            emit_update_ready(&app_handle, &pending);

            let downloading = UpdateUiState::available(version.clone(), pending.download_bytes)
                .downloading(pending.download_bytes);
            let _ = set_update_ui_state(&app_handle, &state, downloading.clone());

            let bytes = download_update_bytes(
                &update,
                &state.update_install_cancel,
                |downloaded_bytes, total_bytes, bytes_per_sec, eta_seconds| {
                    let progress_state =
                        UpdateUiState::available(version.clone(), pending.download_bytes)
                            .with_progress(
                                downloaded_bytes,
                                total_bytes.or(pending.download_bytes),
                                bytes_per_sec,
                                eta_seconds,
                            );
                    let _ = set_update_ui_state(&app_handle, &state, progress_state);
                },
            )
            .await?;

            let total_bytes = pending.download_bytes.or(Some(bytes.len() as u64));
            let _ = set_update_ui_state(
                &app_handle,
                &state,
                UpdateUiState::available(version.clone(), total_bytes)
                    .verifying(bytes.len() as u64, total_bytes),
            );
            let pubkey = updater_pubkey().map_err(UpdateInstallError::Message)?;
            verify_update_signature(&bytes, &update.signature, &pubkey)
                .map_err(UpdateInstallError::Message)?;

            let _ = set_update_ui_state(
                &app_handle,
                &state,
                UpdateUiState::available(version.clone(), total_bytes)
                    .installing(bytes.len() as u64, total_bytes),
            );
            update.install(&bytes).map_err(|e| {
                UpdateInstallError::Message(format!("Update install failed: {}", e))
            })?;

            if let Ok(mut pending) = state.pending_update.lock() {
                *pending = None;
            }

            let _ = set_update_ui_state(
                &app_handle,
                &state,
                UpdateUiState::available(version.clone(), total_bytes)
                    .ready(bytes.len() as u64, total_bytes),
            );
            eprintln!("[updater] v{} installed, restarting", version);
            std::thread::sleep(Duration::from_millis(700));
            app_handle.restart();
            #[allow(unreachable_code)]
            Ok::<(), UpdateInstallError>(())
        }
        .await;

        if let Err(error) = result {
            match error {
                UpdateInstallError::Cancelled => {
                    if let Ok(mut guard) = state.update_install_state.lock() {
                        *guard = UpdateUiState::default();
                    }
                    if let Ok(guard) = state.pending_update.lock() {
                        if let Some(pending) = guard.as_ref() {
                            emit_update_ready(&app_handle, pending);
                        }
                    }
                }
                UpdateInstallError::Message(message) => {
                    let current = state
                        .update_install_state
                        .lock()
                        .ok()
                        .map(|guard| guard.clone())
                        .unwrap_or_default();
                    let _ = set_update_ui_state(&app_handle, &state, current.failed(message, true));
                }
            }
        }

        state.update_install_cancel.store(false, Ordering::SeqCst);
        state.update_install_running.store(false, Ordering::SeqCst);
    });

    Ok(serde_json::json!({"started": true}))
}

#[tauri::command]
pub fn cmd_cancel_update_install(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !state.update_install_running.load(Ordering::SeqCst) {
        return Err("No update is currently in progress.".into());
    }
    let can_cancel = state
        .update_install_state
        .lock()
        .map_err(|_| "update state lock poisoned".to_string())?
        .can_cancel;
    if !can_cancel {
        return Err("Update can no longer be canceled.".into());
    }
    state.update_install_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub fn cmd_debug_simulate_update(app: tauri::AppHandle, scenario: String) -> Result<(), String> {
    if !app.config().identifier.contains(".dev") {
        return Err("Debug updater simulation is only available in Minutes Dev.app.".into());
    }
    debug_emit_update_state(&app, &scenario)
}

pub fn debug_emit_update_state(app: &tauri::AppHandle, scenario: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let version = state
        .pending_update
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|pending| pending.version.clone()))
        .unwrap_or_else(|| "0.0.0-dev".to_string());
    let available_version = version.clone();
    let total = Some(48 * 1024 * 1024_u64);
    let next = match scenario {
        "available" => UpdateUiState::available(available_version.clone(), total),
        "checking" => UpdateUiState::available(version, total).checking(),
        "downloading" => UpdateUiState::available(version, total).with_progress(
            12 * 1024 * 1024,
            total,
            Some(1.4 * 1024.0 * 1024.0),
            Some(26),
        ),
        "verifying" => UpdateUiState::available(version, total).verifying(48 * 1024 * 1024, total),
        "installing" => {
            UpdateUiState::available(version, total).installing(48 * 1024 * 1024, total)
        }
        "ready" => UpdateUiState::available(version, total).ready(48 * 1024 * 1024, total),
        "error" => UpdateUiState::available(version, total).failed(
            "Update download stalled. Check your connection and try again.",
            true,
        ),
        _ => return Err("Unknown debug scenario.".into()),
    };
    if scenario == "available" {
        emit_update_ready(
            app,
            &PendingUpdate {
                version: available_version,
                body: String::new(),
                download_bytes: total,
            },
        );
    }
    set_update_ui_state(app, &state, next)
}

// ─────────────────────────────────────────────────────────────────────
// What's New (post-update release notes)
// ─────────────────────────────────────────────────────────────────────

fn github_release_url_for_version(version: &str) -> String {
    format!(
        "https://github.com/silverstein/minutes/releases/tag/v{}",
        version
    )
}

fn fetch_github_release_notes(version: &str) -> String {
    let tag = format!("v{}", version);
    let url = format!(
        "https://api.github.com/repos/silverstein/minutes/releases/tags/{}",
        tag
    );

    match ureq::get(&url)
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", "minutes-desktop")
        .call()
    {
        Ok(response) => response
            .into_body()
            .read_to_string()
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|v| v.get("body").and_then(|b| b.as_str()).map(String::from))
            .unwrap_or_default(),
        Err(_) => String::new(),
    }
}

/// Check whether the user should see "What's New" after an update.
///
/// Reads `~/.minutes/whats-new.json` for `last_seen_version`, compares it
/// to the running app version. If different, fetches the matching GitHub
/// release notes and returns them. Offline or 404 → still shows the
/// version bump, just without a body.
#[tauri::command]
pub async fn cmd_check_whats_new(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let current = app.config().version.clone().unwrap_or_default();
    if current.is_empty() {
        return Ok(serde_json::json!({ "show": false }));
    }

    let state_path = Config::minutes_dir().join("whats-new.json");
    let last_seen = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("last_seen_version")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .unwrap_or_default();

    if last_seen == current {
        return Ok(serde_json::json!({ "show": false }));
    }

    // First launch ever → record version, don't show notes
    if last_seen.is_empty() {
        let payload = serde_json::json!({ "last_seen_version": current });
        let _ = std::fs::write(
            &state_path,
            serde_json::to_string_pretty(&payload).unwrap_or_default(),
        );
        return Ok(serde_json::json!({ "show": false }));
    }

    // Version changed → fetch release notes from GitHub
    let body = fetch_github_release_notes(&current);
    let release_url = github_release_url_for_version(&current);

    Ok(serde_json::json!({
        "show": true,
        "version": current,
        "previousVersion": last_seen,
        "body": body,
        "url": release_url,
    }))
}

/// Fetch the current version's release notes for a manual "What's New" view.
#[tauri::command]
pub async fn cmd_get_whats_new(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let current = app.config().version.clone().unwrap_or_default();
    if current.is_empty() {
        return Err("App version is unavailable.".into());
    }

    let state_path = Config::minutes_dir().join("whats-new.json");
    let last_seen = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("last_seen_version")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .unwrap_or_default();

    Ok(serde_json::json!({
        "show": true,
        "version": current,
        "previousVersion": last_seen,
        "body": fetch_github_release_notes(&current),
        "url": github_release_url_for_version(&current),
    }))
}

/// Mark the current version as seen so the modal won't show again.
#[tauri::command]
pub async fn cmd_dismiss_whats_new(app: tauri::AppHandle) -> Result<(), String> {
    let current = app.config().version.clone().unwrap_or_default();
    let state_path = Config::minutes_dir().join("whats-new.json");
    let payload = serde_json::json!({ "last_seen_version": current });
    std::fs::write(
        &state_path,
        serde_json::to_string_pretty(&payload).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())
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
/// - `Open`    → `Closing` → destroy window → `Closed`
/// - `Opening` → ignore (duplicate press mid-create)
/// - `Closing` → queue a reopen; when destroy completes, transition
///   `Closed → Opening` immediately
///
/// All transitions happen under a `Mutex`. The window is destroyed
/// via `WebviewWindow::destroy` (not `close`) so the tear-down is
/// synchronous: codex pass 3 caught that `close()` only enqueues a
/// `RunEvent::CloseRequested` message which the runtime processes on
/// its own schedule, leaving a brief window where the OLD instance is
/// still live and a reopen race could attach to a window that is
/// about to disappear. `destroy()` skips the close-request event and
/// removes the window immediately.
pub fn toggle_palette_window(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();

    let transition: Option<PaletteTransition> = {
        let mut lifecycle = lock_or_recover(&state.palette_lifecycle);
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

/// Lock helper that recovers from a poisoned `PaletteLifecycle` mutex
/// instead of dropping the hotkey on the floor. Codex pass 3 P2:
/// `finalize_palette_open` and the close path were silently strand
/// the state machine in `Opening` if any prior call panicked while
/// holding the lock. Recovering the inner guard via `into_inner()`
/// keeps the palette responsive even after a transient poison.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            eprintln!("[palette] lifecycle mutex was poisoned; recovering");
            poisoned.into_inner()
        }
    }
}

/// Destroy the palette window synchronously and drain any queued
/// reopen request. Both `palette_close` (the webview's Esc key and
/// focus-lost paths) and the shortcut-toggle close path funnel
/// through here. Idempotent — safe to call when no palette window
/// exists.
pub fn close_palette_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("palette") {
        // `destroy()` is the synchronous tear-down. `close()` only
        // enqueues a CloseRequested event which the runtime processes
        // later, leaving the old window briefly alive — that's the
        // race codex pass 3 caught. `destroy()` removes the window
        // immediately so the next `get_webview_window("palette")`
        // returns None.
        if let Err(e) = win.destroy() {
            eprintln!("[palette] failed to destroy palette window: {}", e);
        }
    }

    let reopen = {
        let state = app.state::<AppState>();
        let mut lifecycle = lock_or_recover(&state.palette_lifecycle);
        *lifecycle = PaletteLifecycle::Closed;
        state.palette_reopen_pending.swap(false, Ordering::Relaxed)
    };

    if reopen {
        let state = app.state::<AppState>();
        let should_reopen = {
            let mut lifecycle = lock_or_recover(&state.palette_lifecycle);
            if *lifecycle == PaletteLifecycle::Closed {
                *lifecycle = PaletteLifecycle::Opening;
                true
            } else {
                false
            }
        };
        if should_reopen {
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
    //
    // **Honest caveat** (codex pass 3 P2): `AssertUnwindSafe` here is
    // not a magic recovery story — `AppHandle` contains internal
    // Arcs/Mutexes managed by Tauri, and a panic inside `build()`
    // could leave Tauri's `WindowManager` in an inconsistent state.
    // The catch_unwind only ensures our `palette_lifecycle` flag
    // resets so the user can press the hotkey again. The "right" fix
    // is to never panic in there, which is a deeper Tauri-runtime
    // concern. We accept this trade-off because the alternative —
    // stranding the user with a wedged hotkey — is strictly worse.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        create_or_show_palette_window_inner(app)
    }));
    if let Err(panic) = result {
        eprintln!("[palette] window creation panicked: {:?}", panic);
        let state = app.state::<AppState>();
        let mut lifecycle = lock_or_recover(&state.palette_lifecycle);
        *lifecycle = PaletteLifecycle::Closed;
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
            let mut lifecycle = lock_or_recover(&state.palette_lifecycle);
            *lifecycle = PaletteLifecycle::Closed;
        }
    }
}

fn finalize_palette_open(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let mut lifecycle = lock_or_recover(&state.palette_lifecycle);
    *lifecycle = PaletteLifecycle::Open;

    // Capability smoke test was a D4 dev affordance — kept on debug
    // builds only so prod users don't see the green indicator and so
    // we don't ship dev cruft. Codex pass 3 P3 + claude P3 #18 + #20
    // both flagged this as ship-noise.
    #[cfg(debug_assertions)]
    {
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

#[cfg(test)]
mod update_ui_tests {
    use super::*;

    #[test]
    fn update_ui_state_tracks_phase_transitions() {
        let available = UpdateUiState::available("0.12.0", Some(48 * 1024 * 1024));
        assert_eq!(available.phase, UpdatePhase::Available);
        assert_eq!(available.version.as_deref(), Some("0.12.0"));
        assert!(!available.can_cancel);

        let checking = available.checking();
        assert_eq!(checking.phase, UpdatePhase::Checking);
        assert!(checking.can_cancel);

        let downloading = checking.with_progress(
            12 * 1024 * 1024,
            Some(48 * 1024 * 1024),
            Some(1.5 * 1024.0 * 1024.0),
            Some(24),
        );
        assert_eq!(downloading.phase, UpdatePhase::Downloading);
        assert_eq!(downloading.downloaded_bytes, 12 * 1024 * 1024);
        assert!(downloading.can_cancel);

        let verifying = downloading.verifying(48 * 1024 * 1024, Some(48 * 1024 * 1024));
        assert_eq!(verifying.phase, UpdatePhase::Verifying);
        assert!(!verifying.can_cancel);

        let installing = verifying.installing(48 * 1024 * 1024, Some(48 * 1024 * 1024));
        assert_eq!(installing.phase, UpdatePhase::Installing);
        assert!(!installing.can_cancel);

        let ready = installing.ready(48 * 1024 * 1024, Some(48 * 1024 * 1024));
        assert_eq!(ready.phase, UpdatePhase::Ready);
        assert!(!ready.can_cancel);
    }

    #[test]
    fn update_ui_state_failure_preserves_context() {
        let base = UpdateUiState::available("0.12.0", Some(1024)).with_progress(
            256,
            Some(1024),
            Some(128.0),
            Some(6),
        );
        let failed = base.failed("network stalled", true);

        assert_eq!(failed.phase, UpdatePhase::Error);
        assert_eq!(failed.version.as_deref(), Some("0.12.0"));
        assert_eq!(failed.total_bytes, Some(1024));
        assert_eq!(failed.downloaded_bytes, 256);
        assert_eq!(failed.error_message.as_deref(), Some("network stalled"));
        assert!(failed.recoverable);
        assert!(!failed.can_cancel);
    }
}
