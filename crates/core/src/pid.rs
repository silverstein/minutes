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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_session_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProcessingStatus {
    pub processing: bool,
    pub stage: Option<String>,
    pub owner_pid: u32,
    pub mode: Option<CaptureMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(default)]
    pub job_count: usize,
}

pub fn write_recording_metadata(mode: CaptureMode) -> std::io::Result<()> {
    write_recording_metadata_with_context(mode, None)
}

pub fn write_recording_metadata_with_context(
    mode: CaptureMode,
    context_session_id: Option<&str>,
) -> std::io::Result<()> {
    let path = recording_meta_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let metadata = RecordingMetadata {
        mode,
        context_session_id: context_session_id.map(str::to_string),
    };
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
    title: Option<&str>,
    job_id: Option<&str>,
    job_count: usize,
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
        title: title.map(String::from),
        job_id: job_id.map(String::from),
        job_count,
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
            title: None,
            job_id: None,
            job_count: 0,
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
            title: None,
            job_id: None,
            job_count: 0,
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

/// Outcome of inspecting a PID file that may be held under an exclusive lock.
///
/// [`check_pid_file`] reads the PID bytes to decide liveness, which is defeated
/// on Windows by the very lock that proves liveness: `fs2` locks are *mandatory*
/// there (`LockFileEx` with `LOCKFILE_EXCLUSIVE_LOCK`), so a read from any other
/// handle of a file held by [`create_pid_guard`] fails with `ERROR_LOCK_VIOLATION`
/// and `check_pid_file` collapses a live session to `None`. A held mandatory lock,
/// however, *proves* the owner is alive — Windows releases file locks when the
/// owning process exits — so the lock violation is positive liveness evidence
/// even though the PID value itself is unreadable. On Unix `fs2` uses advisory
/// `flock`, so the read succeeds and this distinction never arises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidFileState {
    /// PID file present, readable, and the process is alive.
    Active(u32),
    /// PID file present but unreadable because the owner holds an exclusive lock
    /// (Windows mandatory lock). The owner is alive; the PID value is unknown.
    LockedAlive,
    /// PID file absent, stale (dead PID — cleaned up), or unreadable for any
    /// reason other than a lock/sharing violation.
    Inactive,
}

impl PidFileState {
    /// True when a live process holds the PID file, whether the PID was readable
    /// (`Active`) or the file was locked by its live owner (`LockedAlive`).
    pub fn is_active(self) -> bool {
        matches!(self, PidFileState::Active(_) | PidFileState::LockedAlive)
    }

    /// The owning PID when it could be read. `None` for `LockedAlive` (the lock
    /// proves liveness but hides the value) and `Inactive`.
    pub fn pid(self) -> Option<u32> {
        match self {
            PidFileState::Active(pid) => Some(pid),
            _ => None,
        }
    }
}

/// Inspect a PID file, distinguishing a live-but-locked owner (Windows mandatory
/// lock) from a genuinely absent or stale file. Use this instead of
/// [`check_pid_file`] for any reader of a PID file held under a persistent
/// [`create_pid_guard`] lock (e.g. the live-transcript or worker PID), so the
/// reader is not fooled by Windows file locking. Stale PID files (dead PID) are
/// cleaned up automatically, mirroring [`check_pid_file`].
pub fn inspect_pid_file(path: &Path) -> PidFileState {
    if !path.exists() {
        return PidFileState::Inactive;
    }

    match fs::read_to_string(path) {
        Ok(contents) => match contents.trim().parse::<u32>() {
            Ok(pid) if is_process_alive(pid) => PidFileState::Active(pid),
            Ok(pid) => {
                tracing::warn!(
                    "stale PID file found at {} (PID {pid} is dead). Cleaning up.",
                    path.display()
                );
                fs::remove_file(path).ok();
                PidFileState::Inactive
            }
            // Present but unparseable (empty/corrupt): treat as not active rather
            // than guessing a PID.
            Err(_) => PidFileState::Inactive,
        },
        // The read failed. On Windows a mandatory-lock violation means the owner
        // is alive and holding the lock; any other error is treated conservatively
        // as inactive (we never claim a live owner without positive evidence).
        Err(err) if is_lock_violation(&err) => PidFileState::LockedAlive,
        Err(_) => PidFileState::Inactive,
    }
}

/// Whether an I/O error is a Windows byte-range lock violation
/// (`ERROR_LOCK_VIOLATION` = 33) — the precise way a read of a region held under
/// `LockFileEx` surfaces. `create_pid_guard` opens the file with shared read
/// access, so our reader's open succeeds and only the `ReadFile` over the locked
/// range fails with 33; that is positive proof a live owner holds the lock.
///
/// We deliberately do NOT treat `ERROR_SHARING_VIOLATION` (32) as a live owner:
/// it signals an unrelated incompatible-share opener (antivirus, indexer, backup)
/// rather than the PID owner's lock, and since this gates start/stop/update
/// decisions, treating it as "alive" could falsely block recording or stall
/// `minutes stop`. A bare sharing violation degrades to the safe `Inactive`
/// default instead. On non-Windows targets `fs2` locks are advisory and reads
/// never fail this way, so this is always `false`.
fn is_lock_violation(err: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        // ERROR_LOCK_VIOLATION
        err.raw_os_error() == Some(33)
    }
    #[cfg(not(windows))]
    {
        let _ = err;
        false
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
            file.unlock().ok();
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
            file.unlock().ok();
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
            file.unlock().ok();
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
    pub processing_title: Option<String>,
    pub processing_job_id: Option<String>,
    pub processing_job_count: usize,
    pub pid: Option<u32>,
    pub duration_secs: Option<f64>,
    pub wav_path: Option<String>,
}

/// Get current recording status. Convenience wrapper that fetches its own
/// active-jobs snapshot. Hot-path callers that already have the snapshot
/// should call [`status_with_active_jobs`] instead — each `active_jobs()`
/// call walks `~/.minutes/jobs/` from disk, and a single status build
/// otherwise scans the dir up to three times.
pub fn status() -> RecordingStatus {
    let active_jobs = crate::jobs::active_jobs();
    status_with_active_jobs(&active_jobs)
}

/// Build a `RecordingStatus` from a pre-fetched `active_jobs` snapshot.
///
/// `status_value` in the Tauri commands layer polls at 1 Hz during a
/// recording. Each invocation needs to see the current set of in-flight
/// jobs once for its own JSON payload, and historically also passed
/// through `status()` which fetched the same set twice more (via
/// `processing_summary` and `active_job_count`). Threading the snapshot
/// through collapses three full directory walks into one — the user's
/// jobs/ directory grows without bound as meetings accumulate, so this
/// is meaningful for power users with hundreds of past meetings.
///
/// Ordering precondition: `active_jobs` must be in the order produced by
/// [`crate::jobs::active_jobs`] — active < queued < terminal, then
/// `created_at` descending. The first element is taken as the in-flight
/// summary; an unsorted slice will surface the wrong job.
pub fn status_with_active_jobs(active_jobs: &[crate::jobs::ProcessingJob]) -> RecordingStatus {
    let jobs_summary = active_jobs.first().cloned();
    let job_count = active_jobs.len();
    let processing = jobs_summary
        .as_ref()
        .map(|job| ProcessingStatus {
            processing: true,
            stage: job.stage.clone().or_else(|| job.state.default_stage()),
            owner_pid: job.owner_pid.unwrap_or(0),
            mode: Some(job.mode),
            title: job
                .title
                .clone()
                .or_else(|| job.output_path.as_ref().map(|path| path.to_string())),
            job_id: Some(job.id.clone()),
            job_count,
        })
        .unwrap_or_else(read_processing_status);
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
                processing: processing.processing,
                processing_stage: processing.stage,
                recording_mode: read_recording_metadata().map(|meta| meta.mode),
                processing_title: processing.title,
                processing_job_id: processing.job_id,
                processing_job_count: processing.job_count,
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
            processing_title: processing.title,
            processing_job_id: processing.job_id,
            processing_job_count: processing.job_count,
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
        let _guard = crate::test_home_env_lock();
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn is_process_alive_returns_false_for_dead_pid() {
        let _guard = crate::test_home_env_lock();
        // PID 99999999 almost certainly doesn't exist
        assert!(!is_process_alive(99_999_999));
    }

    #[test]
    fn processing_status_round_trip() {
        let _guard = crate::test_home_env_lock();
        set_processing_status(
            Some("Transcribing audio"),
            Some(CaptureMode::QuickThought),
            None,
            None,
            0,
        )
        .unwrap();
        let status = read_processing_status();
        assert!(status.processing);
        assert_eq!(status.stage.as_deref(), Some("Transcribing audio"));
        assert_eq!(status.owner_pid, std::process::id());
        assert_eq!(status.mode, Some(CaptureMode::QuickThought));
        assert_eq!(status.title, None);
        assert_eq!(status.job_id, None);
        assert_eq!(status.job_count, 0);
        clear_processing_status().unwrap();
    }

    #[test]
    fn recording_metadata_round_trip() {
        let _guard = crate::test_home_env_lock();
        write_recording_metadata(CaptureMode::QuickThought).unwrap();
        let metadata = read_recording_metadata().unwrap();
        assert_eq!(metadata.mode, CaptureMode::QuickThought);
        clear_recording_metadata().unwrap();
    }

    #[test]
    fn status_with_active_jobs_uses_slice_as_source_of_truth() {
        // Locks in the perf-fix contract: status_with_active_jobs reads job
        // count and summary fields off the passed-in slice instead of going
        // back to disk. Important — this is what removes 2 of the 3 directory
        // walks per UI status poll.
        let _guard = crate::test_home_env_lock();
        let job = crate::jobs::ProcessingJob {
            id: "job-status-check".into(),
            mode: CaptureMode::Meeting,
            content_type: crate::markdown::ContentType::Meeting,
            title: Some("Status check job".into()),
            audio_path: "/tmp/status.wav".into(),
            output_path: None,
            state: crate::jobs::JobState::Transcribing,
            stage: Some("Transcribing meeting".into()),
            created_at: chrono::Local::now(),
            started_at: Some(chrono::Local::now()),
            finished_at: None,
            notice_dismissed_at: None,
            recording_started_at: None,
            recording_finished_at: None,
            context_session_id: None,
            user_notes: None,
            pre_context: None,
            consent: None,
            consent_notice: None,
            calendar_event: None,
            template_slug: None,
            recording_health: None,
            word_count: None,
            error: None,
            owner_pid: Some(4242),
            retry_count: 0,
        };
        let jobs = vec![job];
        let status = status_with_active_jobs(&jobs);
        assert!(status.processing);
        assert_eq!(
            status.processing_job_id.as_deref(),
            Some("job-status-check")
        );
        assert_eq!(status.processing_job_count, 1);
        assert_eq!(status.processing_title.as_deref(), Some("Status check job"));
        assert_eq!(
            status.processing_stage.as_deref(),
            Some("Transcribing meeting")
        );

        let empty: Vec<crate::jobs::ProcessingJob> = Vec::new();
        let empty_status = status_with_active_jobs(&empty);
        assert_eq!(empty_status.processing_job_count, 0);
        assert_eq!(empty_status.processing_job_id, None);
    }

    #[test]
    fn status_with_active_jobs_takes_first_element_as_summary() {
        // Locks in that the function trusts caller-provided ordering.
        // active_jobs() returns active < queued < terminal then created_at
        // desc; the active in-flight job must surface as the summary.
        let _guard = crate::test_home_env_lock();
        let mk = |id: &str, state: crate::jobs::JobState, title: &str| crate::jobs::ProcessingJob {
            id: id.into(),
            mode: CaptureMode::Meeting,
            content_type: crate::markdown::ContentType::Meeting,
            title: Some(title.into()),
            audio_path: format!("/tmp/{id}.wav"),
            output_path: None,
            state,
            stage: state.default_stage(),
            created_at: chrono::Local::now(),
            started_at: None,
            finished_at: None,
            notice_dismissed_at: None,
            recording_started_at: None,
            recording_finished_at: None,
            context_session_id: None,
            user_notes: None,
            pre_context: None,
            consent: None,
            consent_notice: None,
            calendar_event: None,
            template_slug: None,
            recording_health: None,
            word_count: None,
            error: None,
            owner_pid: None,
            retry_count: 0,
        };
        let active = mk("job-a", crate::jobs::JobState::Transcribing, "Active job");
        let queued = mk("job-q", crate::jobs::JobState::Queued, "Queued job");
        let jobs = vec![active, queued];
        let status = status_with_active_jobs(&jobs);
        assert_eq!(status.processing_job_id.as_deref(), Some("job-a"));
        assert_eq!(status.processing_title.as_deref(), Some("Active job"));
        assert_eq!(status.processing_job_count, 2);
    }

    #[test]
    fn sentinel_lifecycle() {
        let _guard = crate::test_home_env_lock();
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
        let _guard = crate::test_home_env_lock();
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
        let _guard = crate::test_home_env_lock();
        // Ensure no sentinel exists
        let _ = std::fs::remove_file(stop_sentinel_path());
        assert!(!check_and_clear_sentinel());
    }

    #[test]
    fn create_pid_file_writes_using_locked_handle_without_reopen() {
        let _guard = crate::test_home_env_lock();
        let tempdir = tempfile::tempdir().unwrap();
        let pid_path = tempdir.path().join("recording.pid");

        create_pid_file(&pid_path).unwrap();

        let pid = check_pid_file(&pid_path).unwrap().unwrap();
        assert_eq!(pid, std::process::id());

        remove_pid_file(&pid_path).unwrap();
        assert!(!pid_path.exists());
    }

    #[test]
    fn inspect_pid_file_absent_is_inactive() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("live-transcript.pid");
        assert_eq!(inspect_pid_file(&path), PidFileState::Inactive);
        assert!(!inspect_pid_file(&path).is_active());
        assert_eq!(inspect_pid_file(&path).pid(), None);
    }

    #[test]
    fn inspect_pid_file_live_self_is_active() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("live-transcript.pid");
        std::fs::write(&path, std::process::id().to_string()).unwrap();

        let state = inspect_pid_file(&path);
        assert!(state.is_active());
        assert_eq!(state, PidFileState::Active(std::process::id()));
        assert_eq!(state.pid(), Some(std::process::id()));
    }

    #[test]
    fn inspect_pid_file_stale_dead_pid_is_inactive_and_cleaned() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("live-transcript.pid");
        // A high PID that almost certainly isn't a live process (matches the
        // value used by is_process_alive_returns_false_for_dead_pid). PID 0 is
        // unusable here: `kill(0, 0)` targets the caller's process group.
        std::fs::write(&path, "99999999").unwrap();

        assert_eq!(inspect_pid_file(&path), PidFileState::Inactive);
        assert!(!path.exists(), "stale PID file should be cleaned up");
    }

    #[test]
    fn inspect_pid_file_corrupt_is_inactive() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("live-transcript.pid");
        std::fs::write(&path, "not-a-pid").unwrap();
        assert_eq!(inspect_pid_file(&path), PidFileState::Inactive);
    }

    /// The fix for #258: a PID file held under a live `create_pid_guard` lock must
    /// report active. On Unix the read succeeds (`Active`); on Windows the read
    /// hits the mandatory lock (`LockedAlive`). Either way `is_active()` is true —
    /// that is the platform-independent invariant the live-transcript readers rely
    /// on.
    #[test]
    fn inspect_pid_file_reports_active_while_guard_is_held() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("live-transcript.pid");

        let guard = create_pid_guard(&path).unwrap();
        let state = inspect_pid_file(&path);
        assert!(
            state.is_active(),
            "a held guard must read as active, got {state:?}"
        );
        #[cfg(windows)]
        assert_eq!(state, PidFileState::LockedAlive);
        #[cfg(not(windows))]
        assert_eq!(state, PidFileState::Active(std::process::id()));

        drop(guard);
        assert_eq!(inspect_pid_file(&path), PidFileState::Inactive);
    }
}
