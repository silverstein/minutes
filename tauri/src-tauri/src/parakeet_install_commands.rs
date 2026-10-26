//! One-click install of Parakeet v3 from the desktop app.
//!
//! Until this existed the model could only be installed with
//! `minutes setup --sherpa`, so a desktop user on Whisper had no way in. The
//! app offers it to users who can run it ([`minutes_core::transcribe::parakeet_install_offered`])
//! in Settings, and invites them once per app version with a banner. The
//! download is pinned and digest-checked
//! ([`minutes_core::sherpa_engine::install_pinned_model`]); installing also
//! moves a saved `engine = "whisper"`, the default before 2026-09, to `auto`,
//! since asking for Parakeet is the reason to install it.
use minutes_core::config::Config;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

const INVITE_STATE_FILE: &str = "parakeet-invite.json";

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallProgress {
    /// "idle", "downloading", "done" or "error".
    phase: &'static str,
    done: u64,
    total: u64,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParakeetInstallStatus {
    /// Settings shows the install button.
    offered: bool,
    /// Show the invitation banner: offered and not dismissed for this version.
    invite: bool,
    /// The configured engine is a saved "whisper" that installing moves to "auto".
    switches_from_whisper: bool,
    download_bytes: u64,
    phase: &'static str,
    done: u64,
    total: u64,
    error: Option<String>,
}

static INSTALLING: AtomicBool = AtomicBool::new(false);
static PROGRESS: Mutex<InstallProgress> = Mutex::new(InstallProgress {
    phase: "idle",
    done: 0,
    total: 0,
    error: None,
});

fn set_progress(update: impl FnOnce(&mut InstallProgress)) {
    if let Ok(mut progress) = PROGRESS.lock() {
        update(&mut progress);
    }
}

fn dismissed_version() -> Option<String> {
    let raw = std::fs::read_to_string(Config::minutes_dir().join(INVITE_STATE_FILE)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("dismissed_version")
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn app_version(app: &tauri::AppHandle) -> String {
    app.config().version.clone().unwrap_or_default()
}

#[tauri::command]
pub fn cmd_parakeet_install_status(app: tauri::AppHandle) -> ParakeetInstallStatus {
    let config = Config::load();
    let installing = INSTALLING.load(Ordering::SeqCst);
    let offered = installing || minutes_core::transcribe::parakeet_install_offered(&config);
    let progress = PROGRESS.lock().map(|p| p.clone()).unwrap_or_default();
    ParakeetInstallStatus {
        offered,
        invite: offered && dismissed_version().as_deref() != Some(app_version(&app).as_str()),
        switches_from_whisper: config.transcription.engine.eq_ignore_ascii_case("whisper"),
        download_bytes: minutes_core::sherpa_engine::pinned_model_bytes(),
        phase: progress.phase,
        done: progress.done,
        total: progress.total,
        error: progress.error,
    }
}

/// Hide the banner until the next app version. Settings keeps the button.
#[tauri::command]
pub fn cmd_dismiss_parakeet_invite(app: tauri::AppHandle) -> Result<(), String> {
    let payload = serde_json::json!({ "dismissed_version": app_version(&app) });
    std::fs::write(
        Config::minutes_dir().join(INVITE_STATE_FILE),
        serde_json::to_string_pretty(&payload).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())
}

/// Start the download in the background; poll [`cmd_parakeet_install_status`].
#[tauri::command]
pub fn cmd_install_parakeet() -> Result<(), String> {
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    set_progress(|p| {
        *p = InstallProgress {
            phase: "downloading",
            ..InstallProgress::default()
        }
    });
    std::thread::spawn(|| {
        let result = install();
        log_result(&result);
        set_progress(|p| match result {
            Ok(()) => {
                p.phase = "done";
                p.error = None;
            }
            Err(error) => {
                p.phase = "error";
                p.error = Some(error);
            }
        });
        INSTALLING.store(false, Ordering::SeqCst);
    });
    Ok(())
}

fn install() -> Result<(), String> {
    let config = Config::load();
    minutes_core::sherpa_engine::install_pinned_model(&config, &mut |done, total| {
        set_progress(|p| {
            p.done = done;
            p.total = total;
        })
    })?;
    let mut config = Config::load();
    if config.transcription.engine.eq_ignore_ascii_case("whisper") {
        config.transcription.engine = "auto".into();
        config.save().map_err(|e| {
            format!("Parakeet v3 is installed, but switching from Whisper failed: {e}")
        })?;
    }
    let resolution = minutes_core::transcribe::transcription_engine_resolution(&config);
    if resolution.engine != "sherpa" {
        return Err(format!(
            "Parakeet v3 is installed, but Minutes still uses Whisper ({}).",
            resolution.reason
        ));
    }
    Ok(())
}

fn log_result(result: &Result<(), String>) {
    minutes_core::logging::append_log(&serde_json::json!({
        "ts": chrono::Local::now().to_rfc3339(),
        "level": if result.is_ok() { "info" } else { "warn" },
        "step": "parakeet_install",
        "file": "",
        "extra": { "error": result.as_ref().err() },
    }))
    .ok();
}
