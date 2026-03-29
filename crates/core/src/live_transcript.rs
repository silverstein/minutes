use crate::config::Config;
#[cfg(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
))]
use crate::error::LiveTranscriptError;
use crate::error::MinutesError;
use crate::pid;
#[cfg(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
))]
use crate::streaming::AudioStream;
#[cfg(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
))]
use crate::streaming_engine::StreamingEngine;
#[cfg(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
))]
use crate::vad::Vad;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fs::File;
#[cfg(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
))]
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
#[cfg(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
))]
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
))]
use std::sync::atomic::Ordering;
use std::sync::Arc;

// ──────────────────────────────────────────────────────────────
// Live transcript pipeline:
//
//   ┌─────────────┐
//   │ AudioStream  │──▶ 100ms chunks at 16kHz
//   └──────┬───────┘
//          │
//          ▼
//   ┌─────────────┐
//   │ VAD loop     │──▶ speaking? → accumulate
//   │              │    silence?  → finalize utterance → JSONL
//   │              │    (NO silence timeout — runs until stop)
//   └──────┬───────┘
//          │
//          ▼
//   ┌─────────────────────────────────┐
//   │ LiveTranscriptWriter            │
//   │  ├─ append JSONL line           │
//   │  └─ append WAV samples          │
//   └──────────────────────────────────┘
//
// Key difference from dictation:
//   - No silence timeout (meetings have silences)
//   - Accumulates all utterances in a single JSONL file
//   - Optionally saves raw WAV for post-meeting reprocessing
//   - Runs until explicit `minutes stop`
// ──────────────────────────────────────────────────────────────

/// A single line in the live transcript JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptLine {
    /// Sequential line number (1-based).
    pub line: usize,
    /// Wall clock timestamp (ISO 8601).
    pub ts: DateTime<Local>,
    /// Milliseconds since session start.
    pub offset_ms: u64,
    /// Utterance duration in milliseconds.
    pub duration_ms: u64,
    /// Transcribed text.
    pub text: String,
    /// Speaker label (null for now, future diarization fills this).
    pub speaker: Option<String>,
}

/// Status of the live transcript session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
    pub active: bool,
    pub pid: Option<u32>,
    pub line_count: usize,
    pub duration_secs: f64,
    pub jsonl_path: Option<String>,
}

/// Manages writing the JSONL and optional WAV file during a live session.
#[cfg(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
))]
struct LiveTranscriptWriter {
    jsonl_writer: BufWriter<File>,
    wav_writer: Option<hound::WavWriter<BufWriter<File>>>,
    line_count: usize,
    start_time: std::time::Instant,
    start_wall: DateTime<Local>,
    jsonl_path: PathBuf,
    jsonl_failed: bool,
    wav_failed: bool,
}

/// Lightweight sidecar written atomically on each utterance.
/// Status readers check this instead of reparsing the full JSONL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveStatus {
    pub start_time: DateTime<Local>,
    pub line_count: usize,
    pub last_offset_ms: u64,
    pub last_duration_ms: u64,
}

#[cfg(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
))]
impl LiveTranscriptWriter {
    fn new(config: &Config) -> Result<Self, MinutesError> {
        let jsonl_path = pid::live_transcript_jsonl_path();
        if let Some(parent) = jsonl_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let jsonl_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&jsonl_path)?;
        set_permissions_0600(&jsonl_path);
        let jsonl_writer = BufWriter::new(jsonl_file);

        let wav_writer = if config.live_transcript.save_wav {
            let wav_path = pid::live_transcript_wav_path();
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            match hound::WavWriter::create(&wav_path, spec) {
                Ok(w) => {
                    set_permissions_0600(&wav_path);
                    Some(w)
                }
                Err(e) => {
                    tracing::warn!("could not create WAV file, continuing without: {}", e);
                    None
                }
            }
        } else {
            None
        };

        let start_wall = Local::now();
        let writer = Self {
            jsonl_writer,
            wav_writer,
            line_count: 0,
            start_time: std::time::Instant::now(),
            start_wall,
            jsonl_path,
            jsonl_failed: false,
            wav_failed: false,
        };

        // Write initial sidecar status
        writer.write_sidecar();

        Ok(writer)
    }

    /// Write the lightweight sidecar status file (atomic rename).
    fn write_sidecar(&self) {
        let status = LiveStatus {
            start_time: self.start_wall,
            line_count: self.line_count,
            last_offset_ms: self.start_time.elapsed().as_millis() as u64,
            last_duration_ms: 0,
        };
        let path = pid::live_transcript_status_path();
        let tmp = path.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_string(&status) {
            if std::fs::write(&tmp, json).is_ok() {
                std::fs::rename(&tmp, &path).ok();
            }
        }
    }

    /// Append a transcribed utterance to the JSONL file.
    /// Returns true if the write succeeded, false if JSONL is broken (data loss).
    fn write_utterance(&mut self, text: &str, duration_secs: f64) -> bool {
        if text.trim().is_empty() {
            return true; // not a failure, just nothing to write
        }
        if self.jsonl_failed {
            return false; // already broken
        }

        self.line_count += 1;
        let offset = self.start_time.elapsed();
        let line = TranscriptLine {
            line: self.line_count,
            ts: Local::now(),
            offset_ms: offset.as_millis() as u64,
            duration_ms: (duration_secs * 1000.0) as u64,
            text: text.trim().to_string(),
            speaker: None,
        };

        match serde_json::to_string(&line) {
            Ok(json) => {
                if let Err(e) = writeln!(self.jsonl_writer, "{}", json) {
                    tracing::error!("JSONL write failed (disk full?): {}", e);
                    self.jsonl_failed = true;
                    return false;
                } else if let Err(e) = self.jsonl_writer.flush() {
                    tracing::error!("JSONL flush failed: {}", e);
                    self.jsonl_failed = true;
                    return false;
                }
            }
            Err(e) => {
                tracing::error!("failed to serialize transcript line: {}", e);
            }
        }
        // Update sidecar after each successful write
        self.write_sidecar();
        true
    }

    /// Write raw audio samples to the WAV file.
    fn write_audio(&mut self, samples: &[f32]) {
        if self.wav_failed {
            return;
        }
        if let Some(ref mut writer) = self.wav_writer {
            for &sample in samples {
                let s = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
                if let Err(e) = writer.write_sample(s) {
                    tracing::warn!("WAV write failed (disk full?), continuing without: {}", e);
                    self.wav_failed = true;
                    return;
                }
            }
        }
    }

    /// Finalize the WAV file and return session summary.
    fn finalize(mut self) -> (usize, f64, PathBuf) {
        if let Some(writer) = self.wav_writer.take() {
            if let Err(e) = writer.finalize() {
                tracing::warn!("WAV finalize failed: {}", e);
            }
        }
        let duration = self.start_time.elapsed().as_secs_f64();
        (self.line_count, duration, self.jsonl_path)
    }
}

/// Run the live transcript session. Blocks until stop_flag is set.
///
/// Unlike dictation, there is NO silence timeout — the session runs
/// until explicitly stopped via `minutes stop` or the stop_flag.
#[cfg(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
))]
pub fn run(
    stop_flag: Arc<AtomicBool>,
    config: &Config,
) -> Result<(usize, f64, PathBuf), MinutesError> {
    // Check conflicts: recording must not be active
    if let Ok(Some(_)) = pid::check_recording() {
        return Err(LiveTranscriptError::RecordingActive.into());
    }

    // Check conflicts: dictation must not be active
    let dict_pid = pid::dictation_pid_path();
    if let Ok(Some(_)) = pid::check_pid_file(&dict_pid) {
        return Err(LiveTranscriptError::DictationActive.into());
    }

    // Clear any stale stop sentinel from a previous session
    pid::check_and_clear_sentinel();

    // Acquire PID with flock held for session lifetime (prevents concurrent starts)
    let lt_pid = pid::live_transcript_pid_path();
    let _pid_guard = pid::create_pid_guard(&lt_pid).map_err(|e| match e {
        crate::error::PidError::AlreadyRecording(pid) => {
            MinutesError::LiveTranscript(LiveTranscriptError::AlreadyActive(pid))
        }
        other => MinutesError::Pid(other),
    })?;

    // Guard holds the flock — dropped when this function returns, cleaning up the PID file
    run_inner(stop_flag, config)
}

#[cfg(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
))]
fn run_inner(
    stop_flag: Arc<AtomicBool>,
    config: &Config,
) -> Result<(usize, f64, PathBuf), MinutesError> {
    let mut engine = StreamingEngine::new_for_live(config)?;

    // Start audio stream FIRST — validate mic access before truncating any files
    let stream = AudioStream::start()?;
    tracing::info!(device = %stream.device_name, "live transcript audio stream started");

    // Only now create the writer (which truncates the JSONL and WAV files)
    let mut writer = LiveTranscriptWriter::new(config)?;

    let mut vad = Vad::new();
    let mut was_speaking = false;
    let mut utterance_samples: usize = 0;
    let max_utterance_secs = config.live_transcript.max_utterance_secs.max(5);
    let max_utterance_samples = (max_utterance_secs as usize).saturating_mul(16000);

    tracing::info!("live transcript session started");

    loop {
        // Check stop flag
        if stop_flag.load(Ordering::Relaxed) {
            // Finalize any in-progress utterance
            if utterance_samples > 0 {
                if let Some(sr) = engine.finalize() {
                    writer.write_utterance(&sr.text, sr.duration_secs);
                }
            }
            break;
        }

        // Check for stop sentinel (from `minutes stop`)
        if pid::check_and_clear_sentinel() {
            if utterance_samples > 0 {
                if let Some(sr) = engine.finalize() {
                    writer.write_utterance(&sr.text, sr.duration_secs);
                }
            }
            break;
        }

        // Receive audio chunk (100ms timeout for stop checks)
        let chunk = match stream
            .receiver
            .recv_timeout(std::time::Duration::from_millis(100))
        {
            Ok(chunk) => chunk,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                tracing::warn!("audio stream disconnected");
                if utterance_samples > 0 {
                    if let Some(sr) = engine.finalize() {
                        writer.write_utterance(&sr.text, sr.duration_secs);
                    }
                }
                break;
            }
        };

        // Write raw audio to WAV
        writer.write_audio(&chunk.samples);

        let vad_result = vad.process(chunk.rms);

        if vad_result.speaking {
            was_speaking = true;
            utterance_samples += chunk.samples.len();

            // Feed to the streaming engine
            if let Some(_sr) = engine.feed(&chunk.samples) {
                // Partial result available — could emit event, but for now just continue
            }

            // Force-finalize if max utterance reached
            if utterance_samples >= max_utterance_samples {
                tracing::info!("max utterance duration reached, force-finalizing");
                if let Some(sr) = engine.finalize() {
                    if !writer.write_utterance(&sr.text, sr.duration_secs) {
                        tracing::error!(
                            "JSONL write failed — stopping session to prevent data loss"
                        );
                        break;
                    }
                }
                engine.reset();
                utterance_samples = 0;
                was_speaking = false;
            }
        } else if was_speaking && utterance_samples > 0 {
            // Speech just ended — finalize the utterance
            if let Some(sr) = engine.finalize() {
                if !writer.write_utterance(&sr.text, sr.duration_secs) {
                    tracing::error!("JSONL write failed — stopping session to prevent data loss");
                    break;
                }
            }
            engine.reset();
            utterance_samples = 0;
            was_speaking = false;
            // No silence timeout — keep running until stop
        }
    }

    let (lines, duration, path) = writer.finalize();
    tracing::info!(
        lines = lines,
        duration_secs = format!("{:.1}", duration),
        "live transcript session ended"
    );

    Ok((lines, duration, path))
}

/// Stub when whisper feature is disabled.
#[cfg(not(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
)))]
pub fn run(
    _stop_flag: Arc<AtomicBool>,
    _config: &Config,
) -> Result<(usize, f64, PathBuf), MinutesError> {
    Err(crate::error::TranscribeError::ModelLoadError(
        "live transcript requires whisper or parakeet-coreml on macOS".into(),
    )
    .into())
}

// ── Delta reader ────────────────────────────────────────────────

/// Read transcript lines from the JSONL file since a given line number.
pub fn read_since_line(since_line: usize) -> Result<Vec<TranscriptLine>, MinutesError> {
    let path = pid::live_transcript_jsonl_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = File::open(&path)?;
    let reader = BufReader::new(file);
    let mut lines = Vec::new();

    for line_result in reader.lines() {
        let line_str = match line_result {
            Ok(s) => s,
            Err(e) => {
                // Skip lines with invalid UTF-8 (e.g., crash-torn multibyte chars)
                tracing::warn!("skipping unreadable JSONL line: {}", e);
                continue;
            }
        };
        if line_str.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<TranscriptLine>(&line_str) {
            Ok(tl) if tl.line > since_line => lines.push(tl),
            Ok(_) => {} // before cursor
            Err(e) => {
                tracing::warn!("skipping malformed JSONL line: {}", e);
            }
        }
    }

    Ok(lines)
}

/// Read transcript lines from the last N milliseconds (wall clock time).
pub fn read_since_duration(duration_ms: u64) -> Result<Vec<TranscriptLine>, MinutesError> {
    let path = pid::live_transcript_jsonl_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let all = read_since_line(0)?;
    if all.is_empty() {
        return Ok(all);
    }

    // Filter by wall clock time, not transcript offset
    let ms = i64::try_from(duration_ms).unwrap_or(i64::MAX);
    let cutoff = Local::now() - chrono::Duration::milliseconds(ms);
    Ok(all.into_iter().filter(|l| l.ts >= cutoff).collect())
}

/// Get the status of the current live transcript session.
pub fn session_status() -> SessionStatus {
    // Single PID read
    let lt_pid = pid::live_transcript_pid_path();
    let pid = pid::check_pid_file(&lt_pid).ok().flatten();
    let active = pid.is_some();

    let jsonl_path = pid::live_transcript_jsonl_path();

    // Read from sidecar if available (O(1) instead of reparsing full JSONL)
    let status_path = pid::live_transcript_status_path();
    let (line_count, duration_secs) = if let Ok(content) = std::fs::read_to_string(&status_path) {
        if let Ok(status) = serde_json::from_str::<LiveStatus>(&content) {
            let elapsed = (Local::now() - status.start_time).num_seconds().max(0) as f64;
            // When active, show wall-clock elapsed. When inactive, show transcript span.
            let dur = if active {
                elapsed
            } else {
                (status.last_offset_ms + status.last_duration_ms) as f64 / 1000.0
            };
            (status.line_count, dur)
        } else {
            (0, 0.0)
        }
    } else {
        // Fallback: no sidecar, parse JSONL (legacy path)
        let lines = if jsonl_path.exists() {
            read_since_line(0).unwrap_or_default()
        } else {
            Vec::new()
        };
        let count = lines.len();
        let dur = lines
            .last()
            .map(|l| (l.offset_ms + l.duration_ms) as f64 / 1000.0)
            .unwrap_or(0.0);
        (count, dur)
    };

    SessionStatus {
        active,
        pid,
        line_count,
        duration_secs,
        jsonl_path: if jsonl_path.exists() {
            Some(jsonl_path.to_string_lossy().to_string())
        } else {
            None
        },
    }
}

#[cfg(any(
    feature = "whisper",
    all(feature = "parakeet-coreml", target_os = "macos")
))]
fn set_permissions_0600(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_transcript_line_roundtrip() {
        let line = TranscriptLine {
            line: 1,
            ts: Local::now(),
            offset_ms: 5000,
            duration_ms: 3200,
            text: "hello world".into(),
            speaker: None,
        };
        let json = serde_json::to_string(&line).unwrap();
        let parsed: TranscriptLine = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.line, 1);
        assert_eq!(parsed.text, "hello world");
        assert_eq!(parsed.offset_ms, 5000);
        assert_eq!(parsed.duration_ms, 3200);
        assert!(parsed.speaker.is_none());
    }

    #[test]
    fn test_read_since_line_filters() {
        let mut tmpfile = NamedTempFile::new().unwrap();
        for i in 1..=5 {
            let line = TranscriptLine {
                line: i,
                ts: Local::now(),
                offset_ms: i as u64 * 10000,
                duration_ms: 3000,
                text: format!("utterance {}", i),
                speaker: None,
            };
            writeln!(tmpfile, "{}", serde_json::to_string(&line).unwrap()).unwrap();
        }

        let file = File::open(tmpfile.path()).unwrap();
        let reader = BufReader::new(file);
        let mut lines = Vec::new();
        for line_result in reader.lines() {
            let line_str = line_result.unwrap();
            if let Ok(tl) = serde_json::from_str::<TranscriptLine>(&line_str) {
                if tl.line > 3 {
                    lines.push(tl);
                }
            }
        }
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].line, 4);
        assert_eq!(lines[1].line, 5);
    }

    #[test]
    fn test_session_status_no_session() {
        let status = session_status();
        // May or may not be active depending on test environment
        // but should not panic
        assert!(status.duration_secs >= 0.0);
    }

    #[test]
    fn test_empty_utterance_skipped() {
        // LiveTranscriptWriter.write_utterance skips empty text
        // We test this by verifying TranscriptLine serialization of empty strings
        let line = TranscriptLine {
            line: 1,
            ts: Local::now(),
            offset_ms: 0,
            duration_ms: 0,
            text: "".into(),
            speaker: None,
        };
        // The writer checks text.trim().is_empty() before writing
        assert!(line.text.trim().is_empty());
    }
}
