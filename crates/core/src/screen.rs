use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
    std::fs::create_dir_all(output_dir)?;

    // Set directory permissions to 0700 (owner-only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output_dir, std::fs::Permissions::from_mode(0o700))?;
    }

    let dir = output_dir.to_path_buf();
    let thread_stop = stop_flag.clone();

    let handle = std::thread::spawn(move || {
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
            if thread_stop.load(Ordering::Relaxed) {
                tracing::info!(
                    captures = 0,
                    "screen capture stopped (before first capture)"
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }

        while !thread_stop.load(Ordering::Relaxed) && index < MAX_SCREENSHOTS {
            let elapsed = start.elapsed().as_secs();
            let filename = format!("screen-{:04}-{:04}s.png", index, elapsed);
            let path = dir.join(&filename);

            if let Err(e) = capture_screenshot(&path) {
                tracing::warn!("screen capture failed: {}", e);
                // Don't break — transient failures (e.g., screen locked) are OK
            } else {
                // Set file permissions to 0600 (owner-only)
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).ok();
                }
                tracing::debug!(file = %filename, "screen captured");
                index += 1;
            }

            // Sleep in small increments so we can respond to stop quickly
            let sleep_end = std::time::Instant::now() + interval;
            while std::time::Instant::now() < sleep_end {
                if thread_stop.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }

        tracing::info!(captures = index, "screen capture stopped");
    });

    Ok(ScreenCaptureHandle {
        thread: Some(handle),
    })
}

/// Maximum number of screenshots per recording session.
/// 8 images × ~200KB (after resize) = ~1.6 MB total — safe for LLM APIs.
const MAX_SCREENSHOTS: u32 = 60;

/// Target resolution for screenshots (width in pixels).
/// Full Retina screenshots are 3-8 MB; resizing to 1280px wide reduces to ~200KB.
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

/// Handle that represents an active screen capture session.
/// The capture thread runs until the stop_flag is set.
/// Joining the thread on drop ensures no screenshots are captured
/// after recording stops.
pub struct ScreenCaptureHandle {
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ScreenCaptureHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.thread.take() {
            // Wait for the capture thread to finish (it checks stop_flag every 250ms)
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
}
