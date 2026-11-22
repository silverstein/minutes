use crate::config::Config;
use crate::events::MinutesEvent;
use crate::markdown::{
    ActionItem, CapturePolicy, ConsentBasis, ContentType, DebriefStatus, Decision, EntityLinks,
    Frontmatter, OutputStatus, Sensitivity, WriteResult,
};
use crate::pid;
use chrono::{DateTime, Local};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// Filename for the active sensitive-session state file.
pub const SENSITIVE_SESSION_FILE: &str = "sensitive-session.json";

/// Filename for the sensitive-session mutation lock.
pub const SENSITIVE_LOCK_FILE: &str = "sensitive-session.lock";

/// A marker typed by the user during a no-capture sensitive meeting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensitiveMarker {
    /// Timestamp when the marker was written.
    pub timestamp: DateTime<Local>,
    /// User-authored marker text.
    pub text: String,
}

/// The active no-capture sensitive meeting session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensitiveSession {
    /// Stable session id used in events and lock metadata.
    pub id: String,
    /// Human-provided meeting title.
    pub title: String,
    /// Session start timestamp.
    pub started_at: DateTime<Local>,
    /// PID of a long-lived owner, or zero for a session ended explicitly.
    /// A nonzero owner that is no longer alive indicates a stale session.
    #[serde(default)]
    pub owner_pid: u32,
    /// User-authored markers collected during the session.
    #[serde(default)]
    pub markers: Vec<SensitiveMarker>,
}

/// Human-written debrief content collected when a sensitive session stops.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SensitiveDebrief {
    /// Human-authored summary text.
    pub summary: Option<String>,
    /// Human-authored decision strings.
    pub decisions: Vec<String>,
    /// Human-authored action-item strings.
    pub action_items: Vec<String>,
}

/// Error returned by sensitive meeting session operations.
#[derive(Debug, thiserror::Error)]
pub enum SensitiveError {
    /// A normal recording is already active.
    #[error("recording in progress - stop it before starting a sensitive meeting")]
    RecordingActive,
    /// Another capture mode already holds the microphone.
    #[error("{0} in progress - stop it before starting a sensitive meeting")]
    CaptureActive(&'static str),
    /// A sensitive session is already active.
    #[error("sensitive meeting already active: {title}")]
    AlreadyActive {
        /// Active sensitive session title.
        title: String,
    },
    /// No sensitive session is active.
    #[error("no sensitive meeting in progress")]
    NotActive,
    /// Marker text was empty after trimming.
    #[error("marker text is empty")]
    EmptyMarker,
    /// Filesystem or serialization failed.
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for SensitiveError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

impl From<serde_json::Error> for SensitiveError {
    fn from(value: serde_json::Error) -> Self {
        Self::Io(value.to_string())
    }
}

impl From<crate::error::MarkdownError> for SensitiveError {
    fn from(value: crate::error::MarkdownError) -> Self {
        Self::Io(value.to_string())
    }
}

/// Path to the active sensitive-session state file.
pub fn session_path() -> PathBuf {
    Config::minutes_dir().join(SENSITIVE_SESSION_FILE)
}

/// Path to the sensitive-session mutation lock file.
pub fn lock_path() -> PathBuf {
    Config::minutes_dir().join(SENSITIVE_LOCK_FILE)
}

/// Return true when a sensitive session file is active.
pub fn is_active() -> bool {
    active_session().is_some()
}

/// Read the active sensitive session, if any.
///
/// A session whose owning process is dead is a crash leftover: it is logged,
/// removed, and reported as no-session, so a crashed sensitive session can
/// never permanently block recording. A zero owner covers explicit-stop
/// sessions and legacy session files.
pub fn active_session() -> Option<SensitiveSession> {
    with_lock(active_session_locked).ok().flatten()
}

fn active_session_locked() -> Result<Option<SensitiveSession>, SensitiveError> {
    let Some(session) = read_session()? else {
        return Ok(None);
    };
    if !session_owner_is_alive(&session) {
        tracing::warn!(
            session_id = %session.id,
            owner_pid = session.owner_pid,
            "removing stale sensitive session left by a dead process"
        );
        remove_session_file()?;
        return Ok(None);
    }
    Ok(Some(session))
}

/// Report session activity without waiting for the capture or mutation lock.
pub fn status_is_active() -> bool {
    if !session_path().exists() {
        return false;
    }
    let lock = fs::File::open(lock_path()).ok();
    if lock
        .as_ref()
        .is_some_and(|file| FileExt::try_lock_shared(file).is_err())
    {
        // A concurrent update can temporarily leave the session file unreadable.
        // Treat the visible marker as active until the writer finishes.
        return true;
    }
    read_session()
        .ok()
        .flatten()
        .as_ref()
        .is_some_and(session_owner_is_alive)
}

fn session_owner_is_alive(session: &SensitiveSession) -> bool {
    session.owner_pid == 0 || crate::pid::is_process_alive(session.owner_pid)
}

fn read_session() -> Result<Option<SensitiveSession>, SensitiveError> {
    let raw = match fs::read_to_string(session_path()) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(Some(serde_json::from_str(&raw)?))
}

/// Start a no-capture session owned by the running desktop app.
pub fn start(title: Option<&str>) -> Result<SensitiveSession, SensitiveError> {
    start_with_owner(title, std::process::id())
}

/// Start a session that persists until the user stops it in a later invocation.
pub fn start_until_stopped(title: Option<&str>) -> Result<SensitiveSession, SensitiveError> {
    start_with_owner(title, 0)
}

fn start_with_owner(
    title: Option<&str>,
    owner_pid: u32,
) -> Result<SensitiveSession, SensitiveError> {
    with_lock(|| {
        ensure_capture_inactive()?;
        if let Some(existing) = active_session_locked()? {
            return Err(SensitiveError::AlreadyActive {
                title: existing.title,
            });
        }

        let now = Local::now();
        let session = SensitiveSession {
            id: format!(
                "sensitive-{}-{}",
                now.format("%Y%m%d%H%M%S"),
                std::process::id()
            ),
            title: title
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("Sensitive meeting")
                .to_string(),
            started_at: now,
            owner_pid,
            markers: Vec::new(),
        };
        write_session(&session)?;
        Ok(session)
    })
}

/// Append a typed marker to the active sensitive session.
pub fn add_marker(text: &str) -> Result<String, SensitiveError> {
    with_lock(|| {
        let mut session = active_session_locked()?.ok_or(SensitiveError::NotActive)?;
        let text = text.trim();
        if text.is_empty() {
            return Err(SensitiveError::EmptyMarker);
        }

        let marker = SensitiveMarker {
            timestamp: Local::now(),
            text: text.to_string(),
        };
        let rendered = format!(
            "[{}] {}",
            elapsed_label(session.started_at, marker.timestamp),
            text
        );
        // Bus first, fallibly: markers ride the event bus by spec, so a
        // failed append fails the command rather than silently diverging.
        // An orphan bus event (if the session write below then failed) is
        // harmless observability; a session marker missing from the bus is
        // a contract violation (review F5).
        crate::events::append_event_strict(MinutesEvent::SensitiveMarker {
            session_id: session.id.clone(),
            text: marker.text.clone(),
        })?;
        session.markers.push(marker);
        write_session(&session)?;
        Ok(rendered)
    })
}

/// Stop the active sensitive session and write its meeting artifact.
pub fn stop(
    debrief: Option<SensitiveDebrief>,
    config: &Config,
) -> Result<WriteResult, SensitiveError> {
    with_lock(|| {
        let session = active_session_locked()?.ok_or(SensitiveError::NotActive)?;
        let result = write_artifact(&session, debrief, config)?;
        remove_session_file()?;
        Ok(result)
    })
}

/// Serialize the check with sensitive start and stop. Capture callers check
/// again after publishing their PID, before opening the microphone.
pub fn ensure_inactive_for_recording() -> Result<(), SensitiveError> {
    with_inactive_for_recording(|| Ok(()))
}

/// Hold the sensitive-session lock while a startup audio probe runs, before
/// the recording has published its PID.
pub fn with_inactive_for_recording<T, E: From<SensitiveError>>(
    action: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    with_lock(|| match active_session_locked()? {
        Some(existing) => Err(SensitiveError::AlreadyActive {
            title: existing.title,
        }),
        None => Ok(action()),
    })?
}

fn with_lock<T>(f: impl FnOnce() -> Result<T, SensitiveError>) -> Result<T, SensitiveError> {
    let path = lock_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    file.lock_exclusive()?;
    let result = f();
    file.unlock().ok();
    result
}

fn write_session(session: &SensitiveSession) -> Result<(), SensitiveError> {
    let path = session_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(session)?;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(json.as_bytes())?;
    file.flush()?;
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn remove_session_file() -> Result<(), SensitiveError> {
    let path = session_path();
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn ensure_capture_inactive() -> Result<(), SensitiveError> {
    if pid::inspect_pid_file(&pid::pid_path()).is_active() {
        return Err(SensitiveError::RecordingActive);
    }
    for (path, mode) in [
        (pid::dictation_pid_path(), "dictation"),
        (pid::live_transcript_pid_path(), "live transcript"),
        (pid::voice_pid_path(), "voice session"),
    ] {
        if pid::inspect_pid_file(&path).is_active() {
            return Err(SensitiveError::CaptureActive(mode));
        }
    }
    Ok(())
}

fn elapsed_label(started_at: DateTime<Local>, timestamp: DateTime<Local>) -> String {
    let elapsed = timestamp
        .signed_duration_since(started_at)
        .num_seconds()
        .max(0) as u64;
    format!("{}:{:02}", elapsed / 60, elapsed % 60)
}

fn render_markers(session: &SensitiveSession) -> Option<String> {
    if session.markers.is_empty() {
        return None;
    }
    Some(
        session
            .markers
            .iter()
            .map(|marker| {
                format!(
                    "[{}] {}",
                    elapsed_label(session.started_at, marker.timestamp),
                    marker.text.trim()
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn write_artifact(
    session: &SensitiveSession,
    debrief: Option<SensitiveDebrief>,
    config: &Config,
) -> Result<WriteResult, SensitiveError> {
    let stopped_at = Local::now();
    let duration_secs = stopped_at
        .signed_duration_since(session.started_at)
        .num_seconds()
        .max(0) as u64;
    let duration = format!("{}m {}s", duration_secs / 60, duration_secs % 60);
    let debrief = debrief.unwrap_or_default();
    let summary = debrief
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let decisions = debrief
        .decisions
        .into_iter()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .map(|text| Decision {
            id: None,
            effective_date: None,
            text,
            topic: None,
            authority: None,
            supersedes: None,
        })
        .collect::<Vec<_>>();
    let action_items = debrief
        .action_items
        .into_iter()
        .map(|task| task.trim().to_string())
        .filter(|task| !task.is_empty())
        .map(|task| ActionItem {
            assignee: "Unassigned".into(),
            task,
            due: None,
            status: "open".into(),
        })
        .collect::<Vec<_>>();
    let debrief_pending = summary.is_none() && decisions.is_empty() && action_items.is_empty();

    let frontmatter = Frontmatter {
        title: session.title.clone(),
        r#type: ContentType::Meeting,
        date: session.started_at,
        duration,
        source: Some("sensitive".into()),
        status: Some(OutputStatus::Complete),
        processing_warnings: Vec::new(),
        tags: vec![],
        attendees: vec![],
        attendees_raw: None,
        calendar_event: None,
        people: vec![],
        entities: EntityLinks::default(),
        device: None,
        captured_at: None,
        context: None,
        action_items,
        decisions,
        intents: vec![],
        recorded_by: config.identity.name.clone(),
        capture: Some(CapturePolicy::None),
        sensitivity: Some(Sensitivity::Restricted),
        debrief: debrief_pending.then_some(DebriefStatus::Pending),
        consent: Some(ConsentBasis::NotApplicable),
        consent_notice: None,
        visibility: None,
        speaker_map: vec![],
        name_corrections: Vec::new(),
        spelling_corrections: Vec::new(),
        recording_health: None,
        speaker_mapping: None,
        summarization: None,
        template: None,
        filter_diagnosis: None,
    };
    let transcript = "Audio was not captured for this sensitive meeting.";
    crate::markdown::write(
        &frontmatter,
        transcript,
        summary.as_deref(),
        render_markers(session).as_deref(),
        config,
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn with_temp_home<T>(f: impl FnOnce(&Config) -> T) -> T {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        let meetings = dir.path().join("meetings");
        let previous_home = std::env::var_os("HOME");
        let previous_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", dir.path());
        std::env::set_var("USERPROFILE", dir.path());
        let config = Config {
            output_dir: meetings,
            ..Config::default()
        };
        let result = f(&config);
        if let Some(home) = previous_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        if let Some(userprofile) = previous_userprofile {
            std::env::set_var("USERPROFILE", userprofile);
        } else {
            std::env::remove_var("USERPROFILE");
        }
        result
    }

    #[test]
    fn start_add_marker_and_stop_writes_no_capture_frontmatter() {
        with_temp_home(|config| {
            let session = start(Some("Board sync")).unwrap();
            assert_eq!(session.title, "Board sync");
            let line = add_marker("Opened with roadmap risk").unwrap();
            assert!(line.contains("Opened with roadmap risk"));
            let events = crate::events::read_events(None, None);
            assert!(events.iter().any(|envelope| matches!(
                &envelope.event,
                MinutesEvent::SensitiveMarker { text, .. } if text == "Opened with roadmap risk"
            )));

            let result = stop(
                Some(SensitiveDebrief {
                    summary: Some("We discussed roadmap risk.".into()),
                    decisions: vec!["Keep the current launch window".into()],
                    action_items: vec!["Send revised risk list".into()],
                }),
                config,
            )
            .unwrap();
            let content = fs::read_to_string(result.path).unwrap();
            assert!(content.contains("capture: none"));
            assert!(content.contains("sensitivity: restricted"));
            assert!(content.contains("consent: na"));
            assert!(!content.contains("debrief: pending"));
            assert!(content.contains("Opened with roadmap risk"));
            assert!(active_session().is_none());
        });
    }

    #[test]
    fn stop_without_debrief_marks_pending() {
        with_temp_home(|config| {
            start(Some("Quiet room")).unwrap();
            let result = stop(None, config).unwrap();
            let content = fs::read_to_string(result.path).unwrap();
            assert!(content.contains("debrief: pending"));
        });
    }

    #[test]
    fn sensitive_start_rejects_active_recording_pid() {
        with_temp_home(|_| {
            pid::create().unwrap();
            let error = start(Some("Board sync")).unwrap_err();
            assert!(matches!(error, SensitiveError::RecordingActive));
            pid::remove().ok();
        });
    }

    #[test]
    fn recording_gate_rejects_active_sensitive_session() {
        with_temp_home(|_| {
            start(Some("Board sync")).unwrap();
            let error = ensure_inactive_for_recording().unwrap_err();
            assert!(matches!(error, SensitiveError::AlreadyActive { .. }));
            #[cfg(feature = "voice-live")]
            assert!(matches!(
                crate::voice_live::refuse_if_microphone_busy(),
                Err(crate::voice_live::VoiceLiveError::SensitiveMeeting(
                    SensitiveError::AlreadyActive { .. }
                ))
            ));
        });
    }
}
