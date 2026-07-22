use crate::capture::RecordingIntent;
use crate::config::Config;
use crate::pid::CaptureMode;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub fn control_dir() -> PathBuf {
    Config::minutes_dir().join("desktop-control")
}

pub fn requests_dir() -> PathBuf {
    control_dir().join("requests")
}

pub fn responses_dir() -> PathBuf {
    control_dir().join("responses")
}

pub fn desktop_app_status_path() -> PathBuf {
    control_dir().join("desktop-app.json")
}

pub fn desktop_app_status_path_for(app_identifier: &str) -> PathBuf {
    let safe_identifier = app_identifier
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    control_dir().join(format!("desktop-app-{safe_identifier}.json"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopAppStatus {
    pub pid: u32,
    pub updated_at: DateTime<Local>,
    pub platform: String,
    #[serde(default)]
    pub frontend_ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontend_error: Option<String>,
    #[serde(default)]
    pub process_started_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontend_ready_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRecordingRequest {
    pub mode: CaptureMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<RecordingIntent>,
    #[serde(default)]
    pub allow_degraded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DesktopControlAction {
    StartRecording(StartRecordingRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopControlRequest {
    pub id: String,
    pub created_at: DateTime<Local>,
    pub action: DesktopControlAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopControlResponse {
    pub id: String,
    pub handled_at: DateTime<Local>,
    pub accepted: bool,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct ClaimedDesktopControlRequest {
    pub request: DesktopControlRequest,
    pub claim_path: PathBuf,
}

fn ensure_dirs() -> std::io::Result<()> {
    fs::create_dir_all(requests_dir())?;
    fs::create_dir_all(responses_dir())?;
    Ok(())
}

pub fn write_desktop_app_status(status: &DesktopAppStatus) -> std::io::Result<()> {
    ensure_dirs()?;
    let path = desktop_app_status_path();
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(status)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

pub fn write_desktop_app_status_for(
    app_identifier: &str,
    status: &DesktopAppStatus,
) -> std::io::Result<()> {
    ensure_dirs()?;
    let path = desktop_app_status_path_for(app_identifier);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(status)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

pub fn clear_desktop_app_status() -> std::io::Result<()> {
    let path = desktop_app_status_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn read_desktop_app_status() -> Option<DesktopAppStatus> {
    let path = desktop_app_status_path();
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

pub fn desktop_app_owns_pid(pid: u32) -> bool {
    read_desktop_app_status().is_some_and(|status| {
        status.pid == pid
            && (chrono::Local::now() - status.updated_at) <= chrono::Duration::seconds(10)
    })
}

pub fn request_path(id: &str) -> PathBuf {
    requests_dir().join(format!("{}.json", id))
}

pub fn response_path(id: &str) -> PathBuf {
    responses_dir().join(format!("{}.json", id))
}

fn claimed_request_path(path: &Path, claimant: &str) -> PathBuf {
    let extension = format!("claimed-{claimant}");
    path.with_extension(extension)
}

pub fn write_request(request: &DesktopControlRequest) -> std::io::Result<()> {
    ensure_dirs()?;
    let path = request_path(&request.id);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(request)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

pub fn write_response(response: &DesktopControlResponse) -> std::io::Result<()> {
    ensure_dirs()?;
    let path = response_path(&response.id);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(response)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

pub fn remove_request(id: &str) -> std::io::Result<()> {
    let path = request_path(id);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn remove_response(id: &str) -> std::io::Result<()> {
    let path = response_path(id);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn claim_pending_requests(claimant: &str) -> Vec<ClaimedDesktopControlRequest> {
    let mut requests = Vec::new();
    let dir = requests_dir();
    if !dir.exists() {
        return requests;
    }

    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }

        let claim_path = claimed_request_path(&path, claimant);
        if fs::rename(&path, &claim_path).is_err() {
            continue;
        }

        match fs::read_to_string(&claim_path)
            .ok()
            .and_then(|text| serde_json::from_str::<DesktopControlRequest>(&text).ok())
        {
            Some(request) => requests.push(ClaimedDesktopControlRequest {
                request,
                claim_path,
            }),
            None => {
                fs::remove_file(&claim_path).ok();
            }
        }
    }

    requests.sort_by_key(|claimed| claimed.request.created_at);
    requests
}

pub fn finish_claimed_request(claim_path: &Path) -> std::io::Result<()> {
    if claim_path.exists() {
        fs::remove_file(claim_path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn claim_pending_requests_is_single_claim() {
        let _guard = crate::test_home_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let original_home = std::env::var_os("HOME");
        #[cfg(windows)]
        let original_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", dir.path());
        #[cfg(windows)]
        std::env::set_var("USERPROFILE", dir.path());

        let restore_env = || {
            if let Some(home) = original_home.as_ref() {
                std::env::set_var("HOME", home);
            } else {
                std::env::remove_var("HOME");
            }
            #[cfg(windows)]
            if let Some(userprofile) = original_userprofile.as_ref() {
                std::env::set_var("USERPROFILE", userprofile);
            } else {
                std::env::remove_var("USERPROFILE");
            }
        };

        let now = Local::now();
        let first = DesktopControlRequest {
            id: "first".into(),
            created_at: now,
            action: DesktopControlAction::StartRecording(StartRecordingRequest {
                mode: CaptureMode::Meeting,
                intent: None,
                allow_degraded: false,
                title: None,
                language: None,
            }),
        };
        let second = DesktopControlRequest {
            id: "second".into(),
            created_at: now + Duration::milliseconds(1),
            action: DesktopControlAction::StartRecording(StartRecordingRequest {
                mode: CaptureMode::Meeting,
                intent: None,
                allow_degraded: false,
                title: None,
                language: None,
            }),
        };

        write_request(&first).unwrap();
        write_request(&second).unwrap();

        let claimed = claim_pending_requests("pid-1");
        assert_eq!(
            claimed
                .iter()
                .map(|item| item.request.id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        assert!(claim_pending_requests("pid-2").is_empty());

        for item in claimed {
            finish_claimed_request(&item.claim_path).unwrap();
        }

        restore_env();
    }

    #[test]
    fn desktop_app_owns_pid_requires_recent_matching_heartbeat() {
        let _guard = crate::test_home_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let original_home = std::env::var_os("HOME");
        #[cfg(windows)]
        let original_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", dir.path());
        #[cfg(windows)]
        std::env::set_var("USERPROFILE", dir.path());

        let restore_env = || {
            if let Some(home) = original_home.as_ref() {
                std::env::set_var("HOME", home);
            } else {
                std::env::remove_var("HOME");
            }
            #[cfg(windows)]
            if let Some(userprofile) = original_userprofile.as_ref() {
                std::env::set_var("USERPROFILE", userprofile);
            } else {
                std::env::remove_var("USERPROFILE");
            }
        };

        let current_pid = std::process::id();
        write_desktop_app_status(&DesktopAppStatus {
            pid: current_pid,
            updated_at: Local::now(),
            platform: "macos".into(),
            frontend_ready: true,
            frontend_error: None,
            process_started_at_unix_ms: 1,
            frontend_ready_at_unix_ms: Some(2),
        })
        .unwrap();
        assert!(desktop_app_owns_pid(current_pid));
        assert!(!desktop_app_owns_pid(current_pid + 1));

        write_desktop_app_status(&DesktopAppStatus {
            pid: current_pid,
            updated_at: Local::now() - Duration::seconds(30),
            platform: "macos".into(),
            frontend_ready: true,
            frontend_error: None,
            process_started_at_unix_ms: 1,
            frontend_ready_at_unix_ms: Some(2),
        })
        .unwrap();
        assert!(!desktop_app_owns_pid(current_pid));

        restore_env();
    }

    #[test]
    fn legacy_desktop_status_defaults_frontend_readiness_to_false() {
        let status: DesktopAppStatus = serde_json::from_str(
            r#"{"pid":42,"updated_at":"2026-07-21T17:53:47-07:00","platform":"macos"}"#,
        )
        .unwrap();

        assert!(!status.frontend_ready);
        assert_eq!(status.frontend_error, None);
        assert_eq!(status.process_started_at_unix_ms, 0);
        assert_eq!(status.frontend_ready_at_unix_ms, None);
    }

    #[test]
    fn namespaced_desktop_status_path_sanitizes_identifier() {
        assert_eq!(
            desktop_app_status_path_for("com.useminutes.desktop.dev/../../bad")
                .file_name()
                .and_then(|name| name.to_str()),
            Some("desktop-app-com.useminutes.desktop.dev_.._.._bad.json")
        );
    }
}
