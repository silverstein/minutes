use std::path::{Path, PathBuf};

use thiserror::Error;

const FFMPEG_ENV_VAR: &str = "MINUTES_FFMPEG";

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
}
