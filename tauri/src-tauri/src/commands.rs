use minutes_core::{CaptureMode, Config, ContentType};
use std::cmp::Reverse;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};
use tauri::{Emitter, Manager};

pub struct AppState {
    pub recording: Arc<AtomicBool>,
    pub starting: Arc<AtomicBool>,
    pub stop_flag: Arc<AtomicBool>,
    pub processing: Arc<AtomicBool>,
    pub processing_stage: Arc<Mutex<Option<String>>>,
    pub latest_output: Arc<Mutex<Option<OutputNotice>>>,
    pub completion_notifications_enabled: Arc<AtomicBool>,
    pub global_hotkey_enabled: Arc<AtomicBool>,
    pub global_hotkey_shortcut: Arc<Mutex<String>>,
    pub hotkey_runtime: Arc<Mutex<HotkeyRuntime>>,
    pub discard_short_hotkey_capture: Arc<AtomicBool>,
    pub pty_manager: Arc<Mutex<crate::pty::PtyManager>>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MeetingSection {
    pub heading: String,
    pub content: String,
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
    pub sections: Vec<MeetingSection>,
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
const HOTKEY_HOLD_THRESHOLD_MS: u64 = 300;
const HOTKEY_MIN_DURATION_MS: u64 = 400;

pub fn default_hotkey_shortcut() -> &'static str {
    HOTKEY_CHOICES[0].0
}

fn hotkey_choices() -> Vec<HotkeyChoice> {
    HOTKEY_CHOICES
        .iter()
        .map(|(value, label)| HotkeyChoice {
            value: (*value).to_string(),
            label: (*label).to_string(),
        })
        .collect()
}

fn validate_hotkey_shortcut(shortcut: &str) -> Result<String, String> {
    HOTKEY_CHOICES
        .iter()
        .find_map(|(value, _)| (*value == shortcut).then(|| (*value).to_string()))
        .ok_or_else(|| {
            format!(
                "Unsupported shortcut: {}. Choose one of: {}",
                shortcut,
                HOTKEY_CHOICES
                    .iter()
                    .map(|(_, label)| *label)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
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
                let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                if rc != 0 {
                    return Err(std::io::Error::last_os_error().to_string());
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

fn display_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_display = home.display().to_string();
        if let Some(stripped) = path.strip_prefix(&home_display) {
            return format!("~{}", stripped);
        }
    }
    path.to_string()
}

fn escape_applescript_literal(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
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
    let script = format!(
        "display notification \"{}\" with title \"Minutes\" subtitle \"{}\"",
        escape_applescript_literal(&body),
        escape_applescript_literal(&notice.title)
    );

    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .spawn();
}

pub fn show_user_notification(title: &str, body: &str) {
    let script = format!(
        "display notification \"{}\" with title \"Minutes\" subtitle \"{}\"",
        escape_applescript_literal(body),
        escape_applescript_literal(title)
    );

    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .spawn();
}

pub fn frontmost_application_name() -> Option<String> {
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

fn copy_to_clipboard(text: &str) -> Result<(), String> {
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

fn paste_into_application(app_name: &str) -> Result<(), String> {
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
                "{} audio input device{} detected. macOS may still prompt the first time you record.",
                devices.len(),
                if devices.len() == 1 { "" } else { "s" }
            )
        } else {
            "No audio input devices detected. Check hardware, macOS input settings, and permissions.".into()
        },
        optional: false,
    }
}

fn calendar_status() -> ReadinessItem {
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
            detail: format!("Permission denied: {}. Try Set Up Vault from the app.", path.display()),
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
        minutes_core::vault::VaultStatus::NotConfigured => {
            ("not_configured", "".into(), "".into(), "Not configured".into())
        }
        minutes_core::vault::VaultStatus::Healthy { strategy, path } => {
            let p = path.display().to_string();
            ("healthy", strategy, p.clone(), format!("Vault active at {}", p))
        }
        minutes_core::vault::VaultStatus::BrokenSymlink {
            link_path,
            target,
        } => (
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
            if path.is_file() {
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
                if path.is_file() {
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
    completion_notifications_enabled: Arc<AtomicBool>,
    hotkey_runtime: Option<Arc<Mutex<HotkeyRuntime>>>,
    discard_short_hotkey_capture: Option<Arc<AtomicBool>>,
    mode: CaptureMode,
) {
    let config = Config::load();
    let wav_path = minutes_core::pid::current_wav_path();

    if let Err(e) = minutes_core::pid::create() {
        eprintln!("Failed to create PID: {}", e);
        show_user_notification("Recording", &format!("Could not start recording: {}", e));
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
    processing.store(false, Ordering::Relaxed);
    set_processing_stage(&processing_stage, None);
    set_latest_output(&latest_output, None);
    minutes_core::pid::clear_processing_status().ok();
    minutes_core::pid::write_recording_metadata(mode).ok();
    crate::update_tray_state(&app_handle, true);

    minutes_core::notes::save_recording_start().ok();
    eprintln!("{} started...", mode.noun());

    let mut remove_current_wav = false;
    match minutes_core::capture::record_to_wav(&wav_path, stop_flag, &config) {
        Ok(()) => {
            recording.store(false, Ordering::Relaxed);
            let should_discard = discard_short_hotkey_capture
                .as_ref()
                .map(|flag| flag.swap(false, Ordering::Relaxed))
                .unwrap_or(false);
            if should_discard {
                remove_current_wav = true;
                eprintln!("Discarded short {} capture.", mode.noun());
            } else {
                processing.store(true, Ordering::Relaxed);
            }
            if !should_discard {
                match minutes_core::pipeline::process_with_progress(
                    &wav_path,
                    mode.content_type(),
                    None,
                    &config,
                    |stage| {
                        let label = stage_label(stage, mode);
                        set_processing_stage(&processing_stage, Some(label));
                        let _ = minutes_core::pid::set_processing_status(Some(label), Some(mode));
                    },
                ) {
                    Ok(result) => {
                        remove_current_wav = true;
                        let detail = match mode {
                            CaptureMode::Meeting => "Saved meeting markdown",
                            CaptureMode::QuickThought => "Saved quick thought memo",
                        };
                        let notice = OutputNotice {
                            kind: "saved".into(),
                            title: result.title.clone(),
                            path: result.path.display().to_string(),
                            detail: detail.into(),
                        };
                        set_latest_output(&latest_output, Some(notice.clone()));
                        maybe_show_completion_notification(
                            &app_handle,
                            &completion_notifications_enabled,
                            &notice,
                        );
                        eprintln!(
                            "Saved {}: {} ({} words)",
                            mode.noun(),
                            result.path.display(),
                            result.word_count
                        );
                    }
                    Err(e) => {
                        if let Some(saved) = preserve_failed_capture(&wav_path, &config) {
                            let detail = match mode {
                                CaptureMode::Meeting => {
                                    "Processing failed, but the raw meeting capture was preserved."
                                }
                                CaptureMode::QuickThought => {
                                    "Processing failed, but the raw quick thought capture was preserved."
                                }
                            };
                            let notice = OutputNotice {
                                kind: "preserved-capture".into(),
                                title: "Raw capture preserved".into(),
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
                                "Pipeline error: {}. Raw audio preserved at {}",
                                e,
                                saved.display()
                            );
                        } else {
                            eprintln!(
                                "Pipeline error: {}. Raw audio left at {}",
                                e,
                                wav_path.display()
                            );
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

    minutes_core::notes::cleanup();
    minutes_core::pid::remove().ok();
    if remove_current_wav && wav_path.exists() {
        std::fs::remove_file(&wav_path).ok();
    }
    processing.store(false, Ordering::Relaxed);
    set_processing_stage(&processing_stage, None);
    minutes_core::pid::clear_processing_status().ok();
    minutes_core::pid::clear_recording_metadata().ok();
    starting.store(false, Ordering::Relaxed);
    recording.store(false, Ordering::Relaxed);
    reset_hotkey_capture_state(
        hotkey_runtime.as_ref(),
        discard_short_hotkey_capture.as_ref(),
    );
}

fn spawn_hotkey_recording(app: &tauri::AppHandle, style: HotkeyCaptureStyle) {
    let state = app.state::<AppState>();
    state.starting.store(true, Ordering::Relaxed);
    if let Ok(mut runtime) = state.hotkey_runtime.lock() {
        runtime.active_capture = Some(style);
        runtime.recording_started_at = Some(Instant::now());
    }
    state
        .discard_short_hotkey_capture
        .store(false, Ordering::Relaxed);
    let rec = state.recording.clone();
    let starting = state.starting.clone();
    let stop = state.stop_flag.clone();
    let processing = state.processing.clone();
    let processing_stage = state.processing_stage.clone();
    let latest_output = state.latest_output.clone();
    let completion_notifications_enabled = state.completion_notifications_enabled.clone();
    let hotkey_runtime = state.hotkey_runtime.clone();
    let discard_short_hotkey_capture = state.discard_short_hotkey_capture.clone();
    let app_handle = app.clone();
    let app_done = app.clone();
    std::thread::spawn(move || {
        start_recording(
            app_handle,
            rec,
            starting,
            stop,
            processing,
            processing_stage,
            latest_output,
            completion_notifications_enabled,
            Some(hotkey_runtime),
            Some(discard_short_hotkey_capture),
            CaptureMode::QuickThought,
        );
        crate::update_tray_state(&app_done, false);
    });
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
            if minutes_core::pid::status().processing || state.processing.load(Ordering::Relaxed) {
                show_user_notification(
                    "Quick thought",
                    "Minutes is still processing the previous capture. Finish that first.",
                );
                return;
            }

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
                        "Quick thought",
                        &format!("Could not stop recording: {}", err),
                    );
                }
                return;
            }

            if minutes_core::pid::status().processing || state.processing.load(Ordering::Relaxed) {
                show_user_notification(
                    "Quick thought",
                    "Minutes is still processing the previous capture. Finish that first.",
                );
                return;
            }

            spawn_hotkey_recording(app, HotkeyCaptureStyle::Locked);
        }
    }
}

#[tauri::command]
pub fn cmd_start_recording(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    mode: Option<String>,
) -> Result<(), String> {
    if recording_active(&state.recording) || state.starting.load(Ordering::Relaxed) {
        return Err("Already recording".into());
    }
    let capture_mode = parse_capture_mode(mode.as_deref())?;
    state.starting.store(true, Ordering::Relaxed);
    let rec = state.recording.clone();
    let starting = state.starting.clone();
    let stop = state.stop_flag.clone();
    let processing = state.processing.clone();
    let processing_stage = state.processing_stage.clone();
    let latest_output = state.latest_output.clone();
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
            completion_notifications_enabled,
            None,
            None,
            capture_mode,
        );
        crate::update_tray_state(&app_done, false);
    });
    Ok(())
}

#[tauri::command]
pub fn cmd_stop_recording(state: tauri::State<AppState>) -> Result<(), String> {
    request_stop(&state.recording, &state.stop_flag)
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
        "latestOutput": latest_output,
        "pid": status.pid,
        "elapsed": elapsed,
        "audioLevel": audio_level,
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
pub fn cmd_open_file(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
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
pub fn cmd_permission_center() -> serde_json::Value {
    let config = Config::load();
    let items = vec![
        model_status(&config),
        microphone_status(),
        calendar_status(),
        watcher_status(&config),
        output_dir_status(&config),
        vault_status(&config),
    ];
    serde_json::to_value(items).unwrap_or(serde_json::json!([]))
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
) -> Result<OutputNotice, String> {
    if recording_active(&state.recording) || state.processing.load(Ordering::Relaxed) {
        return Err("Finish the current recording before retrying recovery items.".into());
    }

    let config = Config::load();
    let audio_path = PathBuf::from(&path);
    if !audio_path.exists() {
        return Err(format!("Recovery item not found: {}", path));
    }

    let ct = match content_type.as_str() {
        "meeting" => ContentType::Meeting,
        "memo" => ContentType::Memo,
        other => return Err(format!("Unsupported recovery type: {}", other)),
    };

    let result =
        minutes_core::pipeline::process_with_progress(&audio_path, ct, None, &config, |_| {})
            .map_err(|e| e.to_string())?;

    let notice = OutputNotice {
        kind: "saved".into(),
        title: result.title.clone(),
        path: result.path.display().to_string(),
        detail: "Recovery item was processed successfully.".into(),
    };
    set_latest_output(&state.latest_output, Some(notice.clone()));
    Ok(notice)
}

#[tauri::command]
pub fn cmd_get_meeting_detail(path: String) -> Result<MeetingDetail, String> {
    let config = Config::load();
    let meeting_path = std::path::PathBuf::from(&path);
    minutes_core::notes::validate_meeting_path(&meeting_path, &config.output_dir)?;

    let content = std::fs::read_to_string(&meeting_path).map_err(|e| e.to_string())?;
    let (frontmatter_str, body) = minutes_core::markdown::split_frontmatter(&content);
    let frontmatter: minutes_core::markdown::Frontmatter =
        serde_yaml::from_str(&format!("---\n{}\n---", frontmatter_str))
            .map_err(|e| e.to_string())?;

    let content_type = match frontmatter.r#type {
        ContentType::Meeting => "meeting",
        ContentType::Memo => "memo",
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
        sections: parse_sections(body),
    })
}

#[tauri::command]
pub fn cmd_upcoming_meetings() -> serde_json::Value {
    let events = minutes_core::calendar::upcoming_events(120); // 2 hour lookahead
    serde_json::to_value(&events).unwrap_or(serde_json::json!([]))
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
/// - A bare command name ("claude", "codex", "bash") — searched in common PATH dirs
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

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let search_dirs = [
        home.join(".cargo/bin"),
        home.join(".local/bin"),
        home.join(".npm-global/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ];
    for dir in &search_dirs {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
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
        let agent_bin = find_agent_binary(agent_name)
            .ok_or_else(|| {
                format!(
                    "'{}' not found. Install it or set a different agent in ~/.config/minutes/config.toml under [assistant].",
                    agent_name
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
        "transcription": {
            "model": config.transcription.model,
            "downloaded_models": downloaded_models,
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
        "call_detection": {
            "enabled": config.call_detection.enabled,
            "poll_interval_secs": config.call_detection.poll_interval_secs,
            "cooldown_minutes": config.call_detection.cooldown_minutes,
            "apps": config.call_detection.apps,
        },
    })
}

#[tauri::command]
pub fn cmd_set_setting(section: String, key: String, value: String) -> Result<String, String> {
    let mut config = Config::load();

    match (section.as_str(), key.as_str()) {
        // Transcription
        ("transcription", "model") => config.transcription.model = value.clone(),

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

        // Call detection
        ("call_detection", "enabled") => {
            config.call_detection.enabled = value == "true";
        }

        _ => return Err(format!("Unknown setting: {}.{}", section, key)),
    }

    config
        .save()
        .map_err(|e| format!("Failed to save config: {}", e))?;

    Ok(format!("Set {}.{} = {}", section, key, value))
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
