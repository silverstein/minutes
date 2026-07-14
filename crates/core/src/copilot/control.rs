use super::{CopilotFeedback, CopilotHealth, CopilotState};
use crate::config::Config;
use crate::error::PidError;
use crate::pid::{self, PidGuard};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CopilotEvidenceMode {
    #[default]
    FinalOnly,
    InProcessPartials,
}

impl CopilotEvidenceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FinalOnly => "final_only",
            Self::InProcessPartials => "in_process_partials",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotSessionStatus {
    pub active: bool,
    pub pid: Option<u32>,
    pub goal: String,
    pub surface: String,
    pub cursor: u64,
    #[serde(default)]
    pub evidence_mode: CopilotEvidenceMode,
    pub capture_attachment: String,
    pub health: CopilotHealth,
    pub updated_ts: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotFeedbackRequest {
    pub nudge_id: String,
    pub feedback: CopilotFeedback,
}

impl Default for CopilotSessionStatus {
    fn default() -> Self {
        Self {
            active: false,
            pid: None,
            goal: String::new(),
            surface: "tui".into(),
            cursor: 0,
            evidence_mode: CopilotEvidenceMode::FinalOnly,
            capture_attachment: "not attached".into(),
            health: CopilotHealth {
                state: CopilotState::Off,
                provider: String::new(),
                model: String::new(),
                session_epoch: 0,
                in_flight_revision: None,
                latest_evidence_revision: None,
                last_error: None,
                policy: super::PolicySnapshot::default(),
                latency_records: Vec::new(),
                updated_ts: Utc::now(),
            },
            updated_ts: Utc::now(),
        }
    }
}

pub fn copilot_pid_path() -> PathBuf {
    Config::minutes_dir().join("copilot.pid")
}

pub fn copilot_pause_path() -> PathBuf {
    Config::minutes_dir().join("copilot.pause")
}

pub fn copilot_stop_path() -> PathBuf {
    Config::minutes_dir().join("copilot.stop")
}

pub fn copilot_status_path() -> PathBuf {
    Config::minutes_dir().join("copilot-status.json")
}

pub fn copilot_feedback_path() -> PathBuf {
    Config::minutes_dir().join("copilot.feedback.json")
}

pub fn create_session_guard() -> Result<PidGuard, PidError> {
    pid::create_pid_guard(&copilot_pid_path())
}

pub fn clear_session_controls() -> std::io::Result<()> {
    for path in [
        copilot_pause_path(),
        copilot_stop_path(),
        copilot_feedback_path(),
    ] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub fn request_pause() -> std::io::Result<()> {
    write_control(copilot_pause_path(), "paused")
}

pub fn request_resume() -> std::io::Result<()> {
    match std::fs::remove_file(copilot_pause_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn request_stop() -> std::io::Result<()> {
    write_control(copilot_stop_path(), "stop")
}

pub fn request_feedback(request: &CopilotFeedbackRequest) -> std::io::Result<()> {
    let json =
        serde_json::to_string(request).map_err(|error| std::io::Error::other(error.to_string()))?;
    write_control(copilot_feedback_path(), &json)
}

pub fn take_feedback_request() -> std::io::Result<Option<CopilotFeedbackRequest>> {
    let path = copilot_feedback_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|error| std::io::Error::other(error.to_string()))
}

fn write_control(path: PathBuf, value: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, value)
}

pub fn write_session_status(status: &CopilotSessionStatus) -> std::io::Result<()> {
    let path = copilot_status_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(status)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    std::fs::write(path, json)
}

pub fn read_session_status() -> CopilotSessionStatus {
    let mut status = std::fs::read_to_string(copilot_status_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<CopilotSessionStatus>(&raw).ok())
        .unwrap_or_default();
    let pid_state = pid::inspect_pid_file(&copilot_pid_path());
    status.active = pid_state.is_active();
    status.pid = pid_state.pid();
    if !status.active {
        status.health.state = CopilotState::Off;
        status.health.in_flight_revision = None;
    }
    status
}
