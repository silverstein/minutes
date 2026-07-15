use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local};

// ──────────────────────────────────────────────────────────────
// Screen context capture.
//
// Periodically captures screenshots during a recording session
// to give the LLM visual context about what was on screen.
//
// Privacy model:
//   - Disabled by default (opt-in via config)
//   - Screenshots stored with 0600 permissions
//   - Cleaned up after summarization (configurable)
//   - Never sent anywhere without explicit LLM config
//
// macOS: uses `screencapture -x` (silent, no shutter sound)
// Linux: uses `scrot` or `gnome-screenshot` if available
// ──────────────────────────────────────────────────────────────

/// Check if screen recording permission is available on macOS.
/// Returns true if we can capture, false if permission is missing.
/// On non-macOS platforms, always returns true.
pub fn check_screen_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        // Capture a 1x1 test screenshot to check permission
        let test_path = std::env::temp_dir().join("minutes-screen-test.png");
        let result = std::process::Command::new("screencapture")
            .args(["-x", "-R", "0,0,1,1", "-t", "png"])
            .arg(&test_path)
            .output();

        let _ = std::fs::remove_file(&test_path);

        match result {
            Ok(output) => {
                if output.status.success() {
                    // Check if the file was created and is non-trivial
                    // (blank/black screenshots from missing permission are still valid PNGs
                    // but we can't easily distinguish them without image analysis)
                    true
                } else {
                    tracing::warn!("screen capture permission check failed — grant Screen Recording permission in System Settings > Privacy & Security");
                    false
                }
            }
            Err(_) => {
                tracing::warn!("screencapture command not found");
                false
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Start capturing screenshots at a regular interval.
/// Returns a handle that stops capture when dropped.
/// Screenshots are saved as timestamped PNGs in `output_dir`.
pub fn start_capture(
    output_dir: &Path,
    interval: Duration,
    stop_flag: Arc<AtomicBool>,
) -> std::io::Result<ScreenCaptureHandle> {
    start_capture_with_events(output_dir, interval, stop_flag, None)
}

#[derive(Debug, Clone)]
pub enum ScreenCaptureEvent {
    Captured {
        path: PathBuf,
        observed_at: DateTime<Local>,
        capture_index: u32,
        elapsed_seconds: u64,
        byte_size: u64,
    },
    Failed {
        observed_at: DateTime<Local>,
        error: String,
    },
    Stopped {
        observed_at: DateTime<Local>,
    },
}

pub type ScreenCaptureEventSink = Arc<dyn Fn(ScreenCaptureEvent) + Send + Sync + 'static>;

/// Start capture with a lightweight event callback. The callback runs only
/// after the platform screenshot command has returned, so context-store work
/// never blocks the OS capture operation itself.
pub fn start_capture_with_events(
    output_dir: &Path,
    interval: Duration,
    stop_flag: Arc<AtomicBool>,
    event_sink: Option<ScreenCaptureEventSink>,
) -> std::io::Result<ScreenCaptureHandle> {
    std::fs::create_dir_all(output_dir)?;

    // Set directory permissions to 0700 (owner-only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output_dir, std::fs::Permissions::from_mode(0o700))?;
    }

    let dir = output_dir.to_path_buf();
    // The handle owns a second stop flag alongside the caller's shared one:
    // not every record-loop exit path sets the shared flag (e.g. the
    // `minutes stop` sentinel and safety-guard auto-stop), and Drop joins the
    // capture thread — without a handle-owned signal, drop would block while
    // screenshots keep being taken after the recording ends (#423).
    let own_stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop_flag.clone();
    let thread_own_stop = own_stop.clone();

    let handle = std::thread::spawn(move || {
        let should_stop =
            || thread_stop.load(Ordering::Relaxed) || thread_own_stop.load(Ordering::Relaxed);
        let mut index: u32 = 0;
        let start = std::time::Instant::now();

        tracing::info!(
            dir = %dir.display(),
            interval_secs = interval.as_secs(),
            "screen capture started"
        );

        // Wait one interval before first capture (skip the t=0 screenshot
        // which is usually taken before the meeting content is on screen)
        let first_sleep_end = std::time::Instant::now() + interval;
        while std::time::Instant::now() < first_sleep_end {
            if should_stop() {
                tracing::info!(
                    captures = 0,
                    "screen capture stopped (before first capture)"
                );
                if let Some(sink) = &event_sink {
                    sink(ScreenCaptureEvent::Stopped {
                        observed_at: Local::now(),
                    });
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }

        while !should_stop() && index < MAX_SCREENSHOTS {
            let elapsed = start.elapsed().as_secs();
            let filename = format!("screen-{:04}-{:04}s.png", index, elapsed);
            let path = dir.join(&filename);

            let capture_index = index;
            if let Err(e) = capture_screenshot(&path) {
                tracing::warn!("screen capture failed: {}", e);
                if let Some(sink) = &event_sink {
                    sink(ScreenCaptureEvent::Failed {
                        observed_at: Local::now(),
                        error: e.to_string(),
                    });
                }
                // Don't break — transient failures (e.g., screen locked) are OK
            } else {
                // Set file permissions to 0600 (owner-only)
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
                }
                tracing::debug!(file = %filename, "screen captured");
                if let Some(sink) = &event_sink {
                    sink(ScreenCaptureEvent::Captured {
                        path: path.clone(),
                        observed_at: Local::now(),
                        capture_index,
                        elapsed_seconds: elapsed,
                        byte_size: path.metadata().map(|metadata| metadata.len()).unwrap_or(0),
                    });
                }
                index += 1;
            }

            // Sleep in small increments so we can respond to stop quickly
            let sleep_end = std::time::Instant::now() + interval;
            while std::time::Instant::now() < sleep_end {
                if should_stop() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }

        tracing::info!(captures = index, "screen capture stopped");
        if let Some(sink) = &event_sink {
            sink(ScreenCaptureEvent::Stopped {
                observed_at: Local::now(),
            });
        }
    });

    Ok(ScreenCaptureHandle {
        stop: own_stop,
        thread: Some(handle),
    })
}

pub const CURRENT_SESSION_FILE: &str = "CURRENT_SESSION.md";

/// Write the small dynamic state breadcrumb consumed by PTY assistants. It
/// deliberately contains no image bytes or transcript text; the CLI remains
/// the authoritative bounded retrieval path.
pub fn write_current_session_status(
    status: &crate::context_store::ScreenContextStatus,
) -> std::io::Result<PathBuf> {
    let workspace = crate::config::Config::minutes_dir().join("assistant");
    std::fs::create_dir_all(&workspace)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&workspace, std::fs::Permissions::from_mode(0o700))?;
    }

    let path = workspace.join(CURRENT_SESSION_FILE);
    let session_id = status
        .context_session_id
        .as_deref()
        .unwrap_or("unavailable");
    let last_capture = status
        .last_successful_capture_at
        .map(|timestamp| timestamp.to_rfc3339())
        .unwrap_or_else(|| "none".into());
    let error = status.most_recent_error.as_deref().unwrap_or("none");
    let state = serde_json::to_string(&status.state)
        .unwrap_or_else(|_| "\"unknown\"".into())
        .trim_matches('"')
        .to_string();
    let content = format!(
        "# Current Minutes Session\n\n\
- Context session: `{session_id}`\n\
- Screen context state: `{state}`\n\
- Successful captures: {}\n\
- Last successful capture: `{last_capture}`\n\
- Most recent error: `{error}`\n\
- Updated: `{}`\n\n\
Screen images and desktop app/window metadata are separate evidence lanes. Run \
`minutes context status --json` to refresh state and `minutes context screen \
--session {session_id} --limit 1 --json` to retrieve a verified image. Never \
claim to see screen contents until a specific returned image has been opened.\n",
        status.successful_capture_count,
        Local::now().to_rfc3339(),
    );
    std::fs::write(&path, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(path)
}

/// Maximum number of screenshots per recording session.
/// 8 images × ~200KB (after resize) = ~1.6 MB total — safe for LLM APIs.
const MAX_SCREENSHOTS: u32 = 60;

/// Target resolution for screenshots (width in pixels).
/// Full Retina screenshots are 3-8 MB; resizing to 1280px wide reduces to ~200KB.
#[cfg(target_os = "macos")]
const TARGET_WIDTH: u32 = 1280;

/// Capture a single screenshot to the given path, downscaled to TARGET_WIDTH.
fn capture_screenshot(path: &Path) -> std::io::Result<()> {
    // macOS: screencapture to temp file, then resize with sips
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("screencapture")
            .args(["-x", "-C", "-t", "png"])
            .arg(path)
            .output()?;

        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "screencapture failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        // Downscale to reduce file size (Retina screenshots are 3-8 MB)
        let _ = std::process::Command::new("sips")
            .args([
                "--resampleWidth",
                &TARGET_WIDTH.to_string(),
                "-s",
                "format",
                "png",
            ])
            .arg(path)
            .output(); // Best-effort — if sips fails, keep the full-res image
    }

    // Linux: try scrot, fall back to gnome-screenshot
    #[cfg(target_os = "linux")]
    {
        let result = std::process::Command::new("scrot").arg(path).output();

        match result {
            Ok(output) if output.status.success() => {}
            _ => {
                // Fall back to gnome-screenshot
                let output = std::process::Command::new("gnome-screenshot")
                    .args(["--file"])
                    .arg(path)
                    .output()?;

                if !output.status.success() {
                    return Err(std::io::Error::other("no screenshot tool available"));
                }
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        return Err(std::io::Error::other(
            "screen capture not supported on this platform",
        ));
    }

    Ok(())
}

/// Derive the screenshots directory for a given audio recording path.
/// e.g., `/tmp/recording.wav` → `~/.minutes/screens/recording/`
pub fn screens_dir_for(audio_path: &Path) -> PathBuf {
    let stem = audio_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".minutes")
        .join("screens")
        .join(stem)
}

/// List all screenshot files in a directory, sorted by name (chronological).
pub fn list_screenshots(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("png"))
        .collect();

    files.sort();
    files
}

/// Parse the elapsed-seconds component out of a capture filename
/// (`screen-{index:04}-{elapsed:04}s.png`, written above). Returns None for
/// filenames that don't follow the capture format (e.g. user-provided PNGs),
/// letting callers fall back to order-based handling.
pub fn elapsed_secs_from_filename(path: &Path) -> Option<u64> {
    let stem = path.file_stem()?.to_str()?;
    let rest = stem.strip_prefix("screen-")?;
    let (_, elapsed) = rest.split_once('-')?;
    elapsed.strip_suffix('s')?.parse().ok()
}

/// Handle that represents an active screen capture session.
/// The capture thread runs until the shared stop_flag is set or the handle
/// is dropped. Dropping signals the handle-owned stop flag before joining,
/// so no screenshots are captured after recording stops regardless of
/// whether the caller's exit path set the shared flag (#423).
pub struct ScreenCaptureHandle {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ScreenCaptureHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            // Wait for the capture thread to finish (it checks the flags every 250ms)
            handle.join().ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_screenshots_returns_sorted_pngs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("screen-0002-0060s.png"), "fake").unwrap();
        std::fs::write(dir.path().join("screen-0000-0000s.png"), "fake").unwrap();
        std::fs::write(dir.path().join("screen-0001-0030s.png"), "fake").unwrap();
        std::fs::write(dir.path().join("not-a-screenshot.txt"), "fake").unwrap();

        let files = list_screenshots(dir.path());
        assert_eq!(files.len(), 3);
        assert!(files[0].to_str().unwrap().contains("0000"));
        assert!(files[1].to_str().unwrap().contains("0001"));
        assert!(files[2].to_str().unwrap().contains("0002"));
    }

    #[test]
    fn elapsed_secs_parses_capture_filenames_and_rejects_others() {
        use std::path::Path;
        assert_eq!(
            elapsed_secs_from_filename(Path::new("/x/screen-0042-1260s.png")),
            Some(1260)
        );
        assert_eq!(
            elapsed_secs_from_filename(Path::new("screen-0000-0000s.png")),
            Some(0)
        );
        // Elapsed can outgrow the 4-digit padding on very long recordings.
        assert_eq!(
            elapsed_secs_from_filename(Path::new("screen-0100-36000s.png")),
            Some(36000)
        );
        assert_eq!(
            elapsed_secs_from_filename(Path::new("Screenshot 2026-06-17.png")),
            None
        );
        assert_eq!(
            elapsed_secs_from_filename(Path::new("screen-12s.png")),
            None
        );
    }

    // Regression test for #423: `minutes stop` breaks the record loop without
    // setting the shared stop flag, so drop must stop the thread on its own.
    // The interval is long enough that the thread stays in its pre-first-capture
    // wait, so the test never takes a real screenshot.
    #[test]
    fn drop_stops_capture_thread_without_external_stop_flag() {
        let dir = tempfile::tempdir().unwrap();
        let external_stop = Arc::new(AtomicBool::new(false));
        let handle =
            start_capture(dir.path(), Duration::from_secs(3600), external_stop.clone()).unwrap();

        let started = std::time::Instant::now();
        drop(handle);

        // The capture thread polls every 250ms; well under 5s means the join
        // returned because drop signaled its own stop, not the 1h interval.
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "drop blocked on the capture thread: took {:?}",
            started.elapsed()
        );
        assert!(!external_stop.load(Ordering::Relaxed));
        assert!(list_screenshots(dir.path()).is_empty());
    }

    #[test]
    fn current_session_breadcrumb_is_metadata_only_and_requires_image_inspection() {
        let _lock = crate::test_home_env_lock();
        let home = tempfile::tempdir().unwrap();
        let original_home = std::env::var_os("HOME");
        #[cfg(windows)]
        let original_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", home.path());
        #[cfg(windows)]
        std::env::set_var("USERPROFILE", home.path());

        let status = crate::context_store::ScreenContextStatus {
            context_session_id: Some("ctx-test".into()),
            state: crate::context_store::ScreenContextState::Capturing,
            successful_capture_count: 2,
            ..crate::context_store::ScreenContextStatus::default()
        };
        let path = write_current_session_status(&status).unwrap();
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("Screen context state: `capturing`"));
        assert!(content.contains("minutes context screen"));
        assert!(content.contains("Never claim to see screen contents"));
        assert!(!content.contains(".png"));

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        #[cfg(windows)]
        if let Some(profile) = original_userprofile {
            std::env::set_var("USERPROFILE", profile);
        } else {
            std::env::remove_var("USERPROFILE");
        }
    }
}
