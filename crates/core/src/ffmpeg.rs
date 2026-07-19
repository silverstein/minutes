use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

const FFMPEG_ENV_VAR: &str = "MINUTES_FFMPEG";
const FFMPEG_PROBE_DEADLINE: Duration = Duration::from_secs(3);
const FFMPEG_PROBE_OUTPUT_LIMIT: u64 = 256 * 1024;
const FFMPEG_PROBE_STDERR_LIMIT: usize = 64 * 1024;

/// Error returned when Minutes cannot resolve an executable `ffmpeg` binary.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct ResolveFfmpegError {
    message: String,
}

impl ResolveFfmpegError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Resolve the `ffmpeg` executable used by decoding, stem mixing, diarization,
/// health checks, and evaluation clip extraction.
///
/// Dock/Finder-launched macOS apps inherit a minimal launchd `PATH`, so a bare
/// `Command::new("ffmpeg")` misses Homebrew installs under `/opt/homebrew/bin`.
/// Resolution order is:
///
/// 1. `MINUTES_FFMPEG` explicit override
/// 2. `PATH` lookup via the `which` crate
/// 3. common macOS/Linux install locations
pub fn resolve_ffmpeg() -> Result<PathBuf, ResolveFfmpegError> {
    resolve_ffmpeg_with_candidates(default_known_ffmpeg_candidates())
}

/// Resolve ffmpeg and prove that the selected image can start and report an
/// ffmpeg version banner through the same bounded child boundary used for
/// decoding.
///
/// Metadata-only resolution is intentionally retained for callers that are
/// about to launch ffmpeg themselves. Readiness surfaces and watcher preflight
/// use this stronger check so a corrupt, foreign, or otherwise non-launchable
/// image cannot be reported as available while an import is left stranded.
/// This is not a codec/demuxer capability certification; each input remains
/// fail-closed at the actual bounded decode boundary.
pub fn resolve_launchable_ffmpeg() -> Result<PathBuf, ResolveFfmpegError> {
    let path = resolve_ffmpeg()?;
    verify_ffmpeg_launch(&path).map_err(|detail| {
        ResolveFfmpegError::new(format!(
            "ffmpeg was found at '{}' but could not be used: {detail}",
            path.display()
        ))
    })?;
    Ok(path)
}

fn verify_ffmpeg_launch(path: &Path) -> Result<(), String> {
    let mut command = crate::bounded_child::BoundedCommand::new(path);
    command.arg("-version");
    let run = crate::bounded_child::run(
        &mut command,
        None,
        crate::bounded_child::StdoutTarget::Capture {
            max_bytes: FFMPEG_PROBE_OUTPUT_LIMIT,
        },
        crate::bounded_child::ChildBudget {
            wall_clock: FFMPEG_PROBE_DEADLINE,
            stderr_tail: FFMPEG_PROBE_STDERR_LIMIT,
        },
    )
    .map_err(|error| format!("the bounded launch probe failed: {error}"))?;

    if run.timed_out {
        return Err("the bounded launch probe timed out".into());
    }
    if !run.output.status.success() {
        let stderr = String::from_utf8_lossy(&run.output.stderr);
        return Err(format!(
            "the launch probe exited unsuccessfully{}",
            stderr
                .lines()
                .last()
                .filter(|line| !line.trim().is_empty())
                .map(|line| format!(": {}", line.trim()))
                .unwrap_or_default()
        ));
    }

    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let stderr = String::from_utf8_lossy(&run.output.stderr);
    if !stdout.lines().chain(stderr.lines()).any(|line| {
        line.trim_start()
            .to_ascii_lowercase()
            .starts_with("ffmpeg version")
    }) {
        return Err("the launch probe did not report an ffmpeg version banner".into());
    }
    Ok(())
}

fn resolve_ffmpeg_with_candidates(
    known_candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<PathBuf, ResolveFfmpegError> {
    let mut candidates_tried = Vec::new();

    if let Some(configured) = std::env::var_os(FFMPEG_ENV_VAR) {
        let configured = PathBuf::from(configured);
        candidates_tried.push(format!("{}={}", FFMPEG_ENV_VAR, configured.display()));
        if verify_ffmpeg_binary(&configured).is_ok() {
            return Ok(configured);
        }
        return Err(ResolveFfmpegError::new(format!(
            "{} points to '{}' but it is not an executable file.",
            FFMPEG_ENV_VAR,
            configured.display()
        )));
    }

    if let Ok(path_binary) = which::which("ffmpeg") {
        candidates_tried.push(format!("PATH:{}", path_binary.display()));
        if verify_ffmpeg_binary(&path_binary).is_ok() {
            return Ok(path_binary);
        }
    } else {
        candidates_tried.push("PATH:ffmpeg".to_string());
    }

    for candidate in dedupe_paths(known_candidates) {
        candidates_tried.push(candidate.display().to_string());
        if verify_ffmpeg_binary(&candidate).is_ok() {
            return Ok(candidate);
        }
    }

    Err(ResolveFfmpegError::new(format!(
        "No executable ffmpeg binary was found. Tried: {}. Install ffmpeg (macOS: brew install ffmpeg) or set {} to an absolute ffmpeg path.",
        candidates_tried.join(", "),
        FFMPEG_ENV_VAR
    )))
}

fn default_known_ffmpeg_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/opt/homebrew/bin/ffmpeg"),
        PathBuf::from("/usr/local/bin/ffmpeg"),
        PathBuf::from("/usr/bin/ffmpeg"),
    ]
}

fn dedupe_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut deduped = Vec::new();
    for path in paths {
        if !deduped.iter().any(|existing: &PathBuf| existing == &path) {
            deduped.push(path);
        }
    }
    deduped
}

fn verify_ffmpeg_binary(path: &Path) -> Result<(), ()> {
    if !path.is_file() {
        return Err(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .map_err(|_| ())?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err(());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsStr, fs};

    fn set_env_var(key: &str, value: impl AsRef<OsStr>) -> Option<std::ffi::OsString> {
        let old = std::env::var_os(key);
        std::env::set_var(key, value);
        old
    }

    fn restore_env_var(key: &str, previous: Option<std::ffi::OsString>) {
        if let Some(previous) = previous {
            std::env::set_var(key, previous);
        } else {
            std::env::remove_var(key);
        }
    }

    #[cfg(unix)]
    fn write_fake_ffmpeg(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        fs::write(path, "#!/bin/sh\nprintf 'ffmpeg fake\\n'\n").unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(windows)]
    fn write_fake_ffmpeg(path: &Path) {
        fs::write(path, "@echo off\r\necho ffmpeg fake\r\n").unwrap();
    }

    #[test]
    fn env_override_wins_over_path_and_known_locations() {
        let _env_lock = crate::test_home_env_lock();
        let env_dir = tempfile::TempDir::new().unwrap();
        let path_dir = tempfile::TempDir::new().unwrap();
        let known_dir = tempfile::TempDir::new().unwrap();
        let env_binary = env_dir.path().join("ffmpeg-env");
        let path_binary = path_dir.path().join("ffmpeg");
        let known_binary = known_dir.path().join("ffmpeg");
        write_fake_ffmpeg(&env_binary);
        write_fake_ffmpeg(&path_binary);
        write_fake_ffmpeg(&known_binary);

        let old_override = set_env_var(FFMPEG_ENV_VAR, env_binary.as_os_str());
        let old_path = set_env_var("PATH", path_dir.path().as_os_str());

        let resolved = resolve_ffmpeg_with_candidates([known_binary]).unwrap();
        assert_eq!(resolved, env_binary);

        restore_env_var("PATH", old_path);
        restore_env_var(FFMPEG_ENV_VAR, old_override);
    }

    #[test]
    fn errors_clearly_when_no_candidate_resolves() {
        let _env_lock = crate::test_home_env_lock();
        let empty_path = tempfile::TempDir::new().unwrap();
        let missing_dir = tempfile::TempDir::new().unwrap();
        let old_override = std::env::var_os(FFMPEG_ENV_VAR);
        std::env::remove_var(FFMPEG_ENV_VAR);
        let old_path = set_env_var("PATH", empty_path.path().as_os_str());

        let error = resolve_ffmpeg_with_candidates([missing_dir.path().join("ffmpeg")])
            .unwrap_err()
            .to_string();
        assert!(error.contains("No executable ffmpeg binary was found"));
        assert!(error.contains("MINUTES_FFMPEG"));

        restore_env_var("PATH", old_path);
        restore_env_var(FFMPEG_ENV_VAR, old_override);
    }

    #[cfg(unix)]
    #[test]
    fn metadata_approved_foreign_image_fails_bounded_launch_probe() {
        use std::os::unix::fs::PermissionsExt;

        let _env_lock = crate::test_home_env_lock();
        let temp = tempfile::TempDir::new().unwrap();
        let foreign_image = temp.path().join("ffmpeg");
        fs::write(&foreign_image, b"MZ\x90\0synthetic foreign image").unwrap();
        fs::set_permissions(&foreign_image, fs::Permissions::from_mode(0o700)).unwrap();
        let old_override = set_env_var(FFMPEG_ENV_VAR, foreign_image.as_os_str());

        assert_eq!(resolve_ffmpeg().unwrap(), foreign_image);
        let error = resolve_launchable_ffmpeg().unwrap_err().to_string();

        restore_env_var(FFMPEG_ENV_VAR, old_override);
        assert!(error.contains("could not be used"));
        assert!(error.contains("bounded launch probe failed"));
    }
}
