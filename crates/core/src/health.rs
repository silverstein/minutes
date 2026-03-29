//! System health checks for readiness diagnostics.
//!
//! Used by both the CLI (`minutes health`) and the Tauri permission center.
//! Each check returns a `HealthItem` with a label, state, detail, and optionality flag.
//!
//! ```text
//!   CHECK FLOW:
//!   Config → model_status()   → HealthItem { state: ready | attention }
//!          → mic_status()     → HealthItem
//!          → calendar_status()→ HealthItem (macOS only)
//!          → watcher_status() → HealthItem
//!          → output_dir_status() → HealthItem
//!          → disk_space()     → HealthItem
//! ```

use crate::config::Config;
use serde::{Deserialize, Serialize};

/// A single health check result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthItem {
    pub label: String,
    pub state: String,
    pub detail: String,
    pub optional: bool,
}

/// Run all health checks and return the results.
pub fn check_all(config: &Config) -> Vec<HealthItem> {
    vec![
        model_status(config),
        vad_model_status(config),
        ffmpeg_status(),
        diarization_status(config),
        mic_status(),
        calendar_status(),
        watcher_status(config),
        output_dir_status(config),
        disk_space(config),
    ]
}

/// Check if the whisper model is downloaded and ready.
pub fn model_status(config: &Config) -> HealthItem {
    let model_name = &config.transcription.model;
    let model_file = config
        .transcription
        .model_path
        .join(format!("ggml-{}.bin", model_name));
    let exists = model_file.exists();

    HealthItem {
        label: "Speech model".into(),
        state: if exists { "ready" } else { "attention" }.into(),
        detail: if exists {
            format!("{} is installed at {}.", model_name, model_file.display())
        } else {
            format!(
                "{} is not installed yet. Run `minutes setup` to download it.",
                model_name
            )
        },
        optional: false,
    }
}

/// Check if the Silero VAD model is downloaded (improves non-English transcription).
pub fn vad_model_status(config: &Config) -> HealthItem {
    let vad_model = &config.transcription.vad_model;
    if vad_model.is_empty() {
        return HealthItem {
            label: "VAD model".into(),
            state: "ready".into(),
            detail: "Disabled (vad_model is empty). Energy-based silence detection will be used."
                .into(),
            optional: true,
        };
    }

    let model_dir = &config.transcription.model_path;
    let mut candidates = vec![model_dir.join(format!("ggml-{}.bin", vad_model))];
    // Accept old filename for backward compatibility (only for silero variants)
    if vad_model.starts_with("silero") {
        candidates.push(model_dir.join("ggml-silero-vad.bin"));
    }
    let found = candidates.iter().find(|p| p.exists());

    HealthItem {
        label: "VAD model".into(),
        state: if found.is_some() {
            "ready"
        } else {
            "attention"
        }
        .into(),
        detail: if let Some(path) = found {
            format!("Silero VAD installed at {}.", path.display())
        } else {
            "Silero VAD not installed. Run `minutes setup` to download it. \
             Without it, non-English audio may produce transcription loops."
                .into()
        },
        optional: true,
    }
}

/// Check if ffmpeg is available for audio decoding.
pub fn ffmpeg_status() -> HealthItem {
    let available = std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok();

    HealthItem {
        label: "ffmpeg".into(),
        state: if available { "ready" } else { "attention" }.into(),
        detail: if available {
            "Installed. Used for high-quality audio decoding of m4a/mp3/ogg files.".into()
        } else {
            "Not found. Non-English audio in m4a/mp3/ogg format may produce poor transcriptions. \
             Install: brew install ffmpeg (macOS) / apt install ffmpeg (Linux)"
                .into()
        },
        optional: true,
    }
}

/// Check if diarization models are downloaded (when diarization is enabled).
pub fn diarization_status(config: &Config) -> HealthItem {
    if config.diarization.engine == "none" {
        return HealthItem {
            label: "Speaker diarization".into(),
            state: "ready".into(),
            detail: "Disabled. Remove `diarization.engine = \"none\"` from config to auto-detect."
                .into(),
            optional: true,
        };
    }

    let is_auto = config.diarization.engine == "auto";
    let is_pyannote_rs = config.diarization.engine == "pyannote-rs" || is_auto;

    if is_pyannote_rs {
        let installed = crate::diarize::models_installed(config);
        return HealthItem {
            label: "Speaker diarization".into(),
            state: if installed || is_auto {
                "ready"
            } else {
                "attention"
            }
            .into(),
            detail: if installed {
                let mode = if is_auto { "auto-detected" } else { "enabled" };
                format!("pyannote-rs models installed ({mode}). Meetings will identify speakers.",)
            } else if is_auto {
                "Models not downloaded — diarization will be skipped. \
                 Run `minutes setup --diarization` to enable speaker identification (~34 MB)."
                    .into()
            } else {
                "Models not downloaded. Run `minutes setup --diarization` to install (~34 MB)."
                    .into()
            },
            optional: true,
        };
    }

    // Legacy pyannote (Python) or other engines
    HealthItem {
        label: "Speaker diarization".into(),
        state: "ready".into(),
        detail: format!("Using {} engine.", config.diarization.engine),
        optional: true,
    }
}

/// Check if audio input devices are available.
pub fn mic_status() -> HealthItem {
    let devices = crate::capture::list_input_devices();
    let has_devices = !devices.is_empty();

    HealthItem {
        label: "Microphone & audio input".into(),
        state: if has_devices { "ready" } else { "attention" }.into(),
        detail: if has_devices {
            format!(
                "{} audio input device{} detected.",
                devices.len(),
                if devices.len() == 1 { "" } else { "s" }
            )
        } else {
            "No audio input devices detected. Check hardware and system settings.".into()
        },
        optional: false,
    }
}

/// Check macOS calendar access (macOS only, returns unavailable on other platforms).
pub fn calendar_status() -> HealthItem {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("osascript");
        cmd.arg("-e")
            .arg(r#"tell application "Calendar" to get name of every calendar"#);
        let output = crate::calendar::output_with_timeout(cmd, std::time::Duration::from_secs(10))
            .map(|o| if o.status.success() { Ok(o) } else { Err(o) });

        match output {
            Some(Ok(_)) => HealthItem {
                label: "Calendar access".into(),
                state: "ready".into(),
                detail: "Calendar access is available for meeting suggestions.".into(),
                optional: true,
            },
            Some(Err(_)) => HealthItem {
                label: "Calendar access".into(),
                state: "attention".into(),
                detail: "Calendar access is unavailable. Meeting suggestions will be hidden."
                    .into(),
                optional: true,
            },
            None => HealthItem {
                label: "Calendar access".into(),
                state: "attention".into(),
                detail: "Calendar check timed out. Meeting suggestions will be hidden.".into(),
                optional: true,
            },
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        HealthItem {
            label: "Calendar access".into(),
            state: "attention".into(),
            detail: "Calendar integration is macOS-only.".into(),
            optional: true,
        }
    }
}

/// Check if configured watch paths exist.
pub fn watcher_status(config: &Config) -> HealthItem {
    let existing = config
        .watch
        .paths
        .iter()
        .filter(|path| path.exists())
        .count();
    let total = config.watch.paths.len();

    let state = if total == 0 || existing == total {
        "ready"
    } else {
        "attention"
    };

    let detail = if total == 0 {
        "No watch folders configured. Voice-memo ingestion is available but not set up.".into()
    } else if existing == total {
        format!(
            "{} watch folder{} ready.",
            total,
            if total == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "{} of {} watch folders exist. Missing folders will prevent inbox processing.",
            existing, total
        )
    };

    HealthItem {
        label: "Watcher folders".into(),
        state: state.into(),
        detail,
        optional: true,
    }
}

/// Check if the output directory exists.
pub fn output_dir_status(config: &Config) -> HealthItem {
    let exists = config.output_dir.exists();
    HealthItem {
        label: "Meeting output folder".into(),
        state: if exists { "ready" } else { "attention" }.into(),
        detail: if exists {
            format!("Meetings are stored in {}.", config.output_dir.display())
        } else {
            format!(
                "Output folder {} does not exist yet. Minutes will create it on demand.",
                config.output_dir.display()
            )
        },
        optional: false,
    }
}

/// Check available disk space in the output directory.
pub fn disk_space(config: &Config) -> HealthItem {
    let target = if config.output_dir.exists() {
        &config.output_dir
    } else {
        // Fall back to home dir
        std::path::Path::new("/")
    };

    // Use statvfs on unix, fallback message on other platforms
    #[cfg(unix)]
    {
        let stat = nix_disk_free(target);
        match stat {
            Some(free_gb) if free_gb < 1.0 => HealthItem {
                label: "Disk space".into(),
                state: "attention".into(),
                detail: format!(
                    "{:.1} GB free. Recordings may fail if disk fills up.",
                    free_gb
                ),
                optional: false,
            },
            Some(free_gb) => HealthItem {
                label: "Disk space".into(),
                state: "ready".into(),
                detail: format!("{:.1} GB free.", free_gb),
                optional: false,
            },
            None => HealthItem {
                label: "Disk space".into(),
                state: "ready".into(),
                detail: "Could not determine free disk space.".into(),
                optional: false,
            },
        }
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        HealthItem {
            label: "Disk space".into(),
            state: "ready".into(),
            detail: "Disk space check is not available on this platform.".into(),
            optional: false,
        }
    }
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
fn nix_disk_free(path: &std::path::Path) -> Option<f64> {
    use std::ffi::CString;
    let c_path = CString::new(path.to_str()?).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if ret == 0 {
        // Cast needed: field widths vary by platform (u32 on some, u64 on others)
        let free_bytes = (stat.f_bavail as u64) * (stat.f_frsize as u64);
        Some(free_bytes as f64 / 1_073_741_824.0)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_all_returns_items() {
        let config = Config::default();
        let items = check_all(&config);
        assert!(items.len() >= 5, "should have at least 5 health checks");
        for item in &items {
            assert!(!item.label.is_empty());
            assert!(
                item.state == "ready" || item.state == "attention",
                "state should be ready or attention, got: {}",
                item.state
            );
        }
    }

    #[test]
    fn test_model_status_missing() {
        let mut config = Config::default();
        config.transcription.model = "nonexistent-model-xyz".into();
        let status = model_status(&config);
        assert_eq!(status.state, "attention");
        assert!(!status.optional);
    }

    #[test]
    fn test_output_dir_missing() {
        let mut config = Config::default();
        config.output_dir = "/nonexistent/path/12345".into();
        let status = output_dir_status(&config);
        assert_eq!(status.state, "attention");
    }

    #[test]
    fn test_watcher_no_paths() {
        let mut config = Config::default();
        config.watch.paths.clear();
        let status = watcher_status(&config);
        assert_eq!(status.state, "ready"); // no paths = not configured, not broken
        assert!(status.optional);
    }

    #[test]
    fn test_disk_space_root() {
        let config = Config::default();
        let status = disk_space(&config);
        // Should always return something on any machine
        assert!(!status.label.is_empty());
    }
}
