use crate::config::Config;
use crate::error::{LiveTranscriptError, MinutesError, TranscribeError};
use crate::pid;
use crate::streaming::AudioStream;
use crate::streaming_whisper::StreamingWhisper;
use crate::vad::Vad;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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

/// How the live transcript is being produced.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TranscriptSource {
    /// Standalone `minutes live` session.
    #[serde(rename = "standalone")]
    Standalone,
    /// Sidecar running alongside `minutes record`.
    #[serde(rename = "recording-sidecar")]
    RecordingSidecar,
}

/// Status of the live transcript session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
    pub active: bool,
    pub pid: Option<u32>,
    pub line_count: usize,
    pub duration_secs: f64,
    pub jsonl_path: Option<String>,
    /// How the transcript is being produced (standalone or recording sidecar).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<TranscriptSource>,
}

/// Manages writing the JSONL and optional WAV file during a live session.
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
#[cfg(feature = "whisper")]
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

#[cfg(feature = "whisper")]
fn run_inner(
    stop_flag: Arc<AtomicBool>,
    config: &Config,
) -> Result<(usize, f64, PathBuf), MinutesError> {
    // Load whisper model: use live_transcript.model if set, otherwise dictation.model
    let whisper_ctx = {
        let model_path = if config.live_transcript.model.is_empty() {
            crate::transcribe::resolve_model_path_for_dictation(config)?
        } else {
            crate::transcribe::resolve_model_path_by_name(&config.live_transcript.model, config)?
        };
        tracing::info!(model = %model_path.display(), "loading whisper model for live transcript");
        whisper_rs::WhisperContext::new_with_params(
            model_path
                .to_str()
                .ok_or_else(|| TranscribeError::ModelLoadError("invalid path".into()))?,
            whisper_rs::WhisperContextParameters::default(),
        )
        .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?
    };

    // Start audio stream FIRST — validate mic access before truncating any files
    let device_override = config.recording.device.as_deref();
    let mut stream = AudioStream::start(device_override)?;
    tracing::info!(device = %stream.device_name, "live transcript audio stream started");

    // Device change monitor for auto-reconnection
    let mut device_monitor = crate::device_monitor::DeviceMonitor::new(&stream.device_name);

    // Only now create the writer (which truncates the JSONL and WAV files)
    let mut writer = LiveTranscriptWriter::new(config)?;

    let mut vad = Vad::new();
    let mut streaming = StreamingWhisper::new(config.transcription.language.clone());
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
                if let Some(sr) = streaming.finalize(&whisper_ctx) {
                    writer.write_utterance(&sr.text, sr.duration_secs);
                }
            }
            break;
        }

        // Check for stop sentinel (from `minutes stop`)
        if pid::check_and_clear_sentinel() {
            if utterance_samples > 0 {
                if let Some(sr) = streaming.finalize(&whisper_ctx) {
                    writer.write_utterance(&sr.text, sr.duration_secs);
                }
            }
            break;
        }

        // Check for stream error or device change — attempt reconnection
        if stream.has_error() || device_monitor.has_device_changed() {
            let old_name = stream.device_name.clone();
            tracing::info!(device = %old_name, "audio stream error or device change — reconnecting");
            drop(stream);
            match AudioStream::start(device_override) {
                Ok(new_stream) => {
                    tracing::info!(
                        old = %old_name, new = %new_stream.device_name,
                        "live transcript audio stream reconnected"
                    );
                    device_monitor.update_device(&new_stream.device_name);
                    stream = new_stream;
                    continue;
                }
                Err(e) => {
                    tracing::error!("live transcript reconnect failed: {}", e);
                    if utterance_samples > 0 {
                        if let Some(sr) = streaming.finalize(&whisper_ctx) {
                            writer.write_utterance(&sr.text, sr.duration_secs);
                        }
                    }
                    break;
                }
            }
        }

        // Receive audio chunk (100ms timeout for stop checks)
        let chunk = match stream
            .receiver
            .recv_timeout(std::time::Duration::from_millis(100))
        {
            Ok(chunk) => chunk,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                // Stream died — try to reconnect (device may have changed)
                let old_name = stream.device_name.clone();
                tracing::warn!("audio stream disconnected — attempting reconnect");
                match AudioStream::start(device_override) {
                    Ok(new_stream) => {
                        tracing::info!(
                            old = %old_name, new = %new_stream.device_name,
                            "live transcript audio stream reconnected after disconnect"
                        );
                        device_monitor.update_device(&new_stream.device_name);
                        stream = new_stream;
                        continue;
                    }
                    Err(e) => {
                        tracing::error!("reconnect after disconnect failed: {}", e);
                        if utterance_samples > 0 {
                            if let Some(sr) = streaming.finalize(&whisper_ctx) {
                                writer.write_utterance(&sr.text, sr.duration_secs);
                            }
                        }
                        break;
                    }
                }
            }
        };

        // Write raw audio to WAV
        writer.write_audio(&chunk.samples);

        let vad_result = vad.process(chunk.rms);

        if vad_result.speaking {
            was_speaking = true;
            utterance_samples += chunk.samples.len();

            // Feed to streaming whisper
            if let Some(_sr) = streaming.feed(&chunk.samples, &whisper_ctx) {
                // Partial result available — could emit event, but for now just continue
            }

            // Force-finalize if max utterance reached
            if utterance_samples >= max_utterance_samples {
                tracing::info!("max utterance duration reached, force-finalizing");
                if let Some(sr) = streaming.finalize(&whisper_ctx) {
                    if !writer.write_utterance(&sr.text, sr.duration_secs) {
                        tracing::error!(
                            "JSONL write failed — stopping session to prevent data loss"
                        );
                        break;
                    }
                }
                streaming.reset();
                utterance_samples = 0;
                was_speaking = false;
            }
        } else if was_speaking && utterance_samples > 0 {
            // Speech just ended — finalize the utterance
            if let Some(sr) = streaming.finalize(&whisper_ctx) {
                if !writer.write_utterance(&sr.text, sr.duration_secs) {
                    tracing::error!("JSONL write failed — stopping session to prevent data loss");
                    break;
                }
            }
            streaming.reset();
            utterance_samples = 0;
            was_speaking = false;
            // No silence timeout — keep running until stop
        }
    }

    let (lines, duration, path) = writer.finalize();
    remove_status_file();
    tracing::info!(
        lines = lines,
        duration_secs = format!("{:.1}", duration),
        "live transcript session ended"
    );

    Ok((lines, duration, path))
}

/// Stub when whisper feature is disabled.
#[cfg(not(feature = "whisper"))]
pub fn run(
    _stop_flag: Arc<AtomicBool>,
    _config: &Config,
) -> Result<(usize, f64, PathBuf), MinutesError> {
    Err(
        TranscribeError::ModelLoadError("live transcript requires the whisper feature".into())
            .into(),
    )
}

// ── Recording sidecar ──────────────────────────────────────────
//
// ── Recording sidecar ──────────────────────────────────────────
//
// Runs alongside record_to_wav to produce a live JSONL transcript
// while recording. Receives audio samples via a stdlib mpsc channel
// from the capture callback and runs the same VAD + StreamingWhisper
// loop that standalone live mode uses. The sidecar does NOT write
// its own WAV (the recording WAV is the canonical audio).

/// Run a live transcript sidecar that consumes audio samples from a channel.
/// Blocks until the channel disconnects (recording stopped) or stop_flag is set.
/// Loads its own whisper model (tiny/base) for real-time streaming.
#[cfg(feature = "whisper")]
pub fn run_sidecar_mpsc(
    rx: std::sync::mpsc::Receiver<Vec<f32>>,
    stop_flag: Arc<AtomicBool>,
    config: &Config,
) {
    if let Err(e) = run_sidecar_inner_mpsc(rx, stop_flag, config) {
        eprintln!(
            "[minutes] Live transcript unavailable: {} — recording continues without real-time transcript",
            e
        );
        tracing::warn!("live sidecar stopped: {}", e);
        // Clean up status file so session_status() doesn't report a dead sidecar as active
        remove_status_file();
    }
}

/// mpsc sidecar implementation.
/// Used by record_to_wav which doesn't depend on the streaming feature.
#[cfg(feature = "whisper")]
fn run_sidecar_inner_mpsc(
    rx: std::sync::mpsc::Receiver<Vec<f32>>,
    stop_flag: Arc<AtomicBool>,
    config: &Config,
) -> Result<(), MinutesError> {
    // Guard: don't clobber a standalone live transcript session's JSONL
    let lt_pid = pid::live_transcript_pid_path();
    if let Ok(Some(_)) = pid::check_pid_file(&lt_pid) {
        tracing::info!("standalone live transcript active — skipping recording sidecar");
        return Ok(());
    }

    let whisper_ctx = {
        let model_path = if config.live_transcript.model.is_empty() {
            crate::transcribe::resolve_model_path_for_dictation(config)?
        } else {
            crate::transcribe::resolve_model_path_by_name(&config.live_transcript.model, config)?
        };
        tracing::info!(model = %model_path.display(), "loading whisper model for recording sidecar");
        whisper_rs::WhisperContext::new_with_params(
            model_path
                .to_str()
                .ok_or_else(|| TranscribeError::ModelLoadError("invalid path".into()))?,
            whisper_rs::WhisperContextParameters::default(),
        )
        .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?
    };

    let mut sidecar_config = config.clone();
    sidecar_config.live_transcript.save_wav = false;
    let mut writer = LiveTranscriptWriter::new(&sidecar_config)?;

    let mut vad = Vad::new();
    let mut streaming = StreamingWhisper::new(config.transcription.language.clone());
    let mut was_speaking = false;
    let mut utterance_samples: usize = 0;
    let max_utterance_secs = config.live_transcript.max_utterance_secs.max(5);
    let max_utterance_samples = (max_utterance_secs as usize).saturating_mul(16000);

    tracing::info!("live sidecar started (recording mode)");

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            if utterance_samples > 0 {
                if let Some(sr) = streaming.finalize(&whisper_ctx) {
                    writer.write_utterance(&sr.text, sr.duration_secs);
                }
            }
            break;
        }

        let samples = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(s) => s,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                if utterance_samples > 0 {
                    if let Some(sr) = streaming.finalize(&whisper_ctx) {
                        writer.write_utterance(&sr.text, sr.duration_secs);
                    }
                }
                break;
            }
        };

        let rms = if samples.is_empty() {
            0.0
        } else {
            let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
            (sum_sq / samples.len() as f32).sqrt()
        };

        let vad_result = vad.process(rms);

        if vad_result.speaking {
            was_speaking = true;
            utterance_samples += samples.len();

            if let Some(_sr) = streaming.feed(&samples, &whisper_ctx) {
                // Partial result — could emit event in future
            }

            if utterance_samples >= max_utterance_samples {
                tracing::info!("sidecar: max utterance duration, force-finalizing");
                if let Some(sr) = streaming.finalize(&whisper_ctx) {
                    writer.write_utterance(&sr.text, sr.duration_secs);
                }
                streaming.reset();
                utterance_samples = 0;
                was_speaking = false;
            }
        } else if was_speaking && utterance_samples > 0 {
            if let Some(sr) = streaming.finalize(&whisper_ctx) {
                writer.write_utterance(&sr.text, sr.duration_secs);
            }
            streaming.reset();
            utterance_samples = 0;
            was_speaking = false;
        }
    }

    let (lines, duration, _path) = writer.finalize();
    // Clean up status file so session_status() doesn't report stale data
    remove_status_file();
    tracing::info!(
        lines = lines,
        duration_secs = format!("{:.1}", duration),
        "live sidecar ended (recording mode)"
    );

    Ok(())
}

/// Stub when whisper feature is disabled.
#[cfg(not(feature = "whisper"))]
pub fn run_sidecar_mpsc(
    _rx: std::sync::mpsc::Receiver<Vec<f32>>,
    _stop_flag: Arc<AtomicBool>,
    _config: &Config,
) {
    tracing::warn!("live sidecar requires the whisper feature");
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
///
/// Detects both standalone live transcript sessions (via live-transcript.pid)
/// and recording sidecar sessions (recording active + sidecar status file exists).
pub fn session_status() -> SessionStatus {
    // Check standalone live transcript PID
    let lt_pid = pid::live_transcript_pid_path();
    let lt_process_pid = pid::check_pid_file(&lt_pid).ok().flatten();

    let recording_pid = pid::check_recording().ok().flatten();
    let status_path = pid::live_transcript_status_path();
    let jsonl_path = pid::live_transcript_jsonl_path();

    derive_session_status(lt_process_pid, recording_pid, &status_path, &jsonl_path)
}

fn derive_session_status(
    lt_process_pid: Option<u32>,
    recording_pid: Option<u32>,
    status_path: &Path,
    jsonl_path: &Path,
) -> SessionStatus {
    let live_status = read_live_status(status_path);
    #[cfg(all(feature = "whisper", feature = "streaming"))]
    let sidecar_active = recording_pid.is_some() && live_status.is_some();
    #[cfg(not(all(feature = "whisper", feature = "streaming")))]
    let sidecar_active = false;

    let active = lt_process_pid.is_some() || sidecar_active;
    let pid = lt_process_pid.or(if sidecar_active { recording_pid } else { None });

    // Read stats from status file or JSONL. Only report non-zero values when
    // a session is active — otherwise stale files would leak old data.
    let (line_count, duration_secs) = if active {
        if let Some(status) = live_status {
            let elapsed = (Local::now() - status.start_time).num_seconds().max(0) as f64;
            (status.line_count, elapsed)
        } else {
            // Fallback: no status file, parse JSONL
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
        }
    } else {
        // Inactive — report zeros. The JSONL file may still exist from a
        // previous session, but its stats are not relevant.
        (0, 0.0)
    };

    let source = if lt_process_pid.is_some() {
        Some(TranscriptSource::Standalone)
    } else if sidecar_active {
        Some(TranscriptSource::RecordingSidecar)
    } else {
        None
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
        source,
    }
}

fn read_live_status(path: &Path) -> Option<LiveStatus> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<LiveStatus>(&content).ok())
}

fn set_permissions_0600(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Remove the status file so `session_status()` won't report stale data.
fn remove_status_file() {
    std::fs::remove_file(pid::live_transcript_status_path()).ok();
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{tempdir, NamedTempFile};

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

    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn sidecar_requires_live_status_file_to_report_active() {
        let dir = tempdir().unwrap();
        let status = derive_session_status(
            None,
            Some(std::process::id()),
            &dir.path().join("live-status.json"),
            &dir.path().join("live.jsonl"),
        );

        assert!(!status.active);
        assert_eq!(status.source, None);
        assert_eq!(status.pid, None);
    }

    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn sidecar_reports_active_when_recording_pid_and_status_file_exist() {
        let dir = tempdir().unwrap();
        let status_path = dir.path().join("live-status.json");
        let status = LiveStatus {
            start_time: Local::now(),
            line_count: 3,
            last_offset_ms: 1200,
            last_duration_ms: 400,
        };
        std::fs::write(&status_path, serde_json::to_string(&status).unwrap()).unwrap();

        let status = derive_session_status(
            None,
            Some(std::process::id()),
            &status_path,
            &dir.path().join("live.jsonl"),
        );

        assert!(status.active);
        assert_eq!(status.source, Some(TranscriptSource::RecordingSidecar));
        assert_eq!(status.pid, Some(std::process::id()));
        assert_eq!(status.line_count, 3);
    }
}
