use crate::config::Config;
use crate::error::PidError;
use std::fs;
use std::path::PathBuf;

// ──────────────────────────────────────────────────────────────
// PID file state machine:
//
//   [none] ──create──▶ [recording] ──remove──▶ [none]
//                           │
//                     (process dies)
//                           │
//                           ▼
//                      [stale] ──cleanup──▶ [none]
//
// Files:
//   ~/.minutes/recording.pid   — contains PID as text
//   ~/.minutes/current.wav     — audio being captured
//   ~/.minutes/last-result.json — written by record on shutdown
// ──────────────────────────────────────────────────────────────

pub fn pid_path() -> PathBuf {
    Config::minutes_dir().join("recording.pid")
}

pub fn current_wav_path() -> PathBuf {
    Config::minutes_dir().join("current.wav")
}

pub fn last_result_path() -> PathBuf {
    Config::minutes_dir().join("last-result.json")
}

/// Check if a recording is currently in progress.
/// Returns Ok(Some(pid)) if recording, Ok(None) if not.
/// Cleans up stale PID files automatically.
pub fn check_recording() -> Result<Option<u32>, PidError> {
    let path = pid_path();
    if !path.exists() {
        return Ok(None);
    }

    let pid_str = fs::read_to_string(&path)?;
    let pid: u32 = pid_str.trim().parse().map_err(|_| PidError::StalePid(0))?;

    if is_process_alive(pid) {
        Ok(Some(pid))
    } else {
        // Stale PID — process is dead. Clean up.
        tracing::warn!("stale PID file found (PID {pid} is dead). Cleaning up.");
        cleanup_stale()?;
        Ok(None)
    }
}

/// Create PID file for current process. Errors if already recording.
pub fn create() -> Result<(), PidError> {
    if let Some(pid) = check_recording()? {
        return Err(PidError::AlreadyRecording(pid));
    }

    let path = pid_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let pid = std::process::id();
    fs::write(&path, pid.to_string())?;
    tracing::debug!("PID file created: {} (PID {})", path.display(), pid);
    Ok(())
}

/// Remove PID file. Called on graceful shutdown.
pub fn remove() -> Result<(), PidError> {
    let path = pid_path();
    if path.exists() {
        fs::remove_file(&path)?;
        tracing::debug!("PID file removed: {}", path.display());
    }
    Ok(())
}

/// Clean up stale recording artifacts.
fn cleanup_stale() -> Result<(), PidError> {
    let path = pid_path();
    if path.exists() {
        fs::remove_file(&path)?;
    }
    // Don't delete current.wav — it may contain recoverable audio
    Ok(())
}

/// Check if a process with the given PID is alive.
fn is_process_alive(pid: u32) -> bool {
    // kill(pid, 0) checks if the process exists without sending a signal
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Recording status, returned by `minutes status`.
#[derive(Debug, serde::Serialize)]
pub struct RecordingStatus {
    pub recording: bool,
    pub pid: Option<u32>,
    pub duration_secs: Option<f64>,
    pub wav_path: Option<String>,
}

/// Get current recording status.
pub fn status() -> RecordingStatus {
    match check_recording() {
        Ok(Some(pid)) => {
            let wav = current_wav_path();
            let duration = wav
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|modified| {
                    std::time::SystemTime::now()
                        .duration_since(modified)
                        .ok()
                        .map(|d| d.as_secs_f64())
                });

            RecordingStatus {
                recording: true,
                pid: Some(pid),
                // Duration is approximate: time since WAV was last modified.
                // The record process writes continuously, so this is close.
                duration_secs: duration,
                wav_path: Some(wav.display().to_string()),
            }
        }
        _ => RecordingStatus {
            recording: false,
            pid: None,
            duration_secs: None,
            wav_path: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn is_process_alive_detects_current_process() {
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn is_process_alive_returns_false_for_dead_pid() {
        // PID 99999999 almost certainly doesn't exist
        assert!(!is_process_alive(99_999_999));
    }
}
