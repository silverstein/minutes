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
use crate::diarize::{CaptureSource, DiagnosticConfidence, FailureKind};
use crate::markdown::{CaptureWarning, DiarizationPath, RecordingHealth};
use crate::system_audio_backend::{system_audio_backend_for_config, ProbeResult, RouteDescription};
use serde::{Deserialize, Serialize};

const SYSTEM_AUDIO_PROBE_SECS: u32 = 5;

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
        check_system_audio_capture(config),
        screen_recording_status(config),
        calendar_status(config),
        watcher_status(config),
        output_dir_status(config),
        disk_space(config),
    ]
}

/// Probe the configured system-audio capture route, if any.
///
/// A missing route is not itself unhealthy: room meetings and memos are
/// deliberately mic-only. A configured route that captures no signal is
/// unhealthy because source-aware diarization depends on this stem.
pub fn probe_system_audio_capture(
    config: &Config,
) -> Result<Option<(RouteDescription, ProbeResult)>, String> {
    let Some(device_override) = crate::capture::resolve_system_audio_probe_device(config)? else {
        return Ok(None);
    };

    let backend = system_audio_backend_for_config(config, device_override)
        .map_err(|error| error.to_string())?;
    let route = backend.current_route();
    backend
        .probe(SYSTEM_AUDIO_PROBE_SECS)
        .map(|result| Some((route, result)))
        .map_err(|error| error.to_string())
}

pub fn check_system_audio_capture(config: &Config) -> HealthItem {
    match probe_system_audio_capture(config) {
        Ok(None) => HealthItem {
            label: "System audio capture".into(),
            state: "ready".into(),
            detail: "No system-audio source configured. Room and memo recordings use microphone capture only.".into(),
            optional: true,
        },
        Ok(Some((route, result))) => health_item_for_system_audio_probe(Some(&route), &result),
        Err(error) => HealthItem {
            label: "System audio capture".into(),
            state: "attention".into(),
            detail: format!("System-audio probe could not start: {error}"),
            optional: true,
        },
    }
}

pub fn health_item_for_system_audio_probe(
    route: Option<&RouteDescription>,
    result: &ProbeResult,
) -> HealthItem {
    let route_label = route
        .and_then(|route| route.device_name.as_deref())
        .unwrap_or("configured system-audio route");
    let observed = &result.observed_signal;
    let detail = if let Some(kind) = &result.failure_kind {
        format!(
            "Probe on '{route_label}' found {:?}: {} frame(s), max RMS {:.4}, avg RMS {:.4}.",
            kind, observed.frames_captured, observed.max_rms, observed.avg_rms
        )
    } else {
        format!(
            "Probe on '{route_label}' captured signal: {} frame(s), max RMS {:.4}, avg RMS {:.4}.",
            observed.frames_captured, observed.max_rms, observed.avg_rms
        )
    };

    HealthItem {
        label: "System audio capture".into(),
        state: if result.failure_kind.is_none() {
            "ready"
        } else {
            "attention"
        }
        .into(),
        detail,
        optional: true,
    }
}

pub fn recording_health_for_skipped_system_audio_probe(reason: &str) -> RecordingHealth {
    RecordingHealth {
        voice_stem_active_ratio: None,
        system_stem_active_ratio: None,
        system_dominant_ratio: None,
        capture_warnings: vec![CaptureWarning {
            kind: FailureKind::Other {
                code: "system-audio-probe-skipped".into(),
            },
            source: CaptureSource::System,
            message: format!(
                "System-audio readiness probe was skipped before recording. Operator reason: {}",
                reason.trim()
            ),
            diagnostic_confidence: DiagnosticConfidence::Inferred,
        }],
        diarization_path: Some(DiarizationPath::None),
    }
}

pub fn recording_health_for_system_audio_probe_failure(
    route: Option<&RouteDescription>,
    result: &ProbeResult,
) -> RecordingHealth {
    let kind = result.failure_kind.clone().unwrap_or(FailureKind::Other {
        code: "probe-failed".into(),
    });
    let route_label = route
        .and_then(|route| route.device_name.as_deref())
        .unwrap_or("configured system-audio route");
    RecordingHealth {
        voice_stem_active_ratio: None,
        system_stem_active_ratio: None,
        system_dominant_ratio: None,
        capture_warnings: vec![CaptureWarning {
            kind,
            source: CaptureSource::System,
            message: format!(
                "System-audio readiness probe failed before recording on '{}': {} frame(s), max RMS {:.4}, avg RMS {:.4}.",
                route_label,
                result.observed_signal.frames_captured,
                result.observed_signal.max_rms,
                result.observed_signal.avg_rms
            ),
            diagnostic_confidence: result.diagnostic_confidence,
        }],
        diarization_path: Some(DiarizationPath::None),
    }
}

pub const NATIVE_CALL_MICROPHONE_RECOVERY_CODE: &str = "native-call-microphone-stem-recovery";
pub const NATIVE_CALL_SYSTEM_RECOVERY_CODE: &str = "native-call-system-stem-recovery";
pub const NATIVE_CALL_CAPTURE_WARNING_CODE: &str = "native-call-capture-warning";

/// Describe a native call recovered from only one usable PCM stem.
///
/// `surviving_source` is the audio that will actually be transcribed. Keeping
/// that convention consistent makes the frontmatter useful to both people and
/// automated recovery tooling.
pub fn recording_health_for_native_call_stem_recovery(
    surviving_source: CaptureSource,
) -> RecordingHealth {
    let (code, message) = match surviving_source {
        CaptureSource::System => (
            NATIVE_CALL_MICROPHONE_RECOVERY_CODE,
            "Microphone audio was missing or silent; the probable cause is microphone device selection or permission. This transcript contains call/remote audio only.",
        ),
        CaptureSource::Voice => (
            NATIVE_CALL_SYSTEM_RECOVERY_CODE,
            "Call/remote audio was missing or silent. This transcript contains local microphone audio only.",
        ),
        CaptureSource::Both | CaptureSource::Backend => (
            NATIVE_CALL_CAPTURE_WARNING_CODE,
            "Native call capture was degraded, but recoverable audio was preserved.",
        ),
    };

    RecordingHealth {
        voice_stem_active_ratio: None,
        system_stem_active_ratio: None,
        system_dominant_ratio: None,
        capture_warnings: vec![CaptureWarning {
            kind: FailureKind::Other { code: code.into() },
            source: surviving_source,
            message: message.into(),
            diagnostic_confidence: DiagnosticConfidence::Inferred,
        }],
        diarization_path: Some(DiarizationPath::None),
    }
}

pub fn append_native_call_capture_warning(
    health: &mut RecordingHealth,
    message: impl Into<String>,
) {
    health.capture_warnings.push(CaptureWarning {
        kind: FailureKind::Other {
            code: NATIVE_CALL_CAPTURE_WARNING_CODE.into(),
        },
        source: CaptureSource::Both,
        message: message.into(),
        diagnostic_confidence: DiagnosticConfidence::Inferred,
    });
}

/// Check if the whisper model is downloaded and ready.
pub fn model_status(config: &Config) -> HealthItem {
    if config.transcription.engine == "parakeet" {
        return crate::transcription_coordinator::parakeet_health_item(config);
    }

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
    let resolved = crate::ffmpeg::resolve_ffmpeg();
    let available = resolved.is_ok();

    HealthItem {
        label: "ffmpeg".into(),
        state: if available { "ready" } else { "attention" }.into(),
        detail: match resolved {
            Ok(path) => format!(
                "Installed at {}. Used for high-quality audio decoding of m4a/mp3/ogg files.",
                path.display()
            ),
            Err(error) => format!(
                "{} Non-English audio in m4a/mp3/ogg format may produce poor transcriptions.",
                error
            ),
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
            state: if installed {
                "ready"
            } else {
                if is_auto {
                    "ready"
                } else {
                    "attention"
                }
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

/// Check screen-context capture readiness for recordings started from this
/// process's environment.
///
/// macOS: probes Screen Recording permission with a real capture attempt
/// (same check the recording path uses), which also catches the stale-grant
/// case: after an app's code signature changes, System Settings still shows
/// it as enabled while macOS silently denies capture and never re-prompts
/// (#424). Note TCC grants are per-identity — run from a terminal this
/// validates CLI recordings, not the desktop app; the app's own probe at
/// recording start raises a notification when it fails.
pub fn screen_recording_status(config: &Config) -> HealthItem {
    if !config.screen_context.enabled {
        return HealthItem {
            label: "Screen recording".into(),
            state: "ready".into(),
            detail:
                "Screen-context capture disabled. Enable with [screen_context] enabled = true in config.toml."
                    .into(),
            optional: true,
        };
    }

    #[cfg(target_os = "macos")]
    {
        if crate::screen::check_screen_permission() {
            HealthItem {
                label: "Screen recording".into(),
                state: "ready".into(),
                detail: format!(
                    "Screen Recording permission granted for this environment — recordings started here will capture screenshots every {}s. The desktop app's grant is separate; if it fails at recording start, a notification is raised.",
                    config.screen_context.interval_secs
                ),
                optional: true,
            }
        } else {
            HealthItem {
                label: "Screen recording".into(),
                state: "attention".into(),
                detail: "Screen-context capture is enabled but Screen Recording permission is unavailable in this environment — recordings started here will have no screenshots. If System Settings shows the app as enabled, the grant is stale: toggle it off and on under Privacy & Security > Screen & System Audio Recording."
                    .into(),
                optional: true,
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Mirrors capture_screenshot's tool order (scrot, then gnome-screenshot).
        let tool = ["scrot", "gnome-screenshot"].into_iter().find(|tool| {
            crate::engine_process::command(tool)
                .arg("--version")
                .output()
                .is_ok()
        });
        match tool {
            Some(tool) => HealthItem {
                label: "Screen recording".into(),
                state: "ready".into(),
                detail: format!(
                    "Screenshots will be captured with {} every {}s during recordings.",
                    tool, config.screen_context.interval_secs
                ),
                optional: true,
            },
            None => HealthItem {
                label: "Screen recording".into(),
                state: "attention".into(),
                detail: "Screen-context capture is enabled but no screenshot tool was found — install scrot or gnome-screenshot, or recordings will have no screenshots."
                    .into(),
                optional: true,
            },
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        HealthItem {
            label: "Screen recording".into(),
            state: "attention".into(),
            detail:
                "Screen-context capture is not supported on this platform — recordings will have no screenshots."
                    .into(),
            optional: true,
        }
    }
}

/// Check macOS calendar access (macOS only, returns unavailable on other platforms).
pub fn calendar_status(config: &Config) -> HealthItem {
    if !config.calendar.enabled {
        return HealthItem {
            label: "Calendar access".into(),
            state: "ready".into(),
            detail: "Calendar integration disabled. Enable with [calendar] enabled = true in config.toml.".into(),
            optional: true,
        };
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = crate::engine_process::command("osascript");
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
                detail: "Calendar access is unavailable. Meeting suggestions will be hidden. Disable with [calendar] enabled = false in config.toml."
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
        assert!(items.len() >= 6, "should have at least 6 health checks");
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
    fn screen_recording_health_is_ready_when_disabled() {
        let config = Config::default();
        assert!(
            !config.screen_context.enabled,
            "screen context should be disabled by default"
        );
        let item = screen_recording_status(&config);

        assert_eq!(item.label, "Screen recording");
        assert_eq!(item.state, "ready");
        assert!(item.optional);
        assert!(item.detail.contains("disabled"));
    }

    #[test]
    fn system_audio_health_is_ready_when_no_route_configured() {
        let config = Config::default();
        let item = check_system_audio_capture(&config);

        assert_eq!(item.label, "System audio capture");
        assert_eq!(item.state, "ready");
        assert!(item.optional);
        assert!(item.detail.contains("No system-audio source configured"));
    }

    #[test]
    fn system_audio_health_reports_probe_signal() {
        let route = RouteDescription {
            capture_backend: "cpal".into(),
            device_name: Some("BlackHole 2ch".into()),
        };
        let result = ProbeResult {
            observed_signal: crate::diarize::ObservedSignal {
                frames_captured: 1_600,
                max_rms: 0.02,
                avg_rms: 0.01,
            },
            failure_kind: None,
            diagnostic_confidence: DiagnosticConfidence::High,
        };

        let item = health_item_for_system_audio_probe(Some(&route), &result);

        assert_eq!(item.state, "ready");
        assert!(item.detail.contains("BlackHole 2ch"));
        assert!(item.detail.contains("captured signal"));
    }

    #[test]
    fn system_audio_health_reports_silent_probe() {
        let result = ProbeResult {
            observed_signal: crate::diarize::ObservedSignal {
                frames_captured: 1_600,
                max_rms: 0.0,
                avg_rms: 0.0,
            },
            failure_kind: Some(FailureKind::Silent),
            diagnostic_confidence: DiagnosticConfidence::Inferred,
        };

        let item = health_item_for_system_audio_probe(None, &result);

        assert_eq!(item.state, "attention");
        assert!(item.detail.contains("Silent"));
        assert!(item.detail.contains("max RMS 0.0000"));
    }

    #[test]
    fn skipped_system_audio_probe_health_records_reason() {
        let health = recording_health_for_skipped_system_audio_probe("hotel Wi-Fi call");

        assert_eq!(health.diarization_path, Some(DiarizationPath::None));
        assert_eq!(health.capture_warnings.len(), 1);
        assert_eq!(health.capture_warnings[0].source, CaptureSource::System);
        assert!(matches!(
            health.capture_warnings[0].kind,
            FailureKind::Other { ref code } if code == "system-audio-probe-skipped"
        ));
        assert!(health.capture_warnings[0]
            .message
            .contains("hotel Wi-Fi call"));
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
    fn test_parakeet_model_status_missing_assets() {
        let mut config = Config::default();
        config.transcription.engine = "parakeet".into();
        let tmp = tempfile::TempDir::new().unwrap();
        config.transcription.model_path = tmp.path().to_path_buf();
        let status = model_status(&config);
        assert_eq!(status.state, "attention");
        assert!(status.detail.contains("Parakeet not ready"));
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn test_parakeet_model_status_ready_with_metadata() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut config = Config::default();
        config.transcription.engine = "parakeet".into();
        config.transcription.model_path = tmp.path().to_path_buf();
        config.transcription.parakeet_binary = if cfg!(windows) {
            "cmd".into()
        } else {
            "sh".into()
        };
        // The default `parakeet_model` is "tdt-600m"; the test was written
        // when the default was "tdt-ctc-110m" and silently broke when the
        // default moved. Pin the model explicitly so the install-dir lookup
        // and the file fixtures agree regardless of future default changes.
        config.transcription.parakeet_model = "tdt-ctc-110m".into();

        let install_dir = crate::parakeet::install_dir(&config, "tdt-ctc-110m");
        std::fs::create_dir_all(&install_dir).unwrap();
        let model = install_dir.join("tdt-ctc-110m.safetensors");
        let tokenizer = install_dir.join("tdt-ctc-110m.tokenizer.vocab");
        std::fs::write(&model, b"model").unwrap();
        std::fs::write(&tokenizer, b"tokenizer").unwrap();
        crate::parakeet::write_install_metadata(&config, "tdt-ctc-110m", &model, &tokenizer)
            .unwrap();

        let status = model_status(&config);
        assert_eq!(status.state, "ready");
        assert!(status.detail.contains("Metadata:"));
    }

    #[test]
    fn test_output_dir_missing() {
        let config = Config {
            output_dir: "/nonexistent/path/12345".into(),
            ..Config::default()
        };
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
