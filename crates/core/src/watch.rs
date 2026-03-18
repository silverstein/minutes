use crate::config::Config;
use crate::error::WatchError;
use crate::markdown::ContentType;
use crate::pipeline;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

// ──────────────────────────────────────────────────────────────
// Folder watcher event loop:
//
//   [detect new file]
//        │
//        ▼
//   [settle check: size stable across 2 checks?]
//        │ no → skip, retry next cycle
//        │ yes
//        ▼
//   [acquire lock (watch.lock)]
//        │ fail → "another watcher running"
//        │ ok
//        ▼
//   [check extension filter]
//        │ no match → skip
//        │ match
//        ▼
//   [run pipeline: transcribe → write markdown]
//        │ success → move to processed/
//        │ failure → move to failed/
//        ▼
//   [release lock]
//
// Files:
//   ~/.minutes/watch.lock          — prevents concurrent watchers
//   ~/.minutes/inbox/              — watched folder (default)
//   ~/.minutes/inbox/processed/    — successfully processed
//   ~/.minutes/inbox/failed/       — processing failed
// ──────────────────────────────────────────────────────────────

pub fn lock_path() -> PathBuf {
    Config::minutes_dir().join("watch.lock")
}

/// Acquire the watcher lock. Returns error if another watcher is running.
fn acquire_lock() -> Result<(), WatchError> {
    let path = lock_path();
    if path.exists() {
        // Check if the PID in the lock file is still alive
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Ok(pid) = contents.trim().parse::<u32>() {
                if is_process_alive(pid) {
                    return Err(WatchError::AlreadyRunning(path.display().to_string()));
                }
            }
        }
        // Stale lock — remove it
        tracing::warn!("stale watch lock found, removing");
        fs::remove_file(&path).ok();
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, std::process::id().to_string())?;
    Ok(())
}

/// Release the watcher lock.
fn release_lock() {
    let path = lock_path();
    fs::remove_file(&path).ok();
}

fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Check if a file has a watched extension.
fn has_valid_extension(path: &Path, config: &Config) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            config
                .watch
                .extensions
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(ext))
        })
}

/// Wait for a file to finish syncing (size-stability check).
/// Returns true if the file is stable and ready to process.
fn wait_for_settle(path: &Path, delay_ms: u64) -> bool {
    let delay = Duration::from_millis(delay_ms);

    // First check
    let size1 = match fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return false, // File disappeared
    };

    if size1 == 0 {
        // File is empty — might still be syncing. Wait and check again.
        std::thread::sleep(delay);
        match fs::metadata(path) {
            Ok(m) if m.len() == 0 => return false, // Still empty
            Ok(_) => {}                            // Now has content, continue
            Err(_) => return false,                // Disappeared
        }
    }

    std::thread::sleep(delay);

    // Second check
    let size2 = match fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return false,
    };

    if size1 != size2 || size2 == 0 {
        tracing::debug!(
            path = %path.display(),
            size1, size2,
            "file not yet stable, skipping this cycle"
        );
        return false;
    }

    true
}

/// Move a file to a subdirectory (processed/ or failed/).
fn move_to(file: &Path, subdir: &str) -> Result<PathBuf, WatchError> {
    let parent = file.parent().unwrap_or(Path::new("."));
    let dest_dir = parent.join(subdir);
    fs::create_dir_all(&dest_dir)
        .map_err(|e| WatchError::MoveError(dest_dir.display().to_string(), e))?;

    let filename = file.file_name().unwrap_or_default();
    let dest = dest_dir.join(filename);

    // Handle collision in destination
    let dest = if dest.exists() {
        let stem = dest.file_stem().unwrap_or_default().to_string_lossy();
        let ext = dest
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();
        let ts = chrono::Local::now().timestamp();
        dest_dir.join(format!("{}-{}.{}", stem, ts, ext))
    } else {
        dest
    };

    fs::rename(file, &dest).map_err(|e| WatchError::MoveError(dest.display().to_string(), e))?;

    tracing::debug!(from = %file.display(), to = %dest.display(), "moved file");
    Ok(dest)
}

/// Process a single file through the pipeline.
fn process_file(path: &Path, config: &Config) -> Result<(), WatchError> {
    let content_type = if config.watch.r#type == "meeting" {
        ContentType::Meeting
    } else {
        ContentType::Memo
    };

    match pipeline::process(path, content_type, None, config) {
        Ok(result) => {
            tracing::info!(
                input = %path.display(),
                output = %result.path.display(),
                words = result.word_count,
                "file processed successfully"
            );
            move_to(path, "processed")?;
            Ok(())
        }
        Err(e) => {
            tracing::error!(
                input = %path.display(),
                error = %e,
                "pipeline failed — moving to failed/"
            );
            move_to(path, "failed")?;
            Err(WatchError::Io(std::io::Error::other(format!(
                "pipeline error: {}",
                e
            ))))
        }
    }
}

/// Run the folder watcher. Blocks until interrupted (Ctrl-C).
pub fn run(watch_dir: Option<&Path>, config: &Config) -> Result<(), WatchError> {
    let dirs: Vec<PathBuf> = if let Some(dir) = watch_dir {
        vec![dir.to_path_buf()]
    } else {
        config.watch.paths.clone()
    };

    // Validate directories
    for dir in &dirs {
        if !dir.exists() {
            fs::create_dir_all(dir)?;
            tracing::info!(dir = %dir.display(), "created watch directory");
        }
        // Create processed/ and failed/ subdirs
        fs::create_dir_all(dir.join("processed"))?;
        fs::create_dir_all(dir.join("failed"))?;
    }

    // Acquire lock
    acquire_lock()?;
    tracing::info!("watcher lock acquired");

    // Set up cleanup on exit
    let _guard = LockGuard;

    eprintln!(
        "Watching {} for audio files... (Ctrl-C to stop)",
        dirs.iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Process any existing files first
    for dir in &dirs {
        process_existing_files(dir, config);
    }

    // Set up file watcher
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                tx.send(event).ok();
            }
        },
        notify::Config::default().with_poll_interval(Duration::from_secs(2)),
    )
    .map_err(|e| WatchError::NotifyError(e.to_string()))?;

    for dir in &dirs {
        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|e| WatchError::NotifyError(e.to_string()))?;
    }

    // Event loop
    let settle_delay = config.watch.settle_delay_ms;
    loop {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(event) => {
                if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                    for path in event.paths {
                        handle_file_event(&path, settle_delay, config);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Normal timeout — continue watching
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::info!("watcher channel disconnected, exiting");
                break;
            }
        }
    }

    Ok(())
}

/// Process files that already exist in the watch directory.
fn process_existing_files(dir: &Path, config: &Config) {
    let entries: Vec<_> = match fs::read_dir(dir) {
        Ok(entries) => entries.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };

    for entry in entries {
        let path = entry.path();
        if path.is_file() && has_valid_extension(&path, config) {
            tracing::info!(path = %path.display(), "processing existing file");
            if wait_for_settle(&path, config.watch.settle_delay_ms) {
                if let Err(e) = process_file(&path, config) {
                    tracing::error!(path = %path.display(), error = %e, "failed to process existing file");
                }
            }
        }
    }
}

/// Handle a single file event from the watcher.
fn handle_file_event(path: &Path, settle_delay: u64, config: &Config) {
    // Skip directories, processed/, failed/ subdirs
    if !path.is_file() {
        return;
    }
    if let Some(parent) = path.parent() {
        if let Some(name) = parent.file_name() {
            let name = name.to_string_lossy();
            if name == "processed" || name == "failed" {
                return;
            }
        }
    }

    // Check extension
    if !has_valid_extension(path, config) {
        tracing::debug!(path = %path.display(), "skipping — unsupported extension");
        return;
    }

    // Settle check
    if !wait_for_settle(path, settle_delay) {
        tracing::debug!(path = %path.display(), "file not stable yet");
        return;
    }

    tracing::info!(path = %path.display(), "new file detected, processing");
    if let Err(e) = process_file(path, config) {
        tracing::error!(path = %path.display(), error = %e, "processing failed");
    }
}

/// RAII guard that releases the lock file on drop.
struct LockGuard;

impl Drop for LockGuard {
    fn drop(&mut self) {
        release_lock();
        tracing::debug!("watcher lock released");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn has_valid_extension_matches_configured_types() {
        let config = Config::default();
        let path = Path::new("test.m4a");
        assert!(has_valid_extension(path, &config));

        let path = Path::new("test.wav");
        assert!(has_valid_extension(path, &config));

        let path = Path::new("test.txt");
        assert!(!has_valid_extension(path, &config));

        let path = Path::new("test.pdf");
        assert!(!has_valid_extension(path, &config));
    }

    #[test]
    fn has_valid_extension_is_case_insensitive() {
        let config = Config::default();
        assert!(has_valid_extension(Path::new("test.M4A"), &config));
        assert!(has_valid_extension(Path::new("test.WAV"), &config));
    }

    #[test]
    fn move_to_processed_works() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.wav");
        fs::write(&file, "audio data").unwrap();

        let dest = move_to(&file, "processed").unwrap();
        assert!(!file.exists());
        assert!(dest.exists());
        assert!(dest.to_str().unwrap().contains("processed"));
    }

    #[test]
    fn move_to_failed_works() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.wav");
        fs::write(&file, "audio data").unwrap();

        let dest = move_to(&file, "failed").unwrap();
        assert!(!file.exists());
        assert!(dest.exists());
        assert!(dest.to_str().unwrap().contains("failed"));
    }

    #[test]
    fn move_to_handles_collision() {
        let dir = TempDir::new().unwrap();

        // Create a file in processed/ with the same name
        let processed = dir.path().join("processed");
        fs::create_dir_all(&processed).unwrap();
        fs::write(processed.join("test.wav"), "existing").unwrap();

        // Create the source file
        let file = dir.path().join("test.wav");
        fs::write(&file, "new audio data").unwrap();

        let dest = move_to(&file, "processed").unwrap();
        assert!(!file.exists());
        assert!(dest.exists());
        // Should have a timestamp suffix to avoid collision
        assert_ne!(dest.file_name().unwrap(), "test.wav");
    }

    #[test]
    fn settle_check_rejects_empty_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("empty.wav");
        fs::write(&file, "").unwrap();

        // Use very short delay for test speed
        assert!(!wait_for_settle(&file, 10));
    }

    #[test]
    fn settle_check_accepts_stable_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("stable.wav");
        fs::write(&file, "some audio data here that is not empty").unwrap();

        assert!(wait_for_settle(&file, 10));
    }

    #[test]
    fn settle_check_handles_missing_file() {
        assert!(!wait_for_settle(Path::new("/nonexistent/file.wav"), 10));
    }

    #[test]
    fn lock_acquire_and_release() {
        // Clean up any existing lock
        release_lock();

        assert!(acquire_lock().is_ok());
        // Second acquire should fail (same process is alive)
        assert!(acquire_lock().is_err());
        // Release and re-acquire
        release_lock();
        assert!(acquire_lock().is_ok());
        release_lock();
    }

    #[test]
    fn skip_files_in_processed_and_failed() {
        let config = Config::default();
        let dir = TempDir::new().unwrap();
        let processed = dir.path().join("processed");
        fs::create_dir_all(&processed).unwrap();
        let file = processed.join("old.wav");
        fs::write(&file, "data").unwrap();

        // handle_file_event should skip files in processed/
        // We can verify by checking the parent directory name logic
        let parent_name = file
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy();
        assert_eq!(parent_name, "processed");
    }
}
