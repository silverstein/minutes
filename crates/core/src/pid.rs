use crate::config::Config;
use crate::error::PidError;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

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

/// Path to the recording PID file (`~/.minutes/recording.pid`).
pub fn pid_path() -> PathBuf {
    Config::minutes_dir().join("recording.pid")
}

/// Path to the dictation PID file (`~/.minutes/dictation.pid`).
pub fn dictation_pid_path() -> PathBuf {
    Config::minutes_dir().join("dictation.pid")
}

/// Path to the live transcript PID file (`~/.minutes/live-transcript.pid`).
pub fn live_transcript_pid_path() -> PathBuf {
    Config::minutes_dir().join("live-transcript.pid")
}

/// Path to the live transcript JSONL file (`~/.minutes/live-transcript.jsonl`).
pub fn live_transcript_jsonl_path() -> PathBuf {
    Config::minutes_dir().join("live-transcript.jsonl")
}

/// Path to the live transcript WAV file (`~/.minutes/live-transcript.wav`).
pub fn live_transcript_wav_path() -> PathBuf {
    Config::minutes_dir().join("live-transcript.wav")
}

/// Path to the live transcript status sidecar (`~/.minutes/live-transcript-status.json`).
pub fn live_transcript_status_path() -> PathBuf {
    Config::minutes_dir().join("live-transcript-status.json")
}

/// Path to the recording metadata JSON (`~/.minutes/recording-meta.json`).
pub fn recording_meta_path() -> PathBuf {
    Config::minutes_dir().join("recording-meta.json")
}

/// Path to the in-progress audio capture file (`~/.minutes/current.wav`).
pub fn current_wav_path() -> PathBuf {
    Config::minutes_dir().join("current.wav")
}

/// Path to the last recording result JSON (`~/.minutes/last-result.json`).
pub fn last_result_path() -> PathBuf {
    Config::minutes_dir().join("last-result.json")
}

/// Path to the processing status JSON (`~/.minutes/processing-status.json`).
pub fn processing_status_path() -> PathBuf {
    Config::minutes_dir().join("processing-status.json")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureMode {
    Meeting,
    QuickThought,
    Dictation,
    LiveTranscript,
}

impl CaptureMode {
    pub fn content_type(self) -> crate::markdown::ContentType {
        match self {
            Self::Meeting | Self::LiveTranscript => crate::markdown::ContentType::Meeting,
            Self::QuickThought => crate::markdown::ContentType::Memo,
            Self::Dictation => crate::markdown::ContentType::Dictation,
        }
    }

    pub fn noun(self) -> &'static str {
        match self {
            Self::Meeting => "meeting",
            Self::QuickThought => "quick thought",
            Self::Dictation => "dictation",
            Self::LiveTranscript => "live transcript",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecordingMetadata {
    pub mode: CaptureMode,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProcessingStatus {
    pub processing: bool,
    pub stage: Option<String>,
    pub owner_pid: u32,
    pub mode: Option<CaptureMode>,
}

pub fn write_recording_metadata(mode: CaptureMode) -> std::io::Result<()> {
    let path = recording_meta_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let metadata = RecordingMetadata { mode };
    let json = serde_json::to_string(&metadata)?;
    fs::write(path, json)
}

pub fn read_recording_metadata() -> Option<RecordingMetadata> {
    let path = recording_meta_path();
    if !path.exists() {
        return None;
    }

    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<RecordingMetadata>(&s).ok())
}

pub fn clear_recording_metadata() -> std::io::Result<()> {
    let path = recording_meta_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn set_processing_status(
    stage: Option<&str>,
    mode: Option<CaptureMode>,
) -> std::io::Result<()> {
    let path = processing_status_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let status = ProcessingStatus {
        processing: true,
        stage: stage.map(String::from),
        owner_pid: std::process::id(),
        mode,
    };
    let json = serde_json::to_string(&status)?;
    fs::write(path, json)
}

pub fn clear_processing_status() -> std::io::Result<()> {
    let path = processing_status_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn read_processing_status() -> ProcessingStatus {
    let path = processing_status_path();
    if !path.exists() {
        return ProcessingStatus {
            processing: false,
            stage: None,
            owner_pid: 0,
            mode: None,
        };
    }

    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<ProcessingStatus>(&s).ok())
        .and_then(|status| {
            if status.owner_pid != 0 && is_process_alive(status.owner_pid) {
                Some(status)
            } else {
                clear_processing_status().ok();
                None
            }
        })
        .unwrap_or(ProcessingStatus {
            processing: false,
            stage: None,
            owner_pid: 0,
            mode: None,
        })
}

/// Check if a process holds the given PID file.
/// Returns Ok(Some(pid)) if active, Ok(None) if not.
/// Cleans up stale PID files automatically.
pub fn check_pid_file(path: &Path) -> Result<Option<u32>, PidError> {
    if !path.exists() {
        return Ok(None);
    }

    let pid_str = fs::read_to_string(path)?;
    let pid: u32 = pid_str.trim().parse().map_err(|_| PidError::StalePid(0))?;

    if is_process_alive(pid) {
        Ok(Some(pid))
    } else {
        tracing::warn!(
            "stale PID file found at {} (PID {pid} is dead). Cleaning up.",
            path.display()
        );
        fs::remove_file(path).ok();
        Ok(None)
    }
}

fn read_locked_pid(file: &mut fs::File) -> Result<Option<u32>, PidError> {
    file.seek(SeekFrom::Start(0))?;

    let mut pid_str = String::new();
    file.read_to_string(&mut pid_str)?;
    let trimmed = pid_str.trim();

    if trimmed.is_empty() {
        return Ok(None);
    }

    let pid = trimmed.parse().map_err(|_| PidError::StalePid(0))?;
    Ok(Some(pid))
}

fn write_locked_pid(file: &mut fs::File, pid: u32) -> Result<(), PidError> {
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    write!(file, "{}", pid)?;
    file.flush()?;
    Ok(())
}

/// Create a PID file at the given path with exclusive flock.
pub fn create_pid_file(path: &Path) -> Result<(), PidError> {
    use fs2::FileExt;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;

    if file.try_lock_exclusive().is_err() {
        let existing_pid = fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);
        return Err(PidError::AlreadyRecording(existing_pid));
    }

    if let Some(old_pid) = read_locked_pid(&mut file)? {
        if old_pid != 0 && is_process_alive(old_pid) {
            fs2::FileExt::unlock(&file).ok();
            return Err(PidError::AlreadyRecording(old_pid));
        }
    }

    let pid = std::process::id();
    write_locked_pid(&mut file, pid)?;

    tracing::debug!("PID file created: {} (PID {})", path.display(), pid);
    Ok(())
}

/// A guard that holds an exclusive flock on a PID file for the lifetime of a session.
/// The PID file is removed and the lock released when the guard is dropped.
pub struct PidGuard {
    file: Option<fs::File>,
    path: PathBuf,
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        // On Unix: unlink first (flock persists on the unlinked inode until fd is closed).
        // This prevents the race where another process acquires the lock between
        // our fd close and our unlink.
        // On Windows: must close the fd before deleting (can't delete an open file).
        #[cfg(unix)]
        {
            fs::remove_file(&self.path).ok();
            self.file.take(); // releases flock on the now-unlinked inode
        }
        #[cfg(not(unix))]
        {
            self.file.take(); // release handle so Windows can delete
            fs::remove_file(&self.path).ok();
        }
        tracing::debug!("PID guard dropped: {}", self.path.display());
    }
}

/// Create a PID file with an exclusive flock held for the lifetime of the returned guard.
/// The flock is NOT released until the guard is dropped, preventing concurrent starts.
pub fn create_pid_guard(path: &Path) -> Result<PidGuard, PidError> {
    use fs2::FileExt;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;

    if file.try_lock_exclusive().is_err() {
        let existing_pid = fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);
        return Err(PidError::AlreadyRecording(existing_pid));
    }

    if let Some(old_pid) = read_locked_pid(&mut file)? {
        if old_pid != 0 && is_process_alive(old_pid) {
            fs2::FileExt::unlock(&file).ok();
            return Err(PidError::AlreadyRecording(old_pid));
        }
    }

    let pid = std::process::id();
    write_locked_pid(&mut file, pid)?;

    tracing::debug!("PID guard created: {} (PID {})", path.display(), pid);
    Ok(PidGuard {
        file: Some(file),
        path: path.to_path_buf(),
    })
}

/// Remove a PID file at the given path.
pub fn remove_pid_file(path: &Path) -> Result<(), PidError> {
    if path.exists() {
        fs::remove_file(path)?;
        tracing::debug!("PID file removed: {}", path.display());
    }
    Ok(())
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

/// Create PID file for current process with exclusive file lock.
/// Uses flock to make the check-and-write atomic, preventing TOCTOU races
/// when two `minutes record` invocations start simultaneously.
pub fn create() -> Result<(), PidError> {
    use fs2::FileExt;

    // Clean up stale sentinel from a previous crashed recording
    check_and_clear_sentinel();

    let path = pid_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Open/create the PID file and acquire an exclusive lock.
    // This is atomic: if another process holds the lock, we block briefly then check.
    let mut file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)?;

    // Try non-blocking lock — if we can't get it, another recorder is running
    if file.try_lock_exclusive().is_err() {
        // Read the existing PID to report which process holds it
        let existing_pid = fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);
        return Err(PidError::AlreadyRecording(existing_pid));
    }

    // We hold the lock. Check if there's a stale PID from a crashed process.
    if let Some(old_pid) = read_locked_pid(&mut file)? {
        if old_pid != 0 && is_process_alive(old_pid) {
            fs2::FileExt::unlock(&file).ok();
            return Err(PidError::AlreadyRecording(old_pid));
        }
    }

    // Write our PID (we still hold the lock)
    let pid = std::process::id();
    write_locked_pid(&mut file, pid)?;

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
    clear_recording_metadata().ok();
    // Don't delete current.wav — it may contain recoverable audio
    Ok(())
}

/// Check if a process with the given PID is alive.
pub fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) checks if the process exists without sending a signal
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
        unsafe {
            let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if handle.is_null() {
                false
            } else {
                CloseHandle(handle);
                true
            }
        }
    }
}

/// Path to the sentinel file used for cross-platform stop signaling.
/// `minutes stop` writes this file; the recording process polls for it.
pub fn stop_sentinel_path() -> PathBuf {
    Config::minutes_dir().join("recording.stop")
}

/// Write the sentinel file to signal the recording process to stop.
pub fn write_stop_sentinel() -> std::io::Result<()> {
    let path = stop_sentinel_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, "stop")
}

/// Check if the stop sentinel exists and remove it.
/// Returns true if it was present (stop was requested).
pub fn check_and_clear_sentinel() -> bool {
    let path = stop_sentinel_path();
    if path.exists() {
        fs::remove_file(&path).ok();
        true
    } else {
        false
    }
}

/// Spawn a background thread that polls for the sentinel file and sets the stop flag.
/// Returns a JoinHandle that can be used to wait for cleanup.
pub fn spawn_sentinel_watcher(
    stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        loop {
            if stop_flag.load(std::sync::atomic::Ordering::Relaxed) {
                // Already stopping (e.g., via SIGTERM on Unix) — clean up sentinel if present
                check_and_clear_sentinel();
                break;
            }
            if check_and_clear_sentinel() {
                tracing::info!("stop sentinel detected — stopping recording");
                stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    })
}

/// Recording status, returned by `minutes status`.
#[derive(Debug, serde::Serialize)]
pub struct RecordingStatus {
    pub recording: bool,
    pub processing: bool,
    pub processing_stage: Option<String>,
    pub recording_mode: Option<CaptureMode>,
    pub pid: Option<u32>,
    pub duration_secs: Option<f64>,
    pub wav_path: Option<String>,
}

/// Get current recording status.
pub fn status() -> RecordingStatus {
    let processing = read_processing_status();
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
                processing: false,
                processing_stage: None,
                recording_mode: read_recording_metadata().map(|meta| meta.mode),
                pid: Some(pid),
                // Duration is approximate: time since WAV was last modified.
                // The record process writes continuously, so this is close.
                duration_secs: duration,
                wav_path: Some(wav.display().to_string()),
            }
        }
        _ => RecordingStatus {
            recording: false,
            processing: processing.processing,
            processing_stage: processing.stage,
            recording_mode: processing.mode,
            pid: None,
            duration_secs: None,
            wav_path: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs2::FileExt;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use tempfile::TempDir;

    fn test_guard() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn temp_minutes_dir() -> (TempDir, crate::config::TestMinutesDirGuard) {
        let dir = TempDir::new().unwrap();
        let guard = crate::config::override_test_minutes_dir(dir.path().join(".minutes"));
        (dir, guard)
    }

    #[test]
    fn is_process_alive_detects_current_process() {
        let _guard = test_guard();
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn is_process_alive_returns_false_for_dead_pid() {
        let _guard = test_guard();
        // PID 99999999 almost certainly doesn't exist
        assert!(!is_process_alive(99_999_999));
    }

    #[test]
    fn processing_status_round_trip() {
        let _guard = test_guard();
        let (_tmp, _minutes_dir) = temp_minutes_dir();
        set_processing_status(Some("Transcribing audio"), Some(CaptureMode::QuickThought)).unwrap();
        let status = read_processing_status();
        assert!(status.processing);
        assert_eq!(status.stage.as_deref(), Some("Transcribing audio"));
        assert_eq!(status.owner_pid, std::process::id());
        assert_eq!(status.mode, Some(CaptureMode::QuickThought));
        clear_processing_status().unwrap();
    }

    #[test]
    fn recording_metadata_round_trip() {
        let _guard = test_guard();
        let (_tmp, _minutes_dir) = temp_minutes_dir();
        write_recording_metadata(CaptureMode::QuickThought).unwrap();
        let metadata = read_recording_metadata().unwrap();
        assert_eq!(metadata.mode, CaptureMode::QuickThought);
        clear_recording_metadata().unwrap();
    }

    #[test]
    fn sentinel_lifecycle() {
        let _guard = test_guard();
        let (_tmp, _minutes_dir) = temp_minutes_dir();
        // Ensure clean state
        let _ = std::fs::remove_file(stop_sentinel_path());
        assert!(!stop_sentinel_path().exists());

        // Write sentinel
        write_stop_sentinel().unwrap();
        assert!(stop_sentinel_path().exists());

        // Check and clear returns true, removes file
        assert!(check_and_clear_sentinel());
        assert!(!stop_sentinel_path().exists());

        // Second check returns false
        assert!(!check_and_clear_sentinel());
    }

    #[test]
    fn sentinel_write_and_clear() {
        let _guard = test_guard();
        let (_tmp, _minutes_dir) = temp_minutes_dir();
        // Write a sentinel and verify check_and_clear removes it
        write_stop_sentinel().unwrap();
        assert!(stop_sentinel_path().exists());
        assert!(check_and_clear_sentinel());
        assert!(!stop_sentinel_path().exists());
        // Second call returns false — already cleared
        assert!(!check_and_clear_sentinel());
    }

    #[test]
    fn check_and_clear_sentinel_returns_false_when_absent() {
        let _guard = test_guard();
        let (_tmp, _minutes_dir) = temp_minutes_dir();
        // Ensure no sentinel exists
        let _ = std::fs::remove_file(stop_sentinel_path());
        assert!(!check_and_clear_sentinel());
    }

    #[test]
    fn create_pid_file_writes_using_locked_handle_without_reopen() {
        let _guard = test_guard();
        let tempdir = tempfile::tempdir().unwrap();
        let pid_path = tempdir.path().join("recording.pid");

        create_pid_file(&pid_path).unwrap();

        let pid = check_pid_file(&pid_path).unwrap().unwrap();
        assert_eq!(pid, std::process::id());

        remove_pid_file(&pid_path).unwrap();
        assert!(!pid_path.exists());
    }
}
