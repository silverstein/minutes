//! Stop-time handling for standalone live transcript artifacts.
//!
//! The real-time live writer intentionally uses fixed scratch paths so readers
//! can tail a stable JSONL file. Once the writer stops, this module gets those
//! artifacts out of the overwrite slot and optionally promotes the WAV through
//! the same meeting pipeline used by normal processing.

use crate::config::{Config, LiveTranscriptPromoteOnStop};
use crate::error::MinutesError;
use crate::markdown::ContentType;
use chrono::{DateTime, Local};
use std::fs;
use std::path::{Path, PathBuf};

const LIVE_SESSIONS_DIR: &str = "live-sessions";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveSessionStopResult {
    NoSavedAudio {
        jsonl_path: PathBuf,
    },
    Off {
        wav_path: PathBuf,
        jsonl_path: PathBuf,
    },
    Preserved {
        wav_path: PathBuf,
        jsonl_path: PathBuf,
    },
    Processed {
        meeting_path: PathBuf,
        wav_path: PathBuf,
        jsonl_path: PathBuf,
    },
}

impl LiveSessionStopResult {
    /// Primary user-facing output: meeting for processing, WAV for preservation,
    /// and JSONL when no saved audio was available or legacy behavior is active.
    pub fn output_path(&self) -> &Path {
        match self {
            Self::NoSavedAudio { jsonl_path } | Self::Off { jsonl_path, .. } => jsonl_path,
            Self::Preserved { wav_path, .. } => wav_path,
            Self::Processed { meeting_path, .. } => meeting_path,
        }
    }

    pub fn jsonl_path(&self) -> &Path {
        match self {
            Self::NoSavedAudio { jsonl_path }
            | Self::Off { jsonl_path, .. }
            | Self::Preserved { jsonl_path, .. }
            | Self::Processed { jsonl_path, .. } => jsonl_path,
        }
    }

    pub fn wav_path(&self) -> Option<&Path> {
        match self {
            Self::NoSavedAudio { .. } => None,
            Self::Off { wav_path, .. }
            | Self::Preserved { wav_path, .. }
            | Self::Processed { wav_path, .. } => Some(wav_path),
        }
    }

    pub fn meeting_path(&self) -> Option<&Path> {
        match self {
            Self::Processed { meeting_path, .. } => Some(meeting_path),
            _ => None,
        }
    }
}

/// Finalize a stopped standalone live transcript session.
///
/// `wav_path` and `jsonl_path` are the fixed scratch paths used while live.
/// When a WAV exists, process/preserve modes move both files to a timestamped
/// pair before doing any expensive work, so a later live session cannot clobber
/// the recoverable source even if meeting processing fails.
pub fn finalize_stopped_live_session(
    config: &Config,
    wav_path: &Path,
    jsonl_path: &Path,
) -> Result<LiveSessionStopResult, MinutesError> {
    finalize_stopped_live_session_with_release(config, wav_path, jsonl_path, || {})
}

/// Variant used by the live runner to release its liveness lock only after the
/// fixed scratch files are safe, but before the expensive meeting pipeline.
pub(crate) fn finalize_stopped_live_session_with_release<F>(
    config: &Config,
    wav_path: &Path,
    jsonl_path: &Path,
    release_live_lock: F,
) -> Result<LiveSessionStopResult, MinutesError>
where
    F: FnOnce(),
{
    finalize_stopped_live_session_at(
        config,
        wav_path,
        jsonl_path,
        Local::now(),
        release_live_lock,
    )
}

fn finalize_stopped_live_session_at<F>(
    config: &Config,
    wav_path: &Path,
    jsonl_path: &Path,
    stopped_at: DateTime<Local>,
    release_live_lock: F,
) -> Result<LiveSessionStopResult, MinutesError>
where
    F: FnOnce(),
{
    if !wav_path.exists() {
        release_live_lock();
        return Ok(LiveSessionStopResult::NoSavedAudio {
            jsonl_path: jsonl_path.to_path_buf(),
        });
    }

    if config.live_transcript.promote_on_stop == LiveTranscriptPromoteOnStop::Off {
        release_live_lock();
        tracing::warn!(
            wav = %wav_path.display(),
            jsonl = %jsonl_path.display(),
            "live transcript artifacts remain in the fixed scratch slot and will be overwritten by the next live session"
        );
        eprintln!(
            "[minutes] Warning: live transcript remains at {}; the next `minutes live` will overwrite it",
            wav_path.display()
        );
        return Ok(LiveSessionStopResult::Off {
            wav_path: wav_path.to_path_buf(),
            jsonl_path: jsonl_path.to_path_buf(),
        });
    }

    let (preserved_wav, preserved_jsonl) = preserve_live_pair(wav_path, jsonl_path, stopped_at)?;
    release_live_lock();

    if config.live_transcript.promote_on_stop == LiveTranscriptPromoteOnStop::Preserve {
        tracing::info!(
            wav = %preserved_wav.display(),
            jsonl = %preserved_jsonl.display(),
            "live transcript artifacts preserved"
        );
        return Ok(LiveSessionStopResult::Preserved {
            wav_path: preserved_wav,
            jsonl_path: preserved_jsonl,
        });
    }

    let meeting = crate::process(&preserved_wav, ContentType::Meeting, None, config)?;
    tracing::info!(
        meeting = %meeting.path.display(),
        wav = %preserved_wav.display(),
        jsonl = %preserved_jsonl.display(),
        "live transcript promoted to meeting"
    );
    Ok(LiveSessionStopResult::Processed {
        meeting_path: meeting.path,
        wav_path: preserved_wav,
        jsonl_path: preserved_jsonl,
    })
}

fn preserve_live_pair(
    wav_path: &Path,
    jsonl_path: &Path,
    stopped_at: DateTime<Local>,
) -> std::io::Result<(PathBuf, PathBuf)> {
    let scratch_dir = wav_path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("live WAV has no parent directory: {}", wav_path.display()),
        )
    })?;
    let archive_dir = scratch_dir.join(LIVE_SESSIONS_DIR);
    fs::create_dir_all(&archive_dir)?;

    let timestamp = stopped_at.format("%Y%m%d-%H%M%S%3f");
    let mut suffix = 1_u32;
    let (destination_wav, destination_jsonl) = loop {
        let stem = if suffix == 1 {
            format!("live-transcript-{timestamp}")
        } else {
            format!("live-transcript-{timestamp}-{suffix}")
        };
        let candidate_wav = archive_dir.join(format!("{stem}.wav"));
        let candidate_jsonl = archive_dir.join(format!("{stem}.jsonl"));
        if !candidate_wav.exists() && !candidate_jsonl.exists() {
            break (candidate_wav, candidate_jsonl);
        }
        suffix = suffix.saturating_add(1);
    };

    fs::rename(wav_path, &destination_wav)?;
    if jsonl_path.exists() {
        if let Err(error) = fs::rename(jsonl_path, &destination_jsonl) {
            if let Err(rollback_error) = fs::rename(&destination_wav, wav_path) {
                tracing::error!(
                    source = %destination_wav.display(),
                    destination = %wav_path.display(),
                    error = %rollback_error,
                    "failed to roll back live WAV after JSONL preservation failed"
                );
            }
            return Err(error);
        }
    }

    Ok((destination_wav, destination_jsonl))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for sample in 0..16_000 {
            let value = (5000.0
                * (2.0 * std::f32::consts::PI * 440.0 * sample as f32 / 16_000.0).sin())
                as i16;
            writer.write_sample(value).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn stopped_session() -> (tempfile::TempDir, PathBuf, PathBuf, Config) {
        let temp = tempfile::TempDir::new().unwrap();
        let scratch = temp.path().join(".minutes");
        fs::create_dir_all(&scratch).unwrap();
        let wav = scratch.join("live-transcript.wav");
        let jsonl = scratch.join("live-transcript.jsonl");
        write_test_wav(&wav);
        fs::write(&jsonl, "{\"line\":1,\"text\":\"hello\"}\n").unwrap();

        let mut config = Config::default();
        config.output_dir = temp.path().join("meetings");
        config.summarization.engine = "none".into();
        (temp, wav, jsonl, config)
    }

    #[test]
    #[cfg(not(feature = "whisper"))]
    fn stopped_live_process_creates_meeting_and_clears_fixed_slot() {
        let (_temp, wav, jsonl, mut config) = stopped_session();
        config.live_transcript.promote_on_stop = LiveTranscriptPromoteOnStop::Process;

        let result = finalize_stopped_live_session(&config, &wav, &jsonl).unwrap();
        let LiveSessionStopResult::Processed {
            meeting_path,
            wav_path,
            jsonl_path,
        } = result
        else {
            panic!("expected processed live session");
        };

        assert!(meeting_path.exists());
        assert!(meeting_path.starts_with(&config.output_dir));
        assert!(fs::read_to_string(&meeting_path)
            .unwrap()
            .contains("type: meeting"));
        assert!(wav_path.exists());
        assert!(jsonl_path.exists());
        assert!(wav_path.starts_with(wav.parent().unwrap().join(LIVE_SESSIONS_DIR)));
        assert!(!wav.exists());
        assert!(!jsonl.exists());
    }

    #[test]
    #[cfg(not(feature = "whisper"))]
    fn process_releases_live_lock_after_preserve_and_before_pipeline() {
        let (_temp, wav, jsonl, mut config) = stopped_session();
        config.live_transcript.promote_on_stop = LiveTranscriptPromoteOnStop::Process;
        let released = std::cell::Cell::new(false);

        let result = finalize_stopped_live_session_with_release(&config, &wav, &jsonl, || {
            assert!(!wav.exists(), "fixed WAV must be clear before unlock");
            assert!(!jsonl.exists(), "fixed JSONL must be clear before unlock");
            assert!(
                !config.output_dir.exists(),
                "meeting pipeline must not start while the live lock is held"
            );
            released.set(true);
        })
        .unwrap();

        assert!(released.get());
        assert!(matches!(result, LiveSessionStopResult::Processed { .. }));
    }

    #[test]
    fn stopped_live_preserve_moves_timestamped_pair_without_meeting() {
        let (_temp, wav, jsonl, mut config) = stopped_session();
        config.live_transcript.promote_on_stop = LiveTranscriptPromoteOnStop::Preserve;

        let result = finalize_stopped_live_session(&config, &wav, &jsonl).unwrap();
        let LiveSessionStopResult::Preserved {
            wav_path,
            jsonl_path,
        } = result
        else {
            panic!("expected preserved live session");
        };

        assert!(wav_path.exists());
        assert!(jsonl_path.exists());
        assert_ne!(wav_path, wav);
        assert_ne!(jsonl_path, jsonl);
        assert!(wav_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("live-transcript-"));
        assert_eq!(
            fs::read_dir(&config.output_dir)
                .map(|entries| entries.count())
                .unwrap_or(0),
            0
        );
        assert!(!wav.exists());
        assert!(!jsonl.exists());
    }

    #[test]
    fn stopped_live_off_leaves_fixed_pair_in_place() {
        let (_temp, wav, jsonl, mut config) = stopped_session();
        config.live_transcript.promote_on_stop = LiveTranscriptPromoteOnStop::Off;

        let result = finalize_stopped_live_session(&config, &wav, &jsonl).unwrap();

        assert!(matches!(result, LiveSessionStopResult::Off { .. }));
        assert!(wav.exists());
        assert!(jsonl.exists());
        assert!(!wav.parent().unwrap().join(LIVE_SESSIONS_DIR).exists());
        assert!(!config.output_dir.exists());
    }

    #[test]
    fn process_is_the_default_stop_behavior() {
        assert_eq!(
            LiveTranscriptPromoteOnStop::default(),
            LiveTranscriptPromoteOnStop::Process
        );
        assert_eq!(
            Config::default().live_transcript.promote_on_stop,
            LiveTranscriptPromoteOnStop::Process
        );
    }

    #[test]
    fn stop_behavior_deserializes_from_documented_config_values() {
        for (value, expected) in [
            ("process", LiveTranscriptPromoteOnStop::Process),
            ("preserve", LiveTranscriptPromoteOnStop::Preserve),
            ("off", LiveTranscriptPromoteOnStop::Off),
        ] {
            let parsed: crate::config::LiveTranscriptConfig =
                toml::from_str(&format!("promote_on_stop = \"{value}\""))
                    .expect("documented promote_on_stop value should parse");
            assert_eq!(parsed.promote_on_stop, expected);
        }
    }
}
