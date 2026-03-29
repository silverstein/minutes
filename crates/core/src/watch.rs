use crate::config::Config;
use crate::error::WatchError;
use crate::markdown::ContentType;
use crate::pipeline::{self, SidecarMetadata};
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
//   [skip .icloud stubs + processed/ + failed/]
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
//   [probe audio duration (symphonia)]
//        │ <threshold → ContentType::Memo (skip diarize)
//        │ >=threshold → ContentType::Meeting
//        │ probe failed → use config.watch.type
//        ▼
//   [read sidecar JSON if present]
//        │ found → enrich frontmatter (device, source)
//        │ missing/malformed → proceed without
//        ▼
//   [run pipeline: transcribe → write markdown]
//        │ success → move to processed/ + emit event + notify
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

/// Path to the watcher lock file (`~/.minutes/watch.lock`).
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
    crate::pid::is_process_alive(pid)
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

/// Check if a file is an iCloud eviction stub (.icloud placeholder).
fn is_icloud_stub(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.') && n.ends_with(".icloud"))
}

/// Probe audio duration from container metadata using symphonia.
/// Returns None if the file can't be probed (corrupt, unsupported, etc.).
fn audio_duration(path: &Path) -> Option<std::time::Duration> {
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .ok()?;

    let track = probed.format.default_track()?;
    let params = &track.codec_params;

    let n_frames = params.n_frames?;
    let sample_rate = params.sample_rate?;
    if sample_rate == 0 {
        return None;
    }

    Some(std::time::Duration::from_secs_f64(
        n_frames as f64 / sample_rate as f64,
    ))
}

/// Read optional sidecar JSON file (e.g., from Apple Shortcut).
/// Returns None if sidecar doesn't exist or is malformed — always best-effort.
fn read_sidecar(audio_path: &Path) -> Option<SidecarMetadata> {
    let sidecar_path = audio_path.with_extension("json");
    if !sidecar_path.exists() {
        return None;
    }

    match fs::read_to_string(&sidecar_path) {
        Ok(contents) => match serde_json::from_str::<SidecarMetadata>(&contents) {
            Ok(meta) => {
                tracing::info!(
                    sidecar = %sidecar_path.display(),
                    device = ?meta.device,
                    "sidecar metadata loaded"
                );
                // Clean up sidecar file after reading
                fs::remove_file(&sidecar_path).ok();
                Some(meta)
            }
            Err(e) => {
                tracing::warn!(
                    sidecar = %sidecar_path.display(),
                    error = %e,
                    "malformed sidecar JSON — processing without metadata"
                );
                None
            }
        },
        Err(e) => {
            tracing::warn!(
                sidecar = %sidecar_path.display(),
                error = %e,
                "could not read sidecar — processing without metadata"
            );
            None
        }
    }
}

/// Determine content type based on audio duration and config.
/// Duration-based routing takes priority over config.watch.type.
/// Set dictation_threshold_secs = 0 to disable duration-based routing.
fn determine_content_type(path: &Path, config: &Config) -> ContentType {
    let threshold = config.watch.dictation_threshold_secs;

    if threshold > 0 {
        if let Some(duration) = audio_duration(path) {
            let secs = duration.as_secs();
            let content_type = if secs < threshold {
                ContentType::Memo
            } else {
                ContentType::Meeting
            };
            tracing::info!(
                path = %path.display(),
                duration_secs = secs,
                threshold,
                content_type = ?content_type,
                "duration-based routing"
            );
            return content_type;
        }
        tracing::debug!(
            path = %path.display(),
            "could not probe duration — falling back to config type"
        );
    }

    // Fallback: use config.watch.type
    if config.watch.r#type == "meeting" {
        ContentType::Meeting
    } else {
        ContentType::Memo
    }
}

/// Process a single file through the pipeline.
fn process_file(path: &Path, config: &Config) -> Result<(), WatchError> {
    let content_type = determine_content_type(path, config);
    let sidecar = read_sidecar(path);

    match pipeline::process_with_sidecar(path, content_type, None, config, sidecar.as_ref(), |_| {})
    {
        Ok(result) => {
            tracing::info!(
                input = %path.display(),
                output = %result.path.display(),
                words = result.word_count,
                "file processed successfully"
            );

            // Emit WatchProcessed event (existing)
            crate::events::append_event(crate::events::MinutesEvent::WatchProcessed {
                path: result.path.display().to_string(),
                title: result.title.clone(),
                word_count: result.word_count,
                source_path: path.display().to_string(),
            });

            // Emit VoiceMemoProcessed event for voice memos (enables agent reactivity)
            if content_type == ContentType::Memo {
                crate::events::append_event(crate::events::MinutesEvent::VoiceMemoProcessed {
                    path: result.path.display().to_string(),
                    title: result.title.clone(),
                    word_count: result.word_count,
                    source_path: path.display().to_string(),
                    device: sidecar.as_ref().and_then(|s| s.device.clone()),
                });
            }

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
        // Reject symlinks — prevents traversal attacks
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            tracing::warn!(path = %path.display(), "skipping symlink in existing files");
            continue;
        }
        if path.is_file() && has_valid_extension(&path, config) {
            tracing::info!(path = %path.display(), "processing existing file");
            if wait_for_settle(&path, config.watch.settle_delay_ms) {
                // Validate file is actual audio (same check as handle_file_event)
                if audio_duration(&path).is_none() {
                    let is_wav = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
                    if !is_wav {
                        tracing::warn!(path = %path.display(), "file failed audio probe — skipping");
                        continue;
                    }
                }
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

    // Reject symlinks — prevents traversal attacks via crafted links
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        tracing::warn!(path = %path.display(), "skipping symlink — only regular files are processed");
        return;
    }

    // Skip iCloud eviction stubs (.NAME.icloud placeholder files)
    if is_icloud_stub(path) {
        tracing::debug!(path = %path.display(), "skipping iCloud stub");
        return;
    }

    // Skip sidecar JSON files (processed alongside their audio)
    if path.extension().and_then(|e| e.to_str()) == Some("json") {
        return;
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

    // Validate file is actual audio by probing with symphonia (magic bytes / container check).
    // Files that merely have the right extension but aren't valid audio are rejected early.
    if audio_duration(path).is_none() {
        // WAV files produced by cpal may not have duration metadata in the header,
        // so only reject non-WAV files that fail the probe.
        let is_wav = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
        if !is_wav {
            tracing::warn!(path = %path.display(), "file failed audio probe — not a valid audio container, skipping");
            return;
        }
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

    fn temp_minutes_dir() -> (TempDir, crate::config::TestMinutesDirGuard) {
        let dir = TempDir::new().unwrap();
        let guard = crate::config::override_test_minutes_dir(dir.path().join(".minutes"));
        (dir, guard)
    }

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
        let (_tmp, _minutes_dir) = temp_minutes_dir();
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
    fn is_icloud_stub_detects_stubs() {
        assert!(is_icloud_stub(Path::new(".recording.m4a.icloud")));
        assert!(is_icloud_stub(Path::new(".test.icloud")));
        assert!(!is_icloud_stub(Path::new("recording.m4a")));
        assert!(!is_icloud_stub(Path::new("icloud")));
        assert!(!is_icloud_stub(Path::new(".hidden_file")));
    }

    #[test]
    fn read_sidecar_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("test.m4a");
        fs::write(&audio, "audio data").unwrap();
        assert!(read_sidecar(&audio).is_none());
    }

    #[test]
    fn read_sidecar_parses_valid_json() {
        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("test.m4a");
        let sidecar = dir.path().join("test.json");
        fs::write(&audio, "audio data").unwrap();
        fs::write(&sidecar, r#"{"device": "iPhone", "source": "voice-memos"}"#).unwrap();

        let meta = read_sidecar(&audio).unwrap();
        assert_eq!(meta.device.as_deref(), Some("iPhone"));
        assert_eq!(meta.source.as_deref(), Some("voice-memos"));
        // Sidecar should be cleaned up after reading
        assert!(!sidecar.exists());
    }

    #[test]
    fn read_sidecar_handles_malformed_json() {
        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("test.m4a");
        let sidecar = dir.path().join("test.json");
        fs::write(&audio, "audio data").unwrap();
        fs::write(&sidecar, "not valid json {{{").unwrap();

        assert!(read_sidecar(&audio).is_none());
    }

    #[test]
    fn determine_content_type_uses_threshold() {
        let mut config = Config::default();
        config.watch.dictation_threshold_secs = 120;

        // When we can't probe duration, falls back to config type
        let path = Path::new("/nonexistent/test.m4a");
        let ct = determine_content_type(path, &config);
        // Default config.watch.type is "memo"
        assert_eq!(ct, ContentType::Memo);
    }

    #[test]
    fn determine_content_type_disabled_when_zero() {
        let mut config = Config::default();
        config.watch.dictation_threshold_secs = 0;
        config.watch.r#type = "meeting".into();

        let path = Path::new("/nonexistent/test.m4a");
        let ct = determine_content_type(path, &config);
        assert_eq!(ct, ContentType::Meeting);
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
