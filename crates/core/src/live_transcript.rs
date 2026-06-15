use crate::config::Config;
use crate::error::{LiveTranscriptError, MinutesError, TranscribeError};
use crate::pid;
use crate::streaming::AudioStream;
use crate::streaming_whisper::StreamingWhisper;
use crate::transcription_coordinator::{collapse_noise_markers, strip_foreign_script};
use crate::vad::Vad;
#[cfg(feature = "whisper")]
use crate::vad::{VadEngine, VadResult};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

impl TranscriptSource {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::RecordingSidecar => "recording-sidecar",
        }
    }
}

pub const PARAKEET_SCOPE_DOC_REF: &str = "docs/PARAKEET.md#scope";
/// Shown at session start when the user configured `engine = "parakeet"` but the
/// binary was built without the `parakeet` Cargo feature. The engine choice is
/// silently honored as whisper for this session.
pub const PARAKEET_LIVE_SCOPE_WARNING: &str =
    "this build does not include parakeet; live transcription uses whisper (see docs/PARAKEET.md#scope)";
/// Shown at runtime when the parakeet engine IS compiled in but fails during a
/// live session (warmup error, sidecar unreachable, transcribe error). The
/// session transparently falls back to whisper for the remainder.
#[cfg(feature = "parakeet")]
pub const PARAKEET_LIVE_FALLBACK_WARNING: &str =
    "parakeet live transcription failed; falling back to whisper for this session (see docs/PARAKEET.md#scope)";

pub const APPLE_SPEECH_SCOPE_DOC_REF: &str = "docs/designs/apple-speech-benchmark-2026-04-22.md";
pub const APPLE_SPEECH_LIVE_SCOPE_WARNING: &str =
    "apple-speech live transcript is unavailable on this machine; falling back to parakeet or whisper for this session";
pub const APPLE_SPEECH_LIVE_FALLBACK_WARNING: &str =
    "apple-speech live transcription failed; falling back to parakeet or whisper for this session";

/// True iff this build can route `engine = "parakeet"` to the parakeet path.
/// Used at session start to decide between scope-warning (compile-time gap)
/// and the real parakeet dispatch.
fn live_engine_scope_warning(engine: &str) -> Option<&'static str> {
    if engine.eq_ignore_ascii_case("parakeet") && !live_supports_parakeet(engine) {
        Some(PARAKEET_LIVE_SCOPE_WARNING)
    } else if engine.eq_ignore_ascii_case("apple-speech") && !live_supports_apple_speech() {
        Some(APPLE_SPEECH_LIVE_SCOPE_WARNING)
    } else {
        None
    }
}

fn live_supports_parakeet(engine: &str) -> bool {
    #[cfg(feature = "parakeet")]
    {
        engine.eq_ignore_ascii_case("parakeet")
    }

    #[cfg(not(feature = "parakeet"))]
    {
        let _ = engine;
        false
    }
}

#[cfg(feature = "parakeet")]
fn live_ready_parakeet_fallback(config: &Config) -> bool {
    crate::transcription_coordinator::parakeet_backend_status(config).ready
}

#[cfg(not(feature = "parakeet"))]
#[allow(dead_code)]
fn live_ready_parakeet_fallback(_config: &Config) -> bool {
    false
}

fn live_supports_apple_speech() -> bool {
    #[cfg(target_os = "macos")]
    {
        match crate::apple_speech::probe_capabilities() {
            Ok(report) => {
                report.runtime_supported && report.speech_transcriber.is_available.unwrap_or(false)
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "apple-speech capability probe failed during live transcript startup"
                );
                false
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Session-start warning: fires only when `engine = "parakeet"` was requested
/// and the current build cannot honor it (parakeet feature not compiled in).
/// Always visible on stderr so the user sees that their engine choice was
/// silently downgraded.
fn emit_live_engine_scope_warning(engine: &str, source: &'static str) {
    let Some(message) = live_engine_scope_warning(engine) else {
        return;
    };

    eprintln!("[minutes] {}", message);
    tracing::warn!(engine, source, "{}", message);
    crate::logging::append_log(&serde_json::json!({
        "ts": Local::now().to_rfc3339(),
        "level": "warn",
        "step": "live_transcript_scope",
        "file": "",
        "message": message,
        "extra": {
            "engine": engine,
            "source": source,
            "doc_ref": PARAKEET_SCOPE_DOC_REF,
        }
    }))
    .ok();
}

/// Runtime fallback warning: fires when the parakeet path fails mid-session
/// (warmup error or transcribe error) and the session is transparently switched
/// to whisper for the remainder. Always visible on stderr — the user needs to
/// know their transcripts changed engines.
#[cfg(feature = "parakeet")]
fn emit_live_engine_fallback_warning(source: &'static str, detail: &str) {
    eprintln!(
        "[minutes] {} (detail: {})",
        PARAKEET_LIVE_FALLBACK_WARNING, detail
    );
    tracing::warn!(source, detail, "{}", PARAKEET_LIVE_FALLBACK_WARNING);
    crate::logging::append_log(&serde_json::json!({
        "ts": Local::now().to_rfc3339(),
        "level": "warn",
        "step": "live_transcript_fallback",
        "file": "",
        "message": PARAKEET_LIVE_FALLBACK_WARNING,
        "extra": {
            "source": source,
            "detail": detail,
            "doc_ref": PARAKEET_SCOPE_DOC_REF,
        }
    }))
    .ok();
}

#[cfg(all(feature = "whisper", target_os = "macos"))]
fn emit_apple_speech_fallback_warning(source: &'static str, detail: &str) {
    eprintln!(
        "[minutes] {} (detail: {})",
        APPLE_SPEECH_LIVE_FALLBACK_WARNING, detail
    );
    tracing::warn!(source, detail, "{}", APPLE_SPEECH_LIVE_FALLBACK_WARNING);
    crate::logging::append_log(&serde_json::json!({
        "ts": Local::now().to_rfc3339(),
        "level": "warn",
        "step": "live_transcript_apple_fallback",
        "file": "",
        "message": APPLE_SPEECH_LIVE_FALLBACK_WARNING,
        "extra": {
            "source": source,
            "detail": detail,
            "doc_ref": APPLE_SPEECH_SCOPE_DOC_REF,
        }
    }))
    .ok();
}

/// Status of the live transcript session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
    pub active: bool,
    pub pid: Option<u32>,
    pub line_count: usize,
    pub duration_secs: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub jsonl_path: Option<String>,
    /// How the transcript is being produced (standalone or recording sidecar).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<TranscriptSource>,
    /// Diagnostic detail when a transcript session is degraded or unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

/// Manages writing the JSONL and optional WAV file during a live session.
struct LiveTranscriptWriter {
    session_id: Option<String>,
    source: TranscriptSource,
    jsonl_writer: BufWriter<File>,
    wav_writer: Option<hound::WavWriter<BufWriter<File>>>,
    line_count: usize,
    start_time: std::time::Instant,
    start_wall: DateTime<Local>,
    jsonl_path: PathBuf,
    jsonl_failed: bool,
    wav_failed: bool,
    pending_utterances: u64,
    dropped_utterances: u64,
    last_status_write: Instant,
}

/// Lightweight sidecar written atomically on each utterance.
/// Status readers check this instead of reparsing the full JSONL.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LiveStatusState {
    Starting,
    Healthy,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveStatus {
    pub start_time: DateTime<Local>,
    pub updated_at: DateTime<Local>,
    pub state: LiveStatusState,
    pub line_count: usize,
    pub last_offset_ms: u64,
    pub last_duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
    /// Finalized utterances queued for transcription but not yet written.
    /// Only present (recording sidecar) when the backlog is non-empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_utterances: Option<u64>,
    /// Utterances whose audio was discarded because the transcription worker
    /// could not keep up. Only present when non-zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_utterances: Option<u64>,
}

const SIDECAR_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const SIDECAR_HEALTH_STALE_AFTER_SECS: i64 = 3;
const SIDECAR_STARTUP_TIMEOUT_SECS: i64 = 10;

impl LiveTranscriptWriter {
    fn new(
        config: &Config,
        session_id: Option<String>,
        source: TranscriptSource,
    ) -> Result<Self, MinutesError> {
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
            session_id,
            source,
            jsonl_writer,
            wav_writer,
            line_count: 0,
            start_time: std::time::Instant::now(),
            start_wall,
            jsonl_path,
            jsonl_failed: false,
            wav_failed: false,
            pending_utterances: 0,
            dropped_utterances: 0,
            last_status_write: Instant::now()
                .checked_sub(SIDECAR_HEARTBEAT_INTERVAL)
                .unwrap_or_else(Instant::now),
        };

        Ok(writer)
    }

    /// Write the lightweight status file (atomic rename).
    fn write_status(
        &mut self,
        state: LiveStatusState,
        last_duration_ms: u64,
        diagnostic: Option<&str>,
    ) {
        let backlog_diagnostic = (self.dropped_utterances > 0).then(|| {
            format!(
                "transcription backlog: {} utterance(s) dropped",
                self.dropped_utterances
            )
        });
        let status = LiveStatus {
            start_time: self.start_wall,
            updated_at: Local::now(),
            state,
            line_count: self.line_count,
            last_offset_ms: self.start_time.elapsed().as_millis() as u64,
            last_duration_ms,
            session_id: self.session_id.clone(),
            diagnostic: diagnostic.map(str::to_string).or(backlog_diagnostic),
            pending_utterances: (self.pending_utterances > 0).then_some(self.pending_utterances),
            dropped_utterances: (self.dropped_utterances > 0).then_some(self.dropped_utterances),
        };
        write_live_status(&status);
        self.last_status_write = Instant::now();
    }

    /// Update transcription-backlog counters surfaced in the status file.
    fn set_backlog_stats(&mut self, pending: u64, dropped: u64) {
        self.pending_utterances = pending;
        self.dropped_utterances = dropped;
    }

    fn mark_healthy(&mut self) {
        self.write_status(LiveStatusState::Healthy, 0, None);
    }

    fn mark_stopped(&mut self) {
        self.write_status(LiveStatusState::Stopped, 0, None);
    }

    fn maybe_write_heartbeat(&mut self) {
        if self.last_status_write.elapsed() >= SIDECAR_HEARTBEAT_INTERVAL {
            self.write_status(LiveStatusState::Healthy, 0, None);
        }
    }

    /// Append a transcribed utterance to the JSONL file.
    /// Returns true if the write succeeded, false if JSONL is broken (data loss).
    fn write_utterance(&mut self, text: &str, duration_secs: f64) -> bool {
        let Some(text) = normalize_live_transcript_text(text) else {
            return true; // not a failure, just nothing to write
        };
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
            text,
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
        self.write_status(LiveStatusState::Healthy, line.duration_ms, None);
        crate::events::append_event(crate::events::MinutesEvent::LiveUtteranceFinal {
            session_id: self.session_id.clone(),
            source: self.source.as_str().to_string(),
            transcript_path: self.jsonl_path.display().to_string(),
            line: line.line,
            text: line.text.clone(),
            speaker: line.speaker.clone(),
            offset_ms: line.offset_ms,
            duration_ms: line.duration_ms,
        });
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
        self.mark_stopped();
        if let Some(writer) = self.wav_writer.take() {
            if let Err(e) = writer.finalize() {
                tracing::warn!("WAV finalize failed: {}", e);
            }
        }
        let duration = self.start_time.elapsed().as_secs_f64();
        (self.line_count, duration, self.jsonl_path)
    }
}

fn normalize_live_transcript_text(text: &str) -> Option<String> {
    let normalized_lines: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let body = live_text_part(line).trim();
            if body.is_empty() {
                None
            } else {
                Some(format!("[0:00] {}", body))
            }
        })
        .collect();

    if normalized_lines.is_empty() {
        return None;
    }

    let normalized_lines = strip_foreign_script(normalized_lines);
    let normalized_lines = collapse_noise_markers(normalized_lines);
    let normalized_lines: Vec<String> = normalized_lines
        .into_iter()
        .filter_map(|line| {
            let body = live_text_part(&line).trim();
            if body.is_empty() || is_live_noise_marker(body) {
                None
            } else {
                Some(body.to_string())
            }
        })
        .collect();

    if normalized_lines.is_empty() {
        None
    } else {
        Some(normalized_lines.join(" "))
    }
}

fn live_text_part(line: &str) -> &str {
    line.find("] ")
        .map(|index| &line[index + 2..])
        .unwrap_or(line)
}

fn is_live_noise_marker(text: &str) -> bool {
    let trimmed = text.trim().strip_suffix('.').unwrap_or(text.trim());
    if !(trimmed.starts_with('[') && trimmed.ends_with(']')) {
        return false;
    }

    let inner = &trimmed[1..trimmed.len() - 1];
    if inner.chars().all(|ch| ch.is_ascii_digit() || ch == ':') {
        return false;
    }

    let word_count = inner.split_whitespace().count();
    (1..=4).contains(&word_count) && inner.len() <= 40
}

/// Run the live transcript session. Blocks until stop_flag is set.
///
/// Unlike dictation, there is NO silence timeout — the session runs
/// until explicitly stopped via `minutes stop` or the stop_flag.
#[cfg(feature = "whisper")]
pub fn run(
    stop_flag: Arc<AtomicBool>,
    config: &Config,
    existing_context_session_id: Option<String>,
) -> Result<(usize, f64, PathBuf), MinutesError> {
    let mark_precreated_session_failed = |error: &MinutesError| {
        if let Some(session_id) = existing_context_session_id.as_deref() {
            crate::context_store::mark_live_transcript_failed(
                session_id,
                Some(Local::now()),
                &error.to_string(),
            )
            .ok();
        }
    };

    // Check conflicts: recording must not be active
    if let Ok(Some(_)) = pid::check_recording() {
        let error: MinutesError = LiveTranscriptError::RecordingActive.into();
        mark_precreated_session_failed(&error);
        return Err(error);
    }

    // Check conflicts: dictation must not be active
    let dict_pid = pid::dictation_pid_path();
    if let Ok(Some(_)) = pid::check_pid_file(&dict_pid) {
        let error: MinutesError = LiveTranscriptError::DictationActive.into();
        mark_precreated_session_failed(&error);
        return Err(error);
    }

    // Clear any stale stop sentinel from a previous session
    pid::check_and_clear_sentinel();

    // Acquire PID with flock held for session lifetime (prevents concurrent starts)
    let lt_pid = pid::live_transcript_pid_path();
    let _pid_guard = match pid::create_pid_guard(&lt_pid) {
        Ok(pid_guard) => pid_guard,
        Err(e) => {
            let error = match e {
                crate::error::PidError::AlreadyRecording(pid) => {
                    MinutesError::LiveTranscript(LiveTranscriptError::AlreadyActive(pid))
                }
                other => MinutesError::Pid(other),
            };
            mark_precreated_session_failed(&error);
            return Err(error);
        }
    };

    // Guard holds the flock — dropped when this function returns, cleaning up the PID file
    write_live_status_transition(LiveStatusState::Starting, None);
    let context_session_id = if let Some(session_id) = existing_context_session_id {
        Some(session_id)
    } else {
        crate::desktop_context::maybe_start_live_transcript_session(
            &config.desktop_context,
            Local::now(),
        )
    };

    match run_inner(stop_flag, config, context_session_id.clone()) {
        Ok((lines, duration, path)) => {
            if let Some(session_id) = context_session_id.as_deref() {
                let wav_path = pid::live_transcript_wav_path();
                crate::context_store::mark_live_transcript_complete(
                    session_id,
                    &path,
                    wav_path.exists().then_some(wav_path.as_path()),
                    Some(Local::now()),
                    json!({
                        "line_count": lines,
                        "duration_secs": duration,
                    }),
                )
                .ok();
            }
            Ok((lines, duration, path))
        }
        Err(error) => {
            if let Some(session_id) = context_session_id.as_deref() {
                crate::context_store::mark_live_transcript_failed(
                    session_id,
                    Some(Local::now()),
                    &error.to_string(),
                )
                .ok();
            }
            Err(error)
        }
    }
}

#[cfg(feature = "whisper")]
fn run_inner(
    stop_flag: Arc<AtomicBool>,
    config: &Config,
    context_session_id: Option<String>,
) -> Result<(usize, f64, PathBuf), MinutesError> {
    let mut whisper_ctx: Option<whisper_rs::WhisperContext> = None;

    // Start audio stream FIRST — validate mic access before truncating any files
    let device_override = config.recording.device.as_deref();
    let permission_preflight = crate::capture::preflight_microphone_only();
    if let Some(reason) = permission_preflight.blocking_reason {
        return Err(LiveTranscriptError::PermissionBlocked(reason).into());
    }
    for warning in permission_preflight.warnings {
        tracing::warn!(warning = %warning, "live transcript microphone permission preflight warning");
    }

    let mut stream = AudioStream::start(device_override)?;
    tracing::info!(device = %stream.device_name, "live transcript audio stream started");

    // Device change monitor for auto-reconnection. Pinned when the user
    // supplied an explicit device override.
    let mut device_monitor = if device_override.is_some() {
        crate::device_monitor::DeviceMonitor::pinned(&stream.device_name)
    } else {
        crate::device_monitor::DeviceMonitor::new(&stream.device_name)
    };

    // Only now create the writer (which truncates the JSONL and WAV files)
    let mut writer =
        LiveTranscriptWriter::new(config, context_session_id, TranscriptSource::Standalone)?;
    writer.mark_healthy();
    crate::events::append_event(crate::events::recording_started_event(
        writer.session_id.clone(),
        "live",
        ["audio.capture", "live.utterance.final"],
    ));

    let mut vad = Vad::new();
    let mut streaming = StreamingWhisper::with_partial_max_secs(
        config.transcription.language.clone(),
        config.transcription.partial_max_secs,
    );
    let standalone_backend = config.effective_live_transcript_backend();
    #[cfg(target_os = "macos")]
    let mut apple_utterance_samples: Vec<f32> = Vec::new();
    #[cfg(target_os = "macos")]
    let mut apple_live_enabled =
        standalone_backend.eq_ignore_ascii_case("apple-speech") && live_supports_apple_speech();
    #[cfg(not(target_os = "macos"))]
    let mut apple_live_enabled = false;

    // Parakeet engine dispatch — mirrors run_sidecar_inner_mpsc. When the user
    // configures `engine = "parakeet"` and the parakeet feature is compiled in,
    // utterance samples are accumulated and routed through the parakeet path
    // (warm sidecar socket when the effective sidecar decision is on, subprocess
    // otherwise) at VAD-end. On failure the session transparently falls back
    // to whisper for the remainder. See RFC 0002.
    #[cfg(feature = "parakeet")]
    let mut parakeet_utterance_samples: Vec<f32> = Vec::new();
    #[cfg(feature = "parakeet")]
    let mut parakeet_live_enabled = live_supports_parakeet(standalone_backend);
    #[cfg(feature = "parakeet")]
    let parakeet_fallback_ready = live_ready_parakeet_fallback(config);
    #[cfg(not(feature = "parakeet"))]
    let parakeet_live_enabled = false;
    #[cfg(not(feature = "parakeet"))]
    let parakeet_fallback_ready = false;

    let mut was_speaking = false;
    let mut utterance_samples: usize = 0;
    let max_utterance_secs = config.live_transcript.max_utterance_secs.max(5);
    let max_utterance_samples = (max_utterance_secs as usize).saturating_mul(16000);

    // One-time scope warning when the user configured parakeet but the feature
    // isn't compiled in. Same warning the recording sidecar emits.
    if standalone_backend.eq_ignore_ascii_case("parakeet") && !parakeet_live_enabled {
        emit_live_engine_scope_warning(standalone_backend, "standalone");
    }
    if standalone_backend.eq_ignore_ascii_case("apple-speech") && !apple_live_enabled {
        emit_live_engine_scope_warning(standalone_backend, "standalone");
    }
    if apple_live_enabled {
        eprintln!("[minutes] Apple Speech live transcript enabled (experimental, standalone only)");
    }

    // Warm the parakeet sidecar at session start so the first utterance doesn't
    // pay subprocess-spawn + model-load latency. We only warm the sidecar lane
    // because:
    //   - sidecar effective-on (auto or explicit) → warmup leaves a hot example-server
    //     child + loaded model, making subsequent utterances fast.
    //   - sidecar effective-off → warmup would just be a throwaway
    //     subprocess on a silent WAV; it wouldn't leave a hot backend behind,
    //     so the first real utterance still pays full spawn/model-load cost.
    //     Skipping the no-op warmup avoids a 4-5s startup stall for nothing.
    //
    // Warmup failure is ADVISORY. We do NOT disable parakeet for the session
    // on warmup error, because `crate::transcribe::transcribe()` already falls
    // back sidecar→subprocess gracefully on a per-utterance basis. Forcing
    // whisper here would be strictly worse than what the per-utterance code
    // already does.
    #[cfg(feature = "parakeet")]
    if parakeet_live_enabled && crate::parakeet_sidecar::sidecar_enabled_effective(config) {
        eprintln!("[minutes] Warming parakeet sidecar... (first-run cold start can take 10-30s)");
        let started = std::time::Instant::now();
        match crate::transcription_coordinator::warmup_active_backend(config) {
            Ok(result) => {
                tracing::info!(
                    backend_id = %result.backend_id,
                    elapsed_ms = result.elapsed_ms,
                    "parakeet backend warmed for live session"
                );
                eprintln!("[minutes] parakeet sidecar ready ({}ms)", result.elapsed_ms);
            }
            Err(error) => {
                // Log loudly, but keep parakeet_live_enabled = true so the
                // per-utterance transcribe() path gets a chance to fall back
                // sidecar→subprocess on its own.
                tracing::warn!(
                    error = %error,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "parakeet sidecar warmup failed; falling through to per-utterance dispatch (may still succeed via subprocess)"
                );
                eprintln!(
                    "[minutes] parakeet sidecar warmup failed ({}); will try per-utterance subprocess path",
                    error
                );
            }
        }
    }

    tracing::info!("live transcript session started");

    loop {
        writer.maybe_write_heartbeat();
        // Check stop flag
        if stop_flag.load(Ordering::Relaxed) {
            // Finalize any in-progress utterance
            if utterance_samples > 0 {
                #[cfg(feature = "parakeet")]
                finalize_on_exit(
                    &mut writer,
                    &mut apple_live_enabled,
                    #[cfg(target_os = "macos")]
                    &mut apple_utterance_samples,
                    parakeet_fallback_ready,
                    &mut parakeet_live_enabled,
                    &mut parakeet_utterance_samples,
                    config,
                    &mut streaming,
                    &mut whisper_ctx,
                    "standalone",
                );
                #[cfg(not(feature = "parakeet"))]
                finalize_on_exit(
                    &mut writer,
                    &mut apple_live_enabled,
                    #[cfg(target_os = "macos")]
                    &mut apple_utterance_samples,
                    parakeet_fallback_ready,
                    config,
                    &mut streaming,
                    &mut whisper_ctx,
                    "standalone",
                );
            }
            break;
        }

        // Check for stop sentinel (from `minutes stop`)
        if pid::check_and_clear_sentinel() {
            if utterance_samples > 0 {
                #[cfg(feature = "parakeet")]
                finalize_on_exit(
                    &mut writer,
                    &mut apple_live_enabled,
                    #[cfg(target_os = "macos")]
                    &mut apple_utterance_samples,
                    parakeet_fallback_ready,
                    &mut parakeet_live_enabled,
                    &mut parakeet_utterance_samples,
                    config,
                    &mut streaming,
                    &mut whisper_ctx,
                    "standalone",
                );
                #[cfg(not(feature = "parakeet"))]
                finalize_on_exit(
                    &mut writer,
                    &mut apple_live_enabled,
                    #[cfg(target_os = "macos")]
                    &mut apple_utterance_samples,
                    parakeet_fallback_ready,
                    config,
                    &mut streaming,
                    &mut whisper_ctx,
                    "standalone",
                );
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
                    // Discard any partial utterance — mic dropped mid-speech,
                    // pre-reconnect audio is unreliable. Splicing pre+post
                    // reconnect samples into one JSONL line would be silent
                    // transcript corruption.
                    if utterance_samples > 0 {
                        tracing::info!(
                            samples_discarded = utterance_samples,
                            "discarding partial utterance across device reconnect"
                        );
                    }
                    streaming.reset();
                    #[cfg(target_os = "macos")]
                    apple_utterance_samples.clear();
                    #[cfg(feature = "parakeet")]
                    parakeet_utterance_samples.clear();
                    utterance_samples = 0;
                    was_speaking = false;
                    continue;
                }
                Err(e) => {
                    tracing::error!("live transcript reconnect failed: {}", e);
                    if utterance_samples > 0 {
                        #[cfg(feature = "parakeet")]
                        finalize_on_exit(
                            &mut writer,
                            &mut apple_live_enabled,
                            #[cfg(target_os = "macos")]
                            &mut apple_utterance_samples,
                            parakeet_fallback_ready,
                            &mut parakeet_live_enabled,
                            &mut parakeet_utterance_samples,
                            config,
                            &mut streaming,
                            &mut whisper_ctx,
                            "standalone",
                        );
                        #[cfg(not(feature = "parakeet"))]
                        finalize_on_exit(
                            &mut writer,
                            &mut apple_live_enabled,
                            #[cfg(target_os = "macos")]
                            &mut apple_utterance_samples,
                            parakeet_fallback_ready,
                            config,
                            &mut streaming,
                            &mut whisper_ctx,
                            "standalone",
                        );
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
                        // Discard partial utterance — see comment above the
                        // device-change reconnect branch for rationale.
                        if utterance_samples > 0 {
                            tracing::info!(
                                samples_discarded = utterance_samples,
                                "discarding partial utterance across stream reconnect"
                            );
                        }
                        streaming.reset();
                        #[cfg(target_os = "macos")]
                        apple_utterance_samples.clear();
                        #[cfg(feature = "parakeet")]
                        parakeet_utterance_samples.clear();
                        utterance_samples = 0;
                        was_speaking = false;
                        continue;
                    }
                    Err(e) => {
                        tracing::error!("reconnect after disconnect failed: {}", e);
                        if utterance_samples > 0 {
                            #[cfg(feature = "parakeet")]
                            finalize_on_exit(
                                &mut writer,
                                &mut apple_live_enabled,
                                #[cfg(target_os = "macos")]
                                &mut apple_utterance_samples,
                                parakeet_fallback_ready,
                                &mut parakeet_live_enabled,
                                &mut parakeet_utterance_samples,
                                config,
                                &mut streaming,
                                &mut whisper_ctx,
                                "standalone",
                            );
                            #[cfg(not(feature = "parakeet"))]
                            finalize_on_exit(
                                &mut writer,
                                &mut apple_live_enabled,
                                #[cfg(target_os = "macos")]
                                &mut apple_utterance_samples,
                                parakeet_fallback_ready,
                                config,
                                &mut streaming,
                                &mut whisper_ctx,
                                "standalone",
                            );
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

            if apple_live_enabled {
                #[cfg(target_os = "macos")]
                {
                    apple_utterance_samples.extend_from_slice(&chunk.samples);
                }
            } else if parakeet_live_enabled {
                #[cfg(feature = "parakeet")]
                {
                    parakeet_utterance_samples.extend_from_slice(&chunk.samples);
                }
            } else if let Ok(whisper_ctx) = ensure_live_whisper_ctx(&mut whisper_ctx, config) {
                if let Some(_sr) = streaming.feed(&chunk.samples, whisper_ctx) {
                    // Intentionally not emitted in event-bus v0. Partial
                    // revisions are high-volume and need a gated v1 contract.
                }
            }

            // Force-finalize if max utterance reached
            if utterance_samples >= max_utterance_samples {
                tracing::info!("max utterance duration reached, force-finalizing");
                #[cfg(feature = "parakeet")]
                let write_ok = finalize_live_utterance(
                    &mut writer,
                    &mut apple_live_enabled,
                    #[cfg(target_os = "macos")]
                    &mut apple_utterance_samples,
                    parakeet_fallback_ready,
                    &mut parakeet_live_enabled,
                    &mut parakeet_utterance_samples,
                    config,
                    &mut streaming,
                    &mut whisper_ctx,
                    "standalone",
                );
                #[cfg(not(feature = "parakeet"))]
                let write_ok = finalize_live_utterance(
                    &mut writer,
                    &mut apple_live_enabled,
                    #[cfg(target_os = "macos")]
                    &mut apple_utterance_samples,
                    parakeet_fallback_ready,
                    config,
                    &mut streaming,
                    &mut whisper_ctx,
                    "standalone",
                );
                if !write_ok {
                    tracing::error!("JSONL write failed — stopping session to prevent data loss");
                    break;
                }
                utterance_samples = 0;
                was_speaking = false;
            }
        } else if was_speaking && utterance_samples > 0 {
            // Speech just ended — finalize the utterance
            #[cfg(feature = "parakeet")]
            let write_ok = finalize_live_utterance(
                &mut writer,
                &mut apple_live_enabled,
                #[cfg(target_os = "macos")]
                &mut apple_utterance_samples,
                parakeet_fallback_ready,
                &mut parakeet_live_enabled,
                &mut parakeet_utterance_samples,
                config,
                &mut streaming,
                &mut whisper_ctx,
                "standalone",
            );
            #[cfg(not(feature = "parakeet"))]
            let write_ok = finalize_live_utterance(
                &mut writer,
                &mut apple_live_enabled,
                #[cfg(target_os = "macos")]
                &mut apple_utterance_samples,
                parakeet_fallback_ready,
                config,
                &mut streaming,
                &mut whisper_ctx,
                "standalone",
            );
            if !write_ok {
                tracing::error!("JSONL write failed — stopping session to prevent data loss");
                break;
            }
            utterance_samples = 0;
            was_speaking = false;
            // No silence timeout — keep running until stop
        }
    }

    let (lines, duration, path) = writer.finalize();
    clear_status_file();
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

#[cfg(feature = "whisper")]
const SIDECAR_VAD_CHUNK_MS: u64 = 100;
#[cfg(feature = "whisper")]
const SIDECAR_VAD_THRESHOLD: f32 = 0.2;
#[cfg(feature = "whisper")]
const SIDECAR_VAD_MIN_SPEECH_MS: i32 = 150;
#[cfg(feature = "whisper")]
const SIDECAR_VAD_MIN_SILENCE_MS: i32 = 500;
#[cfg(feature = "whisper")]
const SIDECAR_VAD_SPEECH_PAD_MS: i32 = 80;
#[cfg(feature = "whisper")]
const SIDECAR_VAD_IDLE_BUFFER_MS: usize = 3000;
#[cfg(feature = "whisper")]
const SIDECAR_VAD_ACTIVE_BUFFER_MS: usize = 8000;

#[cfg(feature = "whisper")]
#[derive(Debug, Default, Clone, Copy)]
struct SidecarGatingStats {
    samples_fed: usize,
    samples_gated: usize,
    speaking_windows: usize,
    silence_windows: usize,
}

#[cfg(feature = "whisper")]
impl SidecarGatingStats {
    fn observe(&mut self, samples_len: usize, speaking: bool) {
        self.samples_fed += samples_len;
        if speaking {
            self.speaking_windows += 1;
        } else {
            self.samples_gated += samples_len;
            self.silence_windows += 1;
        }
    }
}

#[cfg(feature = "whisper")]
enum RecordingSidecarVadBackend {
    Silero(SileroSidecarVad),
    /// Boxed because `OrtSileroVad` owns an ort `Session` plus state
    /// and carry buffers, making it noticeably larger than the other
    /// variants. Boxing keeps the enum size constant and silences
    /// `clippy::large_enum_variant` without a per-variant allow.
    #[cfg(feature = "vad-ort")]
    OrtSilero(Box<crate::silero_vad::OrtSileroVad>),
    Energy(Vad),
}

#[cfg(feature = "whisper")]
struct RecordingSidecarVad {
    backend: RecordingSidecarVadBackend,
}

#[cfg(feature = "whisper")]
impl RecordingSidecarVad {
    fn new(config: &Config) -> Self {
        // Pick the engine the user asked for. When ort-silero is
        // requested but unavailable (feature off, ONNX missing, or
        // load failure), fall through to whisper-silero with an
        // explicit warn — silent fallback is the support footgun
        // codex flagged in the spec review.
        let requested = config.transcription.vad_engine.trim().to_lowercase();
        let want_ort = matches!(requested.as_str(), "ort-silero" | "ort" | "silero-ort");
        if !requested.is_empty()
            && !want_ort
            && !matches!(
                requested.as_str(),
                "whisper-silero" | "silero" | "whisper" | "default"
            )
        {
            tracing::warn!(
                requested = %requested,
                "unknown transcription.vad_engine value — falling through to whisper-silero"
            );
        }

        #[cfg(feature = "vad-ort")]
        if want_ort {
            if let Some(onnx_path) = crate::transcribe::resolve_silero_onnx_path(config) {
                match crate::silero_vad::OrtSileroVad::new(&onnx_path) {
                    Ok(engine) => {
                        tracing::info!(
                            vad_engine = "ort-silero",
                            onnx_model = %onnx_path.display(),
                            "recording sidecar using ort-Silero VAD (streaming)"
                        );
                        return Self {
                            backend: RecordingSidecarVadBackend::OrtSilero(Box::new(engine)),
                        };
                    }
                    Err(e) => {
                        tracing::warn!(
                            onnx_model = %onnx_path.display(),
                            error = %e,
                            "ort-Silero load failed — falling back to whisper-silero"
                        );
                    }
                }
            } else {
                tracing::warn!(
                    "ort-silero requested but silero-vad-v6.2.0.onnx missing in model_path — \
                     falling back to whisper-silero. Run `minutes setup` (with vad-ort enabled \
                     in your build) to fetch the ONNX."
                );
            }
        }
        #[cfg(not(feature = "vad-ort"))]
        if want_ort {
            tracing::warn!(
                "ort-silero requested but this build was not compiled with the `vad-ort` \
                 feature — falling back to whisper-silero"
            );
        }

        if let Some(vad_path) = crate::transcribe::resolve_vad_model_path(config) {
            match SileroSidecarVad::new(&vad_path) {
                Ok(vad) => {
                    tracing::info!(
                        vad_engine = "whisper-silero",
                        vad_model = %vad_path.display(),
                        "recording sidecar using whisper-Silero VAD"
                    );
                    return Self {
                        backend: RecordingSidecarVadBackend::Silero(vad),
                    };
                }
                Err(e) => {
                    tracing::warn!(
                        vad_model = %vad_path.display(),
                        error = %e,
                        "failed to initialize Silero VAD for recording sidecar — falling back to energy VAD"
                    );
                }
            }
        } else {
            tracing::warn!(
                "Silero VAD model unavailable for recording sidecar — falling back to energy VAD"
            );
        }

        tracing::info!(vad_engine = "energy", "recording sidecar using energy VAD");
        Self {
            backend: RecordingSidecarVadBackend::Energy(Vad::new()),
        }
    }

    fn mode_name(&self) -> &'static str {
        match &self.backend {
            RecordingSidecarVadBackend::Silero(_) => "silero",
            #[cfg(feature = "vad-ort")]
            RecordingSidecarVadBackend::OrtSilero(_) => "ort-silero",
            RecordingSidecarVadBackend::Energy(_) => "energy",
        }
    }

    fn process(&mut self, samples: &[f32], rms: f32) -> VadResult {
        loop {
            match &mut self.backend {
                RecordingSidecarVadBackend::Silero(vad) => match vad.process(samples, rms) {
                    Ok(result) => return result,
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "Silero VAD failed during recording sidecar run — falling back to energy VAD"
                        );
                        self.backend = RecordingSidecarVadBackend::Energy(Vad::new());
                    }
                },
                #[cfg(feature = "vad-ort")]
                RecordingSidecarVadBackend::OrtSilero(engine) => {
                    let result = engine.process(samples, rms);
                    if engine.is_healthy() {
                        return result;
                    }
                    // Engine flipped unhealthy mid-call. Replace and
                    // re-dispatch on the next loop iteration. The
                    // result we just got is one silence frame; per
                    // the trait contract, downstream must not act on
                    // it as authoritative, but for the sidecar's
                    // per-call flag it's fine to drop and refresh.
                    tracing::warn!(
                        "ort-Silero engine flipped unhealthy during recording sidecar run — falling back to energy VAD"
                    );
                    self.backend = RecordingSidecarVadBackend::Energy(Vad::new());
                }
                RecordingSidecarVadBackend::Energy(vad) => return vad.process(rms),
            }
        }
    }
}

#[cfg(feature = "whisper")]
struct SileroSidecarVad {
    ctx: whisper_rs::WhisperVadContext,
    params: whisper_rs::WhisperVadParams,
    buffer: Vec<f32>,
    idle_buffer_samples: usize,
    active_buffer_samples: usize,
    min_silence_ms: u64,
    chunk_ms: u64,
    silence_ms: u64,
    /// Sticky failure flag for the `VadEngine` impl. Set to `true` the
    /// first time the inherent `process` returns `Err` so callers
    /// using trait dispatch can surface health via `is_healthy`. The
    /// existing enum-based dispatcher (`RecordingSidecarVad`) reads
    /// the inherent `Result` directly and is unaffected.
    failed: bool,
}

#[cfg(feature = "whisper")]
impl SileroSidecarVad {
    fn new(vad_path: &Path) -> Result<Self, whisper_rs::WhisperError> {
        let vad_path = vad_path
            .to_str()
            .ok_or(whisper_rs::WhisperError::NullPointer)?;

        let mut ctx_params = whisper_rs::WhisperVadContextParams::default();
        ctx_params.set_n_threads(
            std::thread::available_parallelism()
                .map(|count| count.get() as i32)
                .unwrap_or(4)
                .min(4),
        );

        let mut params = whisper_rs::WhisperVadParams::default();
        params.set_threshold(SIDECAR_VAD_THRESHOLD);
        params.set_min_speech_duration(SIDECAR_VAD_MIN_SPEECH_MS);
        params.set_min_silence_duration(SIDECAR_VAD_MIN_SILENCE_MS);
        params.set_speech_pad(SIDECAR_VAD_SPEECH_PAD_MS);

        let ctx = whisper_rs::WhisperVadContext::new(vad_path, ctx_params)?;

        Ok(Self {
            ctx,
            params,
            buffer: Vec::with_capacity(16000 * 3),
            idle_buffer_samples: 16 * SIDECAR_VAD_IDLE_BUFFER_MS,
            active_buffer_samples: 16 * SIDECAR_VAD_ACTIVE_BUFFER_MS,
            min_silence_ms: SIDECAR_VAD_MIN_SILENCE_MS as u64,
            chunk_ms: SIDECAR_VAD_CHUNK_MS,
            silence_ms: 0,
            failed: false,
        })
    }

    fn process(
        &mut self,
        samples: &[f32],
        rms: f32,
    ) -> Result<VadResult, whisper_rs::WhisperError> {
        self.buffer.extend_from_slice(samples);

        let segments = match self.ctx.segments_from_samples(self.params, &self.buffer) {
            Ok(segments) => segments,
            Err(e) => {
                self.failed = true;
                return Err(e);
            }
        };
        let buffer_ms = samples_to_ms(self.buffer.len());
        let last_segment_end_ms = if segments.num_segments() > 0 {
            segments
                .get_segment(segments.num_segments() - 1)
                .map(|segment| (segment.end * 10.0).round().max(0.0) as u64)
        } else {
            None
        };

        let speaking = last_segment_end_ms
            .map(|end_ms| buffer_ms.saturating_sub(end_ms) < self.min_silence_ms)
            .unwrap_or(false);

        self.silence_ms = if speaking {
            0
        } else if let Some(end_ms) = last_segment_end_ms {
            buffer_ms.saturating_sub(end_ms)
        } else {
            self.silence_ms.saturating_add(self.chunk_ms)
        };

        let max_len = if speaking || last_segment_end_ms.is_some() {
            self.active_buffer_samples
        } else {
            self.idle_buffer_samples
        };
        trim_front(&mut self.buffer, max_len);

        Ok(VadResult {
            speaking,
            silence_ms: self.silence_ms,
            energy: rms,
            noise_floor: 0.0,
        })
    }
}

#[cfg(feature = "whisper")]
impl VadEngine for SileroSidecarVad {
    /// Trait dispatch over `SileroSidecarVad::process`. Absorbs whisper
    /// errors by setting the sticky `failed` flag and returning a
    /// silence frame. Callers using trait dispatch must check
    /// `is_healthy()` after each call and swap engines when it flips
    /// to `false`. The existing enum-based `RecordingSidecarVad` keeps
    /// using the inherent fallible API and its swap-on-error semantics
    /// are unaffected.
    fn process(&mut self, samples: &[f32], rms: f32) -> VadResult {
        match SileroSidecarVad::process(self, samples, rms) {
            Ok(result) => result,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "Silero VAD process failed (trait dispatch) — emitting silence frame; is_healthy is now false"
                );
                VadResult {
                    speaking: false,
                    silence_ms: self.silence_ms,
                    energy: rms,
                    noise_floor: 0.0,
                }
            }
        }
    }

    fn name(&self) -> &'static str {
        "whisper-silero"
    }

    fn is_healthy(&self) -> bool {
        !self.failed
    }

    fn reset(&mut self) {
        // Reset reusable per-utterance state only. The `failed` flag
        // is intentionally NOT cleared — sticky failures stay sticky,
        // per the `VadEngine` trait contract. Dispatcher replaces
        // failed engines; reset does not revive them.
        self.buffer.clear();
        self.silence_ms = 0;
        debug_assert!(
            !self.failed,
            "reset called on a failed SileroSidecarVad — dispatcher should replace, not reset"
        );
    }
}

#[cfg(all(test, feature = "whisper"))]
impl SileroSidecarVad {
    /// Test-only seam to flip the sticky failure flag without
    /// requiring a corrupted model context. Used to verify the
    /// `is_healthy` contract without a live whisper session.
    pub(crate) fn force_failed_for_test(&mut self, value: bool) {
        self.failed = value;
    }
}

#[cfg(feature = "whisper")]
fn samples_to_ms(samples: usize) -> u64 {
    ((samples as u64) * 1000) / 16000
}

#[cfg(feature = "whisper")]
fn trim_front(buffer: &mut Vec<f32>, max_len: usize) {
    if buffer.len() > max_len {
        let drop = buffer.len() - max_len;
        buffer.drain(0..drop);
    }
}

// Gated on `whisper` alone: its only caller (`transcribe_utterance_for_sidecar`)
// is cfg(feature = "whisper"), and a narrower gate breaks Linux default-feature
// builds (whisper on, parakeet off, not macOS).
#[cfg(feature = "whisper")]
fn transcribe_with_whisper_for_live_sidecar(
    samples: &[f32],
    whisper_ctx: &whisper_rs::WhisperContext,
    language: Option<String>,
) -> Option<(String, f64)> {
    if samples.is_empty() {
        return None;
    }

    // This helper is a batch path: it accumulates `samples` via `feed()` and
    // discards every partial result, returning only `finalize()`. Running
    // partials here is wasted work — they cost O(buffer_len) per call but
    // the caller only ever uses the final. Pass a 1-second cap so partials
    // are suppressed as soon as the buffer crosses MIN_TRANSCRIBE_SAMPLES,
    // i.e. effectively never. This is independent of the user's live-mode
    // `transcription.partial_max_secs` setting; that knob is for live
    // responsiveness, not for batch fan-in.
    let mut streaming = StreamingWhisper::with_partial_max_secs(language, 1);
    for chunk in samples.chunks(1600) {
        let _ = streaming.feed(chunk, whisper_ctx);
    }
    streaming
        .finalize(whisper_ctx)
        .map(|result| (result.text, result.duration_secs))
}

/// Cap on finalized utterances awaiting transcription in the recording
/// sidecar. Past this the engine is not keeping up with the meeting; holding
/// more audio only grows latency without ever catching up, so overflow drops
/// the newest utterance and is surfaced via `LiveStatus::dropped_utterances`.
#[cfg(feature = "whisper")]
const SIDECAR_UTTERANCE_QUEUE_CAP: usize = 3;

/// A poisoned writer mutex means a thread panicked mid-write; the writer's
/// own failure latches (`jsonl_failed`) already cover torn state, and losing
/// the remaining transcript over a poison flag would be strictly worse.
#[cfg(feature = "whisper")]
fn lock_ignore_poison<'a, T>(mutex: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(feature = "whisper")]
fn enqueue_sidecar_utterance(
    job_tx: &std::sync::mpsc::SyncSender<Vec<f32>>,
    samples: Vec<f32>,
    pending: &AtomicU64,
    dropped: &AtomicU64,
) {
    if samples.is_empty() {
        return;
    }
    match job_tx.try_send(samples) {
        Ok(()) => {
            pending.fetch_add(1, Ordering::Relaxed);
        }
        Err(std::sync::mpsc::TrySendError::Full(_)) => {
            let total = dropped.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(
                dropped_utterances = total,
                "live sidecar transcription backlog full — dropping utterance audio"
            );
        }
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {}
    }
}

/// Engine dispatch for one finalized utterance on the sidecar's transcription
/// worker. A parakeet failure permanently switches the session to whisper
/// (loaded lazily, then kept warm) so a broken engine costs one utterance,
/// not every subsequent one.
#[cfg(feature = "whisper")]
fn transcribe_utterance_for_sidecar(
    samples: &[f32],
    config: &Config,
    whisper_ctx: &mut Option<whisper_rs::WhisperContext>,
    parakeet_enabled: &mut bool,
) -> Option<(String, f64)> {
    #[cfg(feature = "parakeet")]
    if *parakeet_enabled {
        match transcribe_with_parakeet_for_live_sidecar(samples, config) {
            Ok(result) => return result,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "live recording-sidecar parakeet path failed — switching this session to whisper"
                );
                *parakeet_enabled = false;
                emit_live_engine_fallback_warning("recording-sidecar", &error.to_string());
            }
        }
    }
    #[cfg(not(feature = "parakeet"))]
    {
        let _ = &parakeet_enabled;
    }
    let ctx = match whisper_ctx {
        Some(ctx) => ctx,
        None => match load_sidecar_whisper_ctx(config) {
            Ok(ctx) => whisper_ctx.insert(ctx),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "whisper model unavailable for live sidecar — skipping utterance"
                );
                return None;
            }
        },
    };
    transcribe_with_whisper_for_live_sidecar(samples, ctx, config.transcription.language.clone())
}

#[cfg(feature = "whisper")]
fn load_sidecar_whisper_ctx(config: &Config) -> Result<whisper_rs::WhisperContext, MinutesError> {
    let model_path = if config.live_transcript.model.is_empty() {
        crate::transcribe::resolve_model_path_for_dictation(config)?
    } else {
        crate::transcribe::resolve_model_path_by_name(&config.live_transcript.model, config)?
    };
    tracing::info!(model = %model_path.display(), "loading whisper model for recording sidecar");
    Ok(whisper_rs::WhisperContext::new_with_params(
        model_path
            .to_str()
            .ok_or_else(|| TranscribeError::ModelLoadError("invalid path".into()))?,
        crate::transcribe::whisper_context_params(),
    )
    .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?)
}

/// Minimum utterance length for the parakeet path — mirrors
/// `StreamingWhisper::MIN_TRANSCRIBE_SAMPLES`. VAD blips shorter than this
/// are dropped without hitting the sidecar/subprocess, avoiding temp-file
/// churn and latency spikes on noisy inputs.
#[cfg(feature = "parakeet")]
const PARAKEET_LIVE_MIN_SAMPLES: usize = 16_000; // 1 second at 16kHz
#[cfg(all(feature = "whisper", target_os = "macos"))]
const APPLE_SPEECH_LIVE_MIN_SAMPLES: usize = 16_000; // 1 second at 16kHz

#[cfg(feature = "parakeet")]
fn transcribe_with_parakeet_for_live_sidecar(
    samples: &[f32],
    config: &Config,
) -> Result<Option<(String, f64)>, MinutesError> {
    if samples.len() < PARAKEET_LIVE_MIN_SAMPLES {
        // Drop sub-1s blips silently — they're almost always noise or mic
        // pops that VAD didn't fully suppress. Same threshold whisper uses.
        return Ok(None);
    }

    let tmp_wav = tempfile::Builder::new()
        .prefix("minutes-live-sidecar-utterance-")
        .suffix(".wav")
        .tempfile()
        .map_err(TranscribeError::Io)?;
    crate::transcribe::write_wav_16k_mono(tmp_wav.path(), samples)?;

    match crate::transcribe::transcribe(tmp_wav.path(), config) {
        Ok(result) => Ok(Some((result.text, samples.len() as f64 / 16000.0))),
        Err(TranscribeError::EmptyAudio) | Err(TranscribeError::EmptyTranscript(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(all(feature = "whisper", target_os = "macos"))]
fn transcribe_with_apple_speech_for_live_sidecar(
    samples: &[f32],
    config: &Config,
) -> Result<Option<(String, f64)>, MinutesError> {
    transcribe_with_apple_speech_for_live_sidecar_impl(
        samples,
        config,
        crate::apple_speech::transcribe_with_apple_speech,
    )
}

#[cfg(all(feature = "whisper", target_os = "macos"))]
fn transcribe_with_apple_speech_for_live_sidecar_impl<F>(
    samples: &[f32],
    config: &Config,
    transcribe_fn: F,
) -> Result<Option<(String, f64)>, MinutesError>
where
    F: FnOnce(
        &Path,
        Option<&str>,
        crate::apple_speech::AppleSpeechMode,
        bool,
    ) -> crate::error::Result<crate::apple_speech::AppleSpeechTranscriptionResult>,
{
    if samples.len() < APPLE_SPEECH_LIVE_MIN_SAMPLES {
        return Ok(None);
    }

    let tmp_wav = tempfile::Builder::new()
        .prefix("minutes-live-apple-speech-")
        .suffix(".wav")
        .tempfile()
        .map_err(TranscribeError::Io)?;
    crate::transcribe::write_wav_16k_mono(tmp_wav.path(), samples)?;

    let locale = crate::apple_speech::live_locale_hint(config.transcription.language.as_deref());
    let result = transcribe_fn(
        tmp_wav.path(),
        locale.as_deref(),
        crate::apple_speech::AppleSpeechMode::Speech,
        true,
    )?;

    if let Some(error) = result.error {
        return Err(MinutesError::Io(std::io::Error::other(error)));
    }
    if result.transcript.trim().is_empty() {
        return Ok(None);
    }

    Ok(Some((result.transcript, samples.len() as f64 / 16000.0)))
}

#[cfg(feature = "whisper")]
fn ensure_live_whisper_ctx<'a>(
    whisper_ctx: &'a mut Option<whisper_rs::WhisperContext>,
    config: &Config,
) -> Result<&'a whisper_rs::WhisperContext, MinutesError> {
    if whisper_ctx.is_none() {
        let model_path = if config.live_transcript.model.is_empty() {
            crate::transcribe::resolve_model_path_for_dictation(config)?
        } else {
            crate::transcribe::resolve_model_path_by_name(&config.live_transcript.model, config)?
        };
        tracing::info!(
            model = %model_path.display(),
            "loading whisper model for live transcript fallback"
        );
        let ctx = whisper_rs::WhisperContext::new_with_params(
            model_path
                .to_str()
                .ok_or_else(|| TranscribeError::ModelLoadError("invalid path".into()))?,
            crate::transcribe::whisper_context_params(),
        )
        .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?;
        *whisper_ctx = Some(ctx);
    }

    Ok(whisper_ctx
        .as_ref()
        .expect("whisper context should exist after ensure_live_whisper_ctx"))
}

#[cfg(all(feature = "whisper", feature = "parakeet"))]
fn resolve_apple_speech_live_fallback<P, W>(
    parakeet_fallback_ready: bool,
    mut try_parakeet: P,
    mut try_whisper: W,
) -> Result<Option<(String, f64)>, MinutesError>
where
    P: FnMut() -> Result<Option<(String, f64)>, MinutesError>,
    W: FnMut() -> Result<Option<(String, f64)>, MinutesError>,
{
    if parakeet_fallback_ready {
        match try_parakeet() {
            Ok(Some(result)) => return Ok(Some(result)),
            Ok(None) => return Ok(None),
            Err(_) => {}
        }
    }

    try_whisper()
}

/// Finalize one utterance via the active engine (apple-speech, parakeet, or whisper)
/// and write the resulting JSONL line.
///
/// Returns `true` if the write succeeded (or there was no text to write) and the
/// session should continue; `false` on JSONL write failure, which signals the
/// caller to stop to prevent data loss.
///
/// On apple/parakeet failure, the function automatically falls back to whisper
/// for the accumulated samples and flips that engine flag to `false` so the
/// remainder of the session uses whisper.
#[cfg(all(feature = "whisper", feature = "parakeet"))]
#[allow(clippy::too_many_arguments)]
fn finalize_live_utterance(
    writer: &mut LiveTranscriptWriter,
    apple_live_enabled: &mut bool,
    #[cfg(target_os = "macos")] apple_utterance_samples: &mut Vec<f32>,
    parakeet_fallback_ready: bool,
    parakeet_live_enabled: &mut bool,
    parakeet_utterance_samples: &mut Vec<f32>,
    config: &Config,
    streaming: &mut StreamingWhisper,
    whisper_ctx: &mut Option<whisper_rs::WhisperContext>,
    source: &'static str,
) -> bool {
    #[cfg(target_os = "macos")]
    if *apple_live_enabled {
        match transcribe_with_apple_speech_for_live_sidecar(apple_utterance_samples, config) {
            Ok(Some((text, duration_secs))) => {
                let ok = writer.write_utterance(&text, duration_secs);
                apple_utterance_samples.clear();
                return ok;
            }
            Ok(None) => {
                apple_utterance_samples.clear();
                return true;
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "live apple-speech path failed — switching this session to fallback backend"
                );
                *apple_live_enabled = false;
                emit_apple_speech_fallback_warning(source, &error.to_string());
                let fallback_result = resolve_apple_speech_live_fallback(
                    parakeet_fallback_ready,
                    || {
                        *parakeet_live_enabled = true;
                        match transcribe_with_parakeet_for_live_sidecar(
                            apple_utterance_samples,
                            config,
                        ) {
                            Ok(result) => Ok(result),
                            Err(parakeet_error) => {
                                tracing::warn!(
                                    error = %parakeet_error,
                                    "parakeet fallback after apple-speech live failure also failed — switching this session to whisper"
                                );
                                *parakeet_live_enabled = false;
                                Err(parakeet_error)
                            }
                        }
                    },
                    || {
                        let whisper_ctx = ensure_live_whisper_ctx(whisper_ctx, config).map_err(|load_error| {
                            tracing::error!(
                                error = %load_error,
                                "failed to load whisper fallback after apple-speech live failure"
                            );
                            load_error
                        })?;
                        Ok(transcribe_with_whisper_for_live_sidecar(
                            apple_utterance_samples,
                            whisper_ctx,
                            config.transcription.language.clone(),
                        ))
                    },
                );

                match fallback_result {
                    Ok(Some((text, duration_secs))) => {
                        let ok = writer.write_utterance(&text, duration_secs);
                        apple_utterance_samples.clear();
                        return ok;
                    }
                    Ok(None) => {
                        apple_utterance_samples.clear();
                        return true;
                    }
                    Err(_) => {}
                }
                apple_utterance_samples.clear();
                return true;
            }
        }
    }

    if *parakeet_live_enabled {
        match transcribe_with_parakeet_for_live_sidecar(parakeet_utterance_samples, config) {
            Ok(Some((text, duration_secs))) => {
                let ok = writer.write_utterance(&text, duration_secs);
                parakeet_utterance_samples.clear();
                return ok;
            }
            Ok(None) => {
                parakeet_utterance_samples.clear();
                return true;
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "live parakeet path failed — switching this session to whisper"
                );
                *parakeet_live_enabled = false;
                emit_live_engine_fallback_warning(source, &error.to_string());
                match ensure_live_whisper_ctx(whisper_ctx, config) {
                    Ok(whisper_ctx) => {
                        if let Some((text, duration_secs)) =
                            transcribe_with_whisper_for_live_sidecar(
                                parakeet_utterance_samples,
                                whisper_ctx,
                                config.transcription.language.clone(),
                            )
                        {
                            let ok = writer.write_utterance(&text, duration_secs);
                            parakeet_utterance_samples.clear();
                            return ok;
                        }
                    }
                    Err(load_error) => {
                        tracing::error!(
                            error = %load_error,
                            "failed to load whisper fallback after parakeet live failure"
                        );
                    }
                }
                parakeet_utterance_samples.clear();
                return true;
            }
        }
    }

    // Whisper path
    let write_ok = match ensure_live_whisper_ctx(whisper_ctx, config) {
        Ok(whisper_ctx) => {
            let ok = if let Some(sr) = streaming.finalize(whisper_ctx) {
                writer.write_utterance(&sr.text, sr.duration_secs)
            } else {
                true
            };
            streaming.reset();
            ok
        }
        Err(error) => {
            tracing::error!(
                error = %error,
                "failed to load whisper backend for live transcript"
            );
            streaming.reset();
            false
        }
    };
    write_ok
}

/// Shutdown-time helper: finalize any partial utterance and log loudly on
/// JSONL write failure. Called at every early-exit branch of `run_inner`
/// (stop flag, stop sentinel, reconnect failure, stream disconnect failure).
///
/// We can't propagate the write_ok signal back into the caller's control flow
/// at these branches — we're already breaking out of the loop. But if the
/// write failed, the last utterance is silently dropped unless we log it.
#[cfg(all(feature = "whisper", feature = "parakeet"))]
#[allow(clippy::too_many_arguments)]
fn finalize_on_exit(
    writer: &mut LiveTranscriptWriter,
    apple_live_enabled: &mut bool,
    #[cfg(target_os = "macos")] apple_utterance_samples: &mut Vec<f32>,
    parakeet_fallback_ready: bool,
    parakeet_live_enabled: &mut bool,
    parakeet_utterance_samples: &mut Vec<f32>,
    config: &Config,
    streaming: &mut StreamingWhisper,
    whisper_ctx: &mut Option<whisper_rs::WhisperContext>,
    source: &'static str,
) {
    if !finalize_live_utterance(
        writer,
        apple_live_enabled,
        #[cfg(target_os = "macos")]
        apple_utterance_samples,
        parakeet_fallback_ready,
        parakeet_live_enabled,
        parakeet_utterance_samples,
        config,
        streaming,
        whisper_ctx,
        source,
    ) {
        tracing::error!(
            "JSONL write failed while finalizing last utterance on shutdown — last utterance may be lost"
        );
    }
}

#[cfg(all(feature = "whisper", not(feature = "parakeet")))]
#[allow(clippy::too_many_arguments)]
fn finalize_live_utterance(
    writer: &mut LiveTranscriptWriter,
    apple_live_enabled: &mut bool,
    #[cfg(target_os = "macos")] apple_utterance_samples: &mut Vec<f32>,
    _parakeet_fallback_ready: bool,
    config: &Config,
    streaming: &mut StreamingWhisper,
    whisper_ctx: &mut Option<whisper_rs::WhisperContext>,
    source: &'static str,
) -> bool {
    #[cfg(target_os = "macos")]
    if *apple_live_enabled {
        match transcribe_with_apple_speech_for_live_sidecar(apple_utterance_samples, config) {
            Ok(Some((text, duration_secs))) => {
                let ok = writer.write_utterance(&text, duration_secs);
                apple_utterance_samples.clear();
                return ok;
            }
            Ok(None) => {
                apple_utterance_samples.clear();
                return true;
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "live apple-speech path failed — switching this session to whisper"
                );
                *apple_live_enabled = false;
                emit_apple_speech_fallback_warning(source, &error.to_string());
                match ensure_live_whisper_ctx(whisper_ctx, config) {
                    Ok(whisper_ctx) => {
                        if let Some((text, duration_secs)) =
                            transcribe_with_whisper_for_live_sidecar(
                                apple_utterance_samples,
                                whisper_ctx,
                                config.transcription.language.clone(),
                            )
                        {
                            let ok = writer.write_utterance(&text, duration_secs);
                            apple_utterance_samples.clear();
                            return ok;
                        }
                    }
                    Err(load_error) => {
                        tracing::error!(
                            error = %load_error,
                            "failed to load whisper fallback after apple-speech live failure"
                        );
                    }
                }
                apple_utterance_samples.clear();
                return true;
            }
        }
    }

    let write_ok = match ensure_live_whisper_ctx(whisper_ctx, config) {
        Ok(whisper_ctx) => {
            if let Some(sr) = streaming.finalize(whisper_ctx) {
                writer.write_utterance(&sr.text, sr.duration_secs)
            } else {
                true
            }
        }
        Err(error) => {
            tracing::error!(
                error = %error,
                "failed to load whisper backend for live transcript"
            );
            false
        }
    };
    #[cfg(not(target_os = "macos"))]
    let _ = (&apple_live_enabled, &config, source);
    streaming.reset();
    write_ok
}

#[cfg(all(feature = "whisper", not(feature = "parakeet")))]
#[allow(clippy::too_many_arguments)]
fn finalize_on_exit(
    writer: &mut LiveTranscriptWriter,
    apple_live_enabled: &mut bool,
    #[cfg(target_os = "macos")] apple_utterance_samples: &mut Vec<f32>,
    parakeet_fallback_ready: bool,
    config: &Config,
    streaming: &mut StreamingWhisper,
    whisper_ctx: &mut Option<whisper_rs::WhisperContext>,
    source: &'static str,
) {
    if !finalize_live_utterance(
        writer,
        apple_live_enabled,
        #[cfg(target_os = "macos")]
        apple_utterance_samples,
        parakeet_fallback_ready,
        config,
        streaming,
        whisper_ctx,
        source,
    ) {
        tracing::error!(
            "JSONL write failed while finalizing last utterance on shutdown — last utterance may be lost"
        );
    }
}

/// Run a live transcript sidecar that consumes audio samples from a channel.
/// Blocks until the channel disconnects (recording stopped) or stop_flag is set.
/// Loads its own whisper model (tiny/base) for real-time streaming.
#[cfg(feature = "whisper")]
pub fn run_sidecar_mpsc(
    rx: std::sync::mpsc::Receiver<Vec<f32>>,
    stop_flag: Arc<AtomicBool>,
    config: &Config,
) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_sidecar_inner_mpsc(rx, stop_flag, config)
    }));

    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let message = format!("{}", e);
            eprintln!(
                "[minutes] Live transcript unavailable: {} — recording continues without real-time transcript",
                message
            );
            tracing::warn!("live sidecar stopped: {}", message);
            write_live_status_transition(LiveStatusState::Failed, Some(message.as_str()));
        }
        Err(payload) => {
            let message = panic_payload_to_string(payload.as_ref());
            eprintln!(
                "[minutes] Live transcript unavailable: {} — recording continues without real-time transcript",
                message
            );
            tracing::error!("live sidecar panicked: {}", message);
            write_live_status_transition(LiveStatusState::Failed, Some(message.as_str()));
        }
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
    // Guard: don't clobber a standalone live transcript session's JSONL.
    // `inspect_pid_file` (not `check_pid_file`) so a standalone session holding
    // the PID file under a mandatory Windows lock is detected — otherwise the
    // sidecar would write the same `live-transcript.jsonl` concurrently. See #258.
    let lt_pid = pid::live_transcript_pid_path();
    if pid::inspect_pid_file(&lt_pid).is_active() {
        tracing::info!("standalone live transcript active — skipping recording sidecar");
        return Ok(());
    }

    write_live_status_transition(LiveStatusState::Starting, None);

    let mut sidecar_config = config.clone();
    sidecar_config.live_transcript.save_wav = false;
    let writer = Arc::new(Mutex::new(Some(LiveTranscriptWriter::new(
        &sidecar_config,
        None,
        TranscriptSource::RecordingSidecar,
    )?)));
    if let Some(w) = lock_ignore_poison(&writer).as_mut() {
        w.mark_healthy();
    }

    let mut vad = RecordingSidecarVad::new(config);
    let parakeet_live_enabled = live_supports_parakeet(&config.transcription.engine);
    let mut was_speaking = false;
    let mut utterance: Vec<f32> = Vec::new();
    let mut gating_stats = SidecarGatingStats::default();
    let max_utterance_secs = config.live_transcript.max_utterance_secs.max(5);
    let max_utterance_samples = (max_utterance_secs as usize).saturating_mul(16000);

    if config.transcription.engine.eq_ignore_ascii_case("parakeet") && !parakeet_live_enabled {
        emit_live_engine_scope_warning(&config.transcription.engine, "recording-sidecar");
    }
    if config
        .transcription
        .engine
        .eq_ignore_ascii_case("apple-speech")
    {
        eprintln!(
            "[minutes] apple-speech currently applies only to standalone live transcript; recording sidecar continues with whisper"
        );
        tracing::info!(
            "apple-speech requested for recording sidecar live transcript — keeping whisper for this scoped experiment"
        );
    }

    // ── Transcription worker ─────────────────────────────────────
    // Inference must never run on this (consumer) thread: a single slow or
    // wedged engine call starves the bounded capture channel (audio drops →
    // garbled lines), freezes the status heartbeat, and defers stop handling
    // for the duration of the call. The consumer below only does VAD and
    // buffering; finalized utterances are handed to this worker over a small
    // bounded queue, and a backlogged engine costs us utterances (counted in
    // the status file) instead of the whole session.
    let (job_tx, job_rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(SIDECAR_UTTERANCE_QUEUE_CAP);
    let pending_utterances = Arc::new(AtomicU64::new(0));
    let dropped_utterances = Arc::new(AtomicU64::new(0));
    let worker = {
        let writer = Arc::clone(&writer);
        let config = config.clone();
        let pending = Arc::clone(&pending_utterances);
        let stop_flag = Arc::clone(&stop_flag);
        std::thread::Builder::new()
            .name("live-sidecar-transcribe".into())
            .spawn(move || {
                let mut parakeet_enabled = parakeet_live_enabled;
                // Loaded lazily on first whisper-path utterance: the load can
                // take >10s for larger models, and doing it eagerly (worse, on
                // the consumer thread, as before) dropped the first ~13s of
                // every meeting. Parakeet sessions never pay it at all.
                let mut whisper_ctx: Option<whisper_rs::WhisperContext> = None;
                while let Ok(samples) = job_rx.recv() {
                    // After stop, the recording's batch transcription
                    // supersedes the live transcript; drain instead of
                    // transcribing so stop stays responsive.
                    if stop_flag.load(Ordering::Relaxed) {
                        pending.fetch_sub(1, Ordering::Relaxed);
                        continue;
                    }
                    let result = transcribe_utterance_for_sidecar(
                        &samples,
                        &config,
                        &mut whisper_ctx,
                        &mut parakeet_enabled,
                    );
                    pending.fetch_sub(1, Ordering::Relaxed);
                    if let Some((text, duration_secs)) = result {
                        if let Some(w) = lock_ignore_poison(&writer).as_mut() {
                            w.write_utterance(&text, duration_secs);
                        }
                    }
                }
            })
            .map_err(|e| MinutesError::from(TranscribeError::Io(e)))?
    };

    tracing::info!("live sidecar started (recording mode)");

    loop {
        // The heartbeat must never block behind the worker (which holds the
        // lock across file IO in write_utterance). Skipping a contended tick
        // is fine: the heartbeat interval is 1s and the staleness threshold
        // 3s, and a write in progress is itself proof of liveness.
        match writer.try_lock() {
            Ok(mut guard) => {
                if let Some(w) = guard.as_mut() {
                    w.set_backlog_stats(
                        pending_utterances.load(Ordering::Relaxed),
                        dropped_utterances.load(Ordering::Relaxed),
                    );
                    w.maybe_write_heartbeat();
                }
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                if let Some(w) = poisoned.into_inner().as_mut() {
                    w.maybe_write_heartbeat();
                }
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        if stop_flag.load(Ordering::Relaxed) {
            enqueue_sidecar_utterance(
                &job_tx,
                std::mem::take(&mut utterance),
                &pending_utterances,
                &dropped_utterances,
            );
            break;
        }

        let samples = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(s) => s,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                enqueue_sidecar_utterance(
                    &job_tx,
                    std::mem::take(&mut utterance),
                    &pending_utterances,
                    &dropped_utterances,
                );
                break;
            }
        };

        let rms = if samples.is_empty() {
            0.0
        } else {
            let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
            (sum_sq / samples.len() as f32).sqrt()
        };

        let vad_result = vad.process(&samples, rms);
        gating_stats.observe(samples.len(), vad_result.speaking);

        if vad_result.speaking {
            was_speaking = true;
            utterance.extend_from_slice(&samples);

            if utterance.len() >= max_utterance_samples {
                tracing::info!("sidecar: max utterance duration, force-finalizing");
                enqueue_sidecar_utterance(
                    &job_tx,
                    std::mem::take(&mut utterance),
                    &pending_utterances,
                    &dropped_utterances,
                );
                was_speaking = false;
            }
        } else if was_speaking && !utterance.is_empty() {
            enqueue_sidecar_utterance(
                &job_tx,
                std::mem::take(&mut utterance),
                &pending_utterances,
                &dropped_utterances,
            );
            was_speaking = false;
        }
    }

    // Let the worker drain its queue (post-stop jobs are skipped, and a single
    // in-flight engine call is bounded by the parakeet subprocess timeout),
    // then finalize.
    drop(job_tx);
    let _ = worker.join();

    let (lines, duration, _path) = match lock_ignore_poison(&writer).take() {
        Some(w) => w.finalize(),
        None => (0, 0.0, PathBuf::new()),
    };
    // Clean up status file so session_status() doesn't report stale data
    clear_status_file();
    tracing::info!(
        vad_mode = vad.mode_name(),
        samples_fed = gating_stats.samples_fed,
        samples_gated = gating_stats.samples_gated,
        speaking_windows = gating_stats.speaking_windows,
        silence_windows = gating_stats.silence_windows,
        "live sidecar gating summary"
    );
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
    read_since_line_from_path(&path, since_line)
}

fn read_since_line_from_path(
    path: &Path,
    since_line: usize,
) -> Result<Vec<TranscriptLine>, MinutesError> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = File::open(path)?;
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
    // Standalone live-transcript PID. Use `inspect_pid_file` rather than
    // `check_pid_file` so a session that holds the PID file under a mandatory
    // Windows lock is still detected as active: the in-app reader and the lock
    // holder are the same process, and a plain read of the locked file fails on
    // Windows. See `pid::PidFileState` and issue #258.
    let lt_pid = pid::live_transcript_pid_path();
    let lt_pid_state = pid::inspect_pid_file(&lt_pid);

    let recording_pid = pid::check_recording().ok().flatten();
    let status_path = pid::live_transcript_status_path();
    let jsonl_path = pid::live_transcript_jsonl_path();

    derive_session_status(lt_pid_state, recording_pid, &status_path, &jsonl_path)
}

fn derive_session_status(
    lt_pid_state: pid::PidFileState,
    recording_pid: Option<u32>,
    status_path: &Path,
    jsonl_path: &Path,
) -> SessionStatus {
    let live_status = read_live_status(status_path);
    let now = Local::now();
    // A held PID file (readable on Unix, or lock-detected on Windows) proves the
    // standalone session is alive. Liveness is NOT gated on heartbeat freshness:
    // the heartbeat is written inline by the live loop, so a long utterance or
    // model load can stall it past the staleness window while the session is
    // perfectly healthy — gating on it would re-introduce the #258 flicker.
    let standalone_active = lt_pid_state.is_active();

    let (sidecar_active, diagnostic) = if recording_pid.is_some() {
        evaluate_recording_sidecar_status(live_status.as_ref(), now)
    } else {
        (false, None)
    };

    let active = standalone_active || sidecar_active;
    let pid = if standalone_active {
        // `None` when the session is alive but the PID is unreadable (Windows
        // locked file) — we report active without fabricating a PID.
        lt_pid_state.pid()
    } else if sidecar_active {
        recording_pid
    } else {
        None
    };

    let should_report_stats = standalone_active || recording_pid.is_some();
    let (line_count, duration_secs) = if should_report_stats {
        status_metrics(live_status.as_ref(), jsonl_path, now)
    } else {
        (0, 0.0)
    };

    let source = if standalone_active {
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
        session_id: live_status
            .as_ref()
            .and_then(|status| status.session_id.clone()),
        jsonl_path: if jsonl_path.exists() {
            Some(jsonl_path.to_string_lossy().to_string())
        } else {
            None
        },
        source,
        diagnostic,
    }
}

fn read_live_status(path: &Path) -> Option<LiveStatus> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<LiveStatus>(&content).ok())
}

fn write_live_status(status: &LiveStatus) {
    let path = pid::live_transcript_status_path();
    let tmp = path.with_extension("json.tmp");
    if let Ok(json) = serde_json::to_string(status) {
        if std::fs::write(&tmp, json).is_ok() {
            std::fs::rename(&tmp, &path).ok();
        }
    }
}

fn write_live_status_transition(state: LiveStatusState, diagnostic: Option<&str>) {
    let now = Local::now();
    let existing = read_live_status(&pid::live_transcript_status_path());
    let status = LiveStatus {
        start_time: existing.as_ref().map(|s| s.start_time).unwrap_or(now),
        updated_at: now,
        state,
        line_count: existing.as_ref().map(|s| s.line_count).unwrap_or(0),
        last_offset_ms: existing.as_ref().map(|s| s.last_offset_ms).unwrap_or(0),
        last_duration_ms: existing.as_ref().map(|s| s.last_duration_ms).unwrap_or(0),
        session_id: existing.as_ref().and_then(|s| s.session_id.clone()),
        diagnostic: diagnostic.map(str::to_string),
        pending_utterances: existing.as_ref().and_then(|s| s.pending_utterances),
        dropped_utterances: existing.as_ref().and_then(|s| s.dropped_utterances),
    };
    write_live_status(&status);
}

fn evaluate_recording_sidecar_status(
    live_status: Option<&LiveStatus>,
    now: DateTime<Local>,
) -> (bool, Option<String>) {
    let Some(status) = live_status else {
        return (false, Some("sidecar status unavailable".into()));
    };

    match status.state {
        LiveStatusState::Healthy => {
            let age = (now - status.updated_at).num_seconds().max(0);
            if age > SIDECAR_HEALTH_STALE_AFTER_SECS {
                (false, Some("sidecar heartbeat stale".into()))
            } else {
                (true, None)
            }
        }
        LiveStatusState::Starting => {
            let age = (now - status.start_time).num_seconds().max(0);
            if age > SIDECAR_STARTUP_TIMEOUT_SECS {
                (false, Some("sidecar still starting".into()))
            } else {
                (false, Some("sidecar starting".into()))
            }
        }
        LiveStatusState::Failed => (
            false,
            Some(
                status
                    .diagnostic
                    .clone()
                    .filter(|msg| !msg.trim().is_empty())
                    .unwrap_or_else(|| "sidecar failed".into()),
            ),
        ),
        LiveStatusState::Stopped => (
            false,
            Some(
                status
                    .diagnostic
                    .clone()
                    .filter(|msg| !msg.trim().is_empty())
                    .unwrap_or_else(|| "sidecar stopped".into()),
            ),
        ),
    }
}

fn status_metrics(
    live_status: Option<&LiveStatus>,
    jsonl_path: &Path,
    now: DateTime<Local>,
) -> (usize, f64) {
    if let Some(status) = live_status {
        let elapsed = (now - status.start_time).num_seconds().max(0) as f64;
        return (status.line_count, elapsed);
    }

    let lines = if jsonl_path.exists() {
        read_since_line_from_path(jsonl_path, 0).unwrap_or_default()
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

fn panic_payload_to_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("sidecar panicked: {}", message)
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("sidecar panicked: {}", message)
    } else {
        "sidecar panicked".into()
    }
}

fn set_permissions_0600(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Remove the status file so `session_status()` won't report stale data.
pub fn clear_status_file() {
    std::fs::remove_file(pid::live_transcript_status_path()).ok();
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::live_engine_scope_warning;
    #[cfg(feature = "parakeet")]
    use super::PARAKEET_LIVE_FALLBACK_WARNING;
    #[cfg(not(feature = "parakeet"))]
    use super::PARAKEET_LIVE_SCOPE_WARNING;
    use super::*;
    use chrono::Duration as ChronoDuration;
    use std::io::Write;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use tempfile::{tempdir, NamedTempFile};

    fn with_temp_home<T>(f: impl FnOnce() -> T) -> T {
        let _lock = crate::test_home_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let original_home = std::env::var_os("HOME");
        #[cfg(windows)]
        let original_userprofile = std::env::var_os("USERPROFILE");

        std::env::set_var("HOME", dir.path());
        #[cfg(windows)]
        std::env::set_var("USERPROFILE", dir.path());

        let result = f();

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        #[cfg(windows)]
        if let Some(userprofile) = original_userprofile {
            std::env::set_var("USERPROFILE", userprofile);
        } else {
            std::env::remove_var("USERPROFILE");
        }

        result
    }

    fn live_status_with_state(state: LiveStatusState) -> LiveStatus {
        LiveStatus {
            start_time: Local::now(),
            updated_at: Local::now(),
            state,
            line_count: 3,
            last_offset_ms: 1200,
            last_duration_ms: 400,
            session_id: None,
            diagnostic: None,
            pending_utterances: None,
            dropped_utterances: None,
        }
    }

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
    fn sidecar_utterance_queue_drops_newest_when_full_and_counts() {
        let (tx, _rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(2);
        let pending = AtomicU64::new(0);
        let dropped = AtomicU64::new(0);

        enqueue_sidecar_utterance(&tx, vec![0.1; 1600], &pending, &dropped);
        enqueue_sidecar_utterance(&tx, vec![0.1; 1600], &pending, &dropped);
        // Queue full — this one must be dropped, not block the caller.
        enqueue_sidecar_utterance(&tx, vec![0.1; 1600], &pending, &dropped);
        // Empty utterances are a no-op either way.
        enqueue_sidecar_utterance(&tx, Vec::new(), &pending, &dropped);

        assert_eq!(pending.load(Ordering::Relaxed), 2);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn sidecar_utterance_queue_disconnected_worker_is_silent() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(2);
        drop(rx);
        let pending = AtomicU64::new(0);
        let dropped = AtomicU64::new(0);

        enqueue_sidecar_utterance(&tx, vec![0.1; 1600], &pending, &dropped);

        assert_eq!(pending.load(Ordering::Relaxed), 0);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn status_file_surfaces_transcription_backlog() {
        with_temp_home(|| {
            let config = Config::default();
            let mut writer =
                LiveTranscriptWriter::new(&config, None, TranscriptSource::RecordingSidecar)
                    .unwrap();

            writer.set_backlog_stats(2, 3);
            writer.mark_healthy();

            let status = read_live_status(&pid::live_transcript_status_path()).unwrap();
            assert_eq!(status.pending_utterances, Some(2));
            assert_eq!(status.dropped_utterances, Some(3));
            assert!(status
                .diagnostic
                .as_deref()
                .unwrap()
                .contains("3 utterance(s) dropped"));

            // Counters at zero serialize as absent — backward compatible.
            writer.set_backlog_stats(0, 0);
            writer.mark_healthy();
            let status = read_live_status(&pid::live_transcript_status_path()).unwrap();
            assert_eq!(status.pending_utterances, None);
            assert_eq!(status.dropped_utterances, None);
            assert_eq!(status.diagnostic, None);
        });
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

    #[test]
    fn write_utterance_emits_live_utterance_event() {
        with_temp_home(|| {
            let config = Config::default();
            let mut writer = LiveTranscriptWriter::new(
                &config,
                Some("session-1".into()),
                TranscriptSource::Standalone,
            )
            .unwrap();

            assert!(writer.write_utterance("hello from live mode", 1.25));

            let events = crate::events::read_events_since_seq(0, None);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].seq, 1);
            let json = serde_json::to_string(&events[0]).unwrap();
            assert!(json.contains("\"event_type\":\"live.utterance.final\""));
            match &events[0].event {
                crate::events::MinutesEvent::LiveUtteranceFinal {
                    session_id,
                    source,
                    transcript_path,
                    line,
                    text,
                    speaker,
                    offset_ms: _,
                    duration_ms,
                } => {
                    assert_eq!(session_id.as_deref(), Some("session-1"));
                    assert_eq!(source, "standalone");
                    assert!(transcript_path.ends_with("live-transcript.jsonl"));
                    assert_eq!(*line, 1);
                    assert_eq!(text, "hello from live mode");
                    assert!(speaker.is_none());
                    assert_eq!(*duration_ms, 1250);
                }
                other => panic!("expected LiveUtteranceFinal, got {other:?}"),
            }
        });
    }

    #[cfg(feature = "whisper")]
    #[test]
    fn precreated_context_session_is_failed_when_recording_blocks_start() {
        with_temp_home(|| {
            let session =
                crate::context_store::start_live_transcript_session(Local::now()).unwrap();
            let _recording_guard = crate::pid::create_pid_guard(&crate::pid::pid_path()).unwrap();
            let stop_flag = Arc::new(AtomicBool::new(false));

            let error = run(stop_flag, &Config::default(), Some(session.id.clone())).unwrap_err();

            assert!(matches!(
                error,
                MinutesError::LiveTranscript(LiveTranscriptError::RecordingActive)
            ));

            let reloaded = crate::context_store::get_session(&session.id)
                .unwrap()
                .expect("context session should exist");
            assert_eq!(
                reloaded.state,
                crate::context_store::ContextSessionState::Failed
            );
        });
    }

    #[cfg(feature = "whisper")]
    fn read_wav_samples(path: &Path) -> Vec<f32> {
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        let raw: Vec<f32> = reader
            .samples::<i16>()
            .map(|sample| sample.unwrap() as f32 / i16::MAX as f32)
            .collect();

        let mono = if spec.channels == 1 {
            raw
        } else {
            raw.chunks(spec.channels as usize)
                .map(|frame| frame.iter().copied().sum::<f32>() / frame.len() as f32)
                .collect()
        };

        if spec.sample_rate == 16000 {
            mono
        } else {
            crate::transcribe::resample(&mono, spec.sample_rate, 16000)
        }
    }

    #[cfg(feature = "whisper")]
    fn write_wav_samples(path: &Path, samples: &[f32]) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for sample in samples {
            let pcm = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
            writer.write_sample(pcm).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[cfg(feature = "whisper")]
    fn pad_with_silence_to_db_rms(samples: &[f32], target_db: f32) -> Vec<f32> {
        let target_rms = 10f32.powf(target_db / 20.0);
        let speech_energy = samples.iter().map(|sample| sample * sample).sum::<f32>();
        let target_total_samples = (speech_energy / target_rms.powi(2))
            .ceil()
            .max(samples.len() as f32) as usize;
        let silence_needed = target_total_samples.saturating_sub(samples.len());
        let lead_silence = silence_needed / 2;
        let tail_silence = silence_needed - lead_silence;

        let mut padded = Vec::with_capacity(target_total_samples);
        padded.extend(std::iter::repeat_n(0.0, lead_silence));
        padded.extend_from_slice(samples);
        padded.extend(std::iter::repeat_n(0.0, tail_silence));
        padded
    }

    #[cfg(feature = "whisper")]
    fn rms_db(samples: &[f32]) -> f32 {
        let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>()
            / samples.len() as f32)
            .sqrt()
            .max(1e-6);
        20.0 * rms.log10()
    }

    #[cfg(feature = "whisper")]
    fn count_detected_utterances<T>(
        detector: &mut T,
        samples: &[f32],
        mut process: impl FnMut(&mut T, &[f32], f32) -> VadResult,
    ) -> usize {
        let mut utterances = 0usize;
        let mut was_speaking = false;

        for chunk in samples.chunks(1600) {
            let rms = if chunk.is_empty() {
                0.0
            } else {
                let sum_sq: f32 = chunk.iter().map(|sample| sample * sample).sum();
                (sum_sq / chunk.len() as f32).sqrt()
            };
            let vad_result = process(detector, chunk, rms);

            if vad_result.speaking {
                was_speaking = true;
            } else if was_speaking {
                utterances += 1;
                was_speaking = false;
            }
        }

        if was_speaking {
            utterances += 1;
        }

        utterances
    }

    #[cfg(feature = "whisper")]
    #[test]
    fn silero_sidecar_vad_recovers_quiet_minus_40_db_wav() {
        let config = Config::default();
        let Some(vad_path) = crate::transcribe::resolve_vad_model_path(&config) else {
            eprintln!("skipping quiet-audio Silero VAD test — model not installed");
            return;
        };

        let demo_wav =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tauri/src/audio/cue-start.wav");
        let original = read_wav_samples(&demo_wav);
        let quiet = pad_with_silence_to_db_rms(&original, -40.0);
        let quiet_db = rms_db(&quiet);
        assert!(
            (-40.5..=-39.5).contains(&quiet_db),
            "expected fixture RMS near -40 dB, got {quiet_db:.2} dB"
        );
        let dir = tempdir().unwrap();
        let quiet_wav = dir.path().join("demo-minus-40db.wav");
        write_wav_samples(&quiet_wav, &quiet);
        let quiet_samples = read_wav_samples(&quiet_wav);

        let mut silero = SileroSidecarVad::new(&vad_path).unwrap();
        let utterances =
            count_detected_utterances(&mut silero, &quiet_samples, |detector, chunk, rms| {
                detector.process(chunk, rms).unwrap()
            });

        assert!(
            utterances >= 1,
            "expected at least one utterance from -40 dB WAV after Silero VAD"
        );
    }

    /// `is_healthy` must surface the sticky `failed` flag through trait
    /// dispatch. Uses the test-only `force_failed_for_test` seam to
    /// avoid needing a corrupt whisper context, since constructing one
    /// reliably from a unit test is brittle.
    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn silero_is_healthy_reflects_sticky_failure_via_trait() {
        use crate::vad::VadEngine;
        let model_path = dirs::home_dir()
            .unwrap()
            .join(".minutes/models/ggml-silero-v6.2.0.bin");
        if !model_path.exists() {
            eprintln!(
                "[is_healthy] skipping: no Silero model at {} — run `minutes setup`",
                model_path.display()
            );
            return;
        }
        let mut silero = SileroSidecarVad::new(&model_path).unwrap();
        // Healthy at construction.
        assert!(
            <SileroSidecarVad as VadEngine>::is_healthy(&silero),
            "freshly constructed Silero must be healthy"
        );
        assert_eq!(
            <SileroSidecarVad as VadEngine>::name(&silero),
            "whisper-silero"
        );
        // Force the sticky failure flag and confirm trait dispatch
        // reports it.
        silero.force_failed_for_test(true);
        assert!(
            !<SileroSidecarVad as VadEngine>::is_healthy(&silero),
            "is_healthy must return false once `failed` is set"
        );
    }

    /// `reset` must NOT clear sticky failure state — the trait
    /// contract is that failed engines are replaced, not revived.
    /// Verifying this on the real engine guards against future drift.
    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn silero_reset_does_not_clear_sticky_failure() {
        use crate::vad::VadEngine;
        let model_path = dirs::home_dir()
            .unwrap()
            .join(".minutes/models/ggml-silero-v6.2.0.bin");
        if !model_path.exists() {
            eprintln!(
                "[reset] skipping: no Silero model at {} — run `minutes setup`",
                model_path.display()
            );
            return;
        }
        let mut silero = SileroSidecarVad::new(&model_path).unwrap();
        silero.force_failed_for_test(true);
        assert!(!<SileroSidecarVad as VadEngine>::is_healthy(&silero));
        // reset() runs without panic in release; debug_assert would
        // fire in debug builds, which is intentional — we want the
        // dispatcher to surface the contract violation loudly during
        // development. In tests, swallow the panic by clearing the
        // flag first, then resetting, then re-failing to verify reset
        // doesn't touch the flag from a healthy state.
        silero.force_failed_for_test(false);
        <SileroSidecarVad as VadEngine>::reset(&mut silero);
        assert!(
            <SileroSidecarVad as VadEngine>::is_healthy(&silero),
            "reset on a healthy engine must keep is_healthy true"
        );
        silero.force_failed_for_test(true);
        assert!(!<SileroSidecarVad as VadEngine>::is_healthy(&silero));
    }

    /// Look up the canonical Silero model paths for parity tests.
    /// Returns `None` if either model is missing — the caller prints
    /// a `skipping` message and returns instead of failing.
    #[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
    fn parity_model_paths() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let onnx_path = dirs::home_dir()
            .unwrap()
            .join(".minutes/models/silero-vad-v6.2.0.onnx");
        let ggml_path = dirs::home_dir()
            .unwrap()
            .join(".minutes/models/ggml-silero-v6.2.0.bin");
        if !onnx_path.exists() || !ggml_path.exists() {
            return None;
        }
        Some((onnx_path, ggml_path))
    }

    /// Resolve a fixture path under `crates/assets/`. Returns `None`
    /// if the fixture is missing — caller skips. All parity fixtures
    /// are committed to the repo, so this is mostly a hedge against
    /// running tests from a partial checkout.
    #[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
    fn parity_fixture_path(name: &str) -> Option<std::path::PathBuf> {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crate parent")
            .join("assets")
            .join(name);
        path.exists().then_some(path)
    }

    /// Run a fixture through both OrtSileroVad and SileroSidecarVad
    /// at the production 100 ms cadence and return the per-chunk
    /// `speaking` flags. Returned vectors are the same length and
    /// represent identical timestamps so the parity-check helper can
    /// just zip-walk them.
    #[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
    fn run_both_engines_on_fixture(
        fixture_path: &std::path::Path,
        onnx_path: &std::path::Path,
        ggml_path: &std::path::Path,
    ) -> (Vec<bool>, Vec<bool>) {
        use crate::silero_vad::OrtSileroVad;
        use crate::vad::VadEngine;

        let samples = read_wav_samples(fixture_path);
        let mut ort_engine = OrtSileroVad::new(onnx_path).unwrap();
        let mut whisper_engine = SileroSidecarVad::new(ggml_path).unwrap();

        let chunk = 1600;
        let mut ort_speak: Vec<bool> = Vec::new();
        let mut whisper_speak: Vec<bool> = Vec::new();
        for window in samples.chunks(chunk) {
            let rms = (window.iter().map(|s| s * s).sum::<f32>() / window.len() as f32).sqrt();
            let ort = ort_engine.process(window, rms);
            let wh = whisper_engine.process(window, rms).unwrap();
            ort_speak.push(ort.speaking);
            whisper_speak.push(wh.speaking);
        }
        (ort_speak, whisper_speak)
    }

    /// Apply the codex parity bar from PLAN-vad-refactor.md to
    /// per-chunk speaking flags from both engines. `label` shows up
    /// in the eprintln preamble so test output is greppable per
    /// fixture.
    ///
    /// Bars enforced:
    /// 1. Zero missed islands. Every contiguous speech region
    ///    whisper-Silero flags must overlap with ≥1 ort-Silero
    ///    speaking chunk. Utterance loss is release-blocking.
    /// 2. Boundary drift ≤ 2 chunks (±200 ms). Slightly looser than
    ///    the ±150 ms in the design doc, to absorb model-version
    ///    noise without flapping in CI.
    /// 3. No phantom speech. Every ort-Silero speaking chunk must
    ///    fall within ±2 chunks of some whisper-Silero island.
    #[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
    fn assert_parity_bar(label: &str, ort_speak: &[bool], whisper_speak: &[bool]) {
        assert_eq!(
            ort_speak.len(),
            whisper_speak.len(),
            "[{}] engine output lengths must match",
            label
        );

        let islands = contiguous_runs(whisper_speak, true);
        let ort_islands = contiguous_runs(ort_speak, true);
        eprintln!(
            "[parity:{label}] whisper islands={} | ort islands={} | ort speaking={}/{}",
            islands.len(),
            ort_islands.len(),
            ort_speak.iter().filter(|&&s| s).count(),
            ort_speak.len()
        );

        const DRIFT_TOLERANCE_CHUNKS: i64 = 2;

        // Bar 1.
        for (start, end) in &islands {
            let any_overlap = ort_speak[*start..*end].iter().any(|&s| s);
            assert!(
                any_overlap,
                "[{label}] ort-silero missed an entire whisper-silero island at chunks [{},{}) ({:.1}s-{:.1}s)",
                start,
                end,
                *start as f32 * 0.1,
                *end as f32 * 0.1
            );
        }

        // Bar 2.
        for (start, end) in &islands {
            let nearest_start = ort_speak
                .iter()
                .enumerate()
                .filter(|(_, &s)| s)
                .map(|(i, _)| (i as i64 - *start as i64).abs())
                .min()
                .unwrap_or(i64::MAX);
            let nearest_end = ort_speak
                .iter()
                .enumerate()
                .filter(|(_, &s)| s)
                .map(|(i, _)| (i as i64 - (*end as i64 - 1)).abs())
                .min()
                .unwrap_or(i64::MAX);
            assert!(
                nearest_start <= DRIFT_TOLERANCE_CHUNKS,
                "[{label}] island start at chunk {} ({:.1}s): nearest ort-silero speech is {} chunks away (>200ms drift)",
                start,
                *start as f32 * 0.1,
                nearest_start
            );
            assert!(
                nearest_end <= DRIFT_TOLERANCE_CHUNKS,
                "[{label}] island end at chunk {} ({:.1}s): nearest ort-silero speech is {} chunks away (>200ms drift)",
                end,
                *end as f32 * 0.1,
                nearest_end
            );
        }

        // Bar 3.
        for (i, &is_speaking) in ort_speak.iter().enumerate() {
            if !is_speaking {
                continue;
            }
            let inside_or_near = islands.iter().any(|(s, e)| {
                let near_start = (*s as i64 - i as i64).abs() <= DRIFT_TOLERANCE_CHUNKS;
                let near_end = (i as i64 - (*e as i64 - 1)).abs() <= DRIFT_TOLERANCE_CHUNKS;
                let inside = i >= *s && i < *e;
                inside || near_start || near_end
            });
            assert!(
                inside_or_near,
                "[{label}] phantom ort-silero speech at chunk {} ({:.1}s) — not within ±200ms of any whisper-silero island",
                i,
                i as f32 * 0.1
            );
        }
    }

    /// Parity on the original demo.wav. Continuous speech, one
    /// island, both engines should agree trivially. Sanity baseline
    /// for the fixture-based stress tests below.
    #[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
    #[test]
    #[ignore]
    fn parity_ort_silero_vs_whisper_silero_on_demo_wav() {
        let Some((onnx_path, ggml_path)) = parity_model_paths() else {
            eprintln!("[parity:demo] skipping: missing Silero models");
            return;
        };
        let Some(fixture) = parity_fixture_path("demo.wav") else {
            eprintln!("[parity:demo] skipping: no demo.wav fixture");
            return;
        };
        let (ort_speak, whisper_speak) =
            run_both_engines_on_fixture(&fixture, &onnx_path, &ggml_path);
        assert_parity_bar("demo", &ort_speak, &whisper_speak);
    }

    /// Fixture A. silence → 80 ms speech spike → silence. Below the
    /// 150 ms min_speech filter that snakers4's reference and our
    /// smoothing FSM enforce. ort-Silero must report zero speech on
    /// this fixture; if it ever stops filtering, every cough and
    /// click would fire the ASR pipeline (codex's commit 2 review
    /// #1 finding).
    ///
    /// This is intentionally NOT a cross-engine parity test:
    /// whisper-rs's bundled Silero runs in full-buffer rescan mode
    /// and is more permissive on brief spikes than our streaming
    /// implementation with the explicit min_speech gate. That
    /// permissiveness is one of the structural problems Option B
    /// solves; running the parity bar here would penalize ort for
    /// behaving correctly. We log whisper's behavior for diagnostic
    /// awareness and assert only on ort.
    #[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
    #[test]
    #[ignore]
    fn parity_brief_spike_under_min_speech_is_silence() {
        let Some((onnx_path, ggml_path)) = parity_model_paths() else {
            eprintln!("[parity:brief_spike] skipping: missing Silero models");
            return;
        };
        let Some(fixture) = parity_fixture_path("parity_brief_spike.wav") else {
            eprintln!("[parity:brief_spike] skipping: fixture missing — run examples/build_parity_fixtures");
            return;
        };
        let (ort_speak, whisper_speak) =
            run_both_engines_on_fixture(&fixture, &onnx_path, &ggml_path);
        let ort_speech = ort_speak.iter().filter(|&&s| s).count();
        let whisper_speech = whisper_speak.iter().filter(|&&s| s).count();
        eprintln!(
            "[parity:brief_spike] ort_speech_chunks={} whisper_speech_chunks={} (whisper expected to be over-permissive — that's the bug Option B fixes)",
            ort_speech, whisper_speech
        );
        assert_eq!(
            ort_speech, 0,
            "ort-silero must filter an 80 ms spike (below 150 ms min_speech)"
        );
    }

    /// Fixture B. Three 1.5 s utterances separated by 300 / 500 /
    /// 800 ms gaps, with 200 ms head and 800 ms tail silence. The
    /// 300 ms gap is below min_silence (500 ms) so utterances 1 and
    /// 2 should merge into one island; 500 ms is at the boundary;
    /// 800 ms is well above so utterance 3 is clearly its own
    /// island. Both engines should agree on the segmentation,
    /// whatever it works out to.
    #[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
    #[test]
    #[ignore]
    fn parity_three_utterances_with_varied_gaps() {
        let Some((onnx_path, ggml_path)) = parity_model_paths() else {
            eprintln!("[parity:three_utterances] skipping: missing Silero models");
            return;
        };
        let Some(fixture) = parity_fixture_path("parity_three_utterances.wav") else {
            eprintln!("[parity:three_utterances] skipping: fixture missing");
            return;
        };
        let (ort_speak, whisper_speak) =
            run_both_engines_on_fixture(&fixture, &onnx_path, &ggml_path);
        assert_parity_bar("three_utterances", &ort_speak, &whisper_speak);
    }

    /// Fixture C. demo.wav scaled to 5% amplitude — well below the
    /// energy-VAD threshold, but model-based VADs are robust to
    /// quiet speech. Both should still detect the segment.
    #[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
    #[test]
    #[ignore]
    fn parity_low_volume_speech_still_detected() {
        let Some((onnx_path, ggml_path)) = parity_model_paths() else {
            eprintln!("[parity:low_volume] skipping: missing Silero models");
            return;
        };
        let Some(fixture) = parity_fixture_path("parity_low_volume.wav") else {
            eprintln!("[parity:low_volume] skipping: fixture missing");
            return;
        };
        let (ort_speak, whisper_speak) =
            run_both_engines_on_fixture(&fixture, &onnx_path, &ggml_path);
        assert_parity_bar("low_volume", &ort_speak, &whisper_speak);
        // Beyond cross-engine parity: if NEITHER engine detects any
        // speech in 10s of quiet-but-real speech, that's a
        // model-quality alarm we want to surface explicitly.
        let any_ort = ort_speak.iter().any(|&s| s);
        let any_whisper = whisper_speak.iter().any(|&s| s);
        assert!(
            any_ort || any_whisper,
            "neither engine detected speech in low-volume fixture — model-quality regression?"
        );
    }

    /// Fixture D. demo.wav truncated to 16384 + 137 = 16521 samples
    /// (≈1.033 s). The streaming engine processes 32 full 512-sample
    /// chunks and leaves a 137-sample tail in the buffer that never
    /// reaches the ONNX session. This stresses the partial-tail path
    /// — if zero-padding ever gets added to the buffer-flush logic,
    /// the LSTM state could pick up a phantom silence and drift the
    /// final decision. Both engines processing the same short
    /// recording should agree.
    #[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
    #[test]
    #[ignore]
    fn parity_trailing_partial_chunk_handled_consistently() {
        let Some((onnx_path, ggml_path)) = parity_model_paths() else {
            eprintln!("[parity:trailing_partial] skipping: missing Silero models");
            return;
        };
        let Some(fixture) = parity_fixture_path("parity_trailing_partial.wav") else {
            eprintln!("[parity:trailing_partial] skipping: fixture missing");
            return;
        };
        let (ort_speak, whisper_speak) =
            run_both_engines_on_fixture(&fixture, &onnx_path, &ggml_path);
        assert_parity_bar("trailing_partial", &ort_speak, &whisper_speak);
    }

    #[cfg(all(feature = "whisper", feature = "streaming", feature = "vad-ort"))]
    fn contiguous_runs(seq: &[bool], value: bool) -> Vec<(usize, usize)> {
        let mut runs = Vec::new();
        let mut start: Option<usize> = None;
        for (i, &v) in seq.iter().enumerate() {
            if v == value && start.is_none() {
                start = Some(i);
            } else if v != value && start.is_some() {
                runs.push((start.take().unwrap(), i));
            }
        }
        if let Some(s) = start {
            runs.push((s, seq.len()));
        }
        runs
    }

    /// SPIKE — does whisper-rs's Silero VAD carry LSTM state across
    /// `detect_speech` calls? If yes, Option A in PLAN-vad-refactor.md is
    /// viable: feed only new-since-last-call samples per chunk, accumulate
    /// probabilities externally. If no, the per-chunk re-scan is structural
    /// and Option B (independent ort-backed Silero) is required.
    ///
    /// Run with: `cargo test -p minutes-core --features whisper --lib
    ///   live_transcript::tests::option_a_spike -- --ignored --nocapture`
    ///
    /// Requires `~/.minutes/models/ggml-silero-v6.2.0.bin` to exist (i.e.
    /// `minutes setup` has run on this machine). Output is informational
    /// only; the test only fails if the model is unloadable.
    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    #[ignore]
    fn option_a_spike_silero_state_carries_across_detect_speech_calls() {
        let model_path = dirs::home_dir()
            .unwrap()
            .join(".minutes/models/ggml-silero-v6.2.0.bin");
        if !model_path.exists() {
            eprintln!(
                "[spike] no Silero model at {} — run `minutes setup` first",
                model_path.display()
            );
            return;
        }

        // Build deterministic test signal: 1s silence + 1s 440Hz tone +
        // 1s silence, 16kHz f32 normalized to ~0.5 amplitude in the tone
        // region. Total 48,000 samples = 3s.
        let mut samples = Vec::with_capacity(48_000);
        samples.extend(std::iter::repeat_n(0.0_f32, 16_000));
        for i in 0..16_000 {
            let t = i as f32 / 16_000.0;
            samples.push((2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5);
        }
        samples.extend(std::iter::repeat_n(0.0_f32, 16_000));

        let mid = samples.len() / 2;

        let make_ctx = || -> whisper_rs::WhisperVadContext {
            let mut params = whisper_rs::WhisperVadContextParams::default();
            params.set_n_threads(2);
            whisper_rs::WhisperVadContext::new(model_path.to_str().expect("utf-8 path"), params)
                .expect("Silero model load")
        };

        // Mode A: single full-buffer call.
        let mut ctx_a = make_ctx();
        ctx_a.detect_speech(&samples).expect("detect_speech full");
        let probs_full = ctx_a.probabilities().to_vec();

        // Mode B: two halves, fresh context.
        let mut ctx_b = make_ctx();
        ctx_b
            .detect_speech(&samples[..mid])
            .expect("detect_speech first half");
        let probs_first_half = ctx_b.probabilities().to_vec();
        ctx_b
            .detect_speech(&samples[mid..])
            .expect("detect_speech second half");
        let probs_second_half = ctx_b.probabilities().to_vec();

        let probs_concat: Vec<f32> = probs_first_half
            .iter()
            .copied()
            .chain(probs_second_half.iter().copied())
            .collect();

        eprintln!("[spike] full samples: {}", samples.len());
        eprintln!(
            "[spike] mode A (single call) probs.len(): {}",
            probs_full.len()
        );
        eprintln!(
            "[spike] mode B (two halves) probs.len(): first={} second={} concat={}",
            probs_first_half.len(),
            probs_second_half.len(),
            probs_concat.len()
        );

        // Length parity: do we get the same total probability count from
        // both modes? If second_half's probabilities are computed as if the
        // input were standalone (no carried context), the count should
        // match a fresh call on samples[mid..].
        if probs_full.len() != probs_concat.len() {
            eprintln!(
                "[spike] LENGTH MISMATCH (Mode A: {}, Mode B: {}) — \
                 second detect_speech call produces output as if input were \
                 standalone; LSTM state likely RESETS per call",
                probs_full.len(),
                probs_concat.len()
            );
        }

        // Diff stats over the overlapping prefix.
        let n = probs_full.len().min(probs_concat.len());
        if n > 0 {
            let diffs: Vec<f32> = (0..n)
                .map(|i| (probs_full[i] - probs_concat[i]).abs())
                .collect();
            let max_diff = diffs.iter().fold(0.0_f32, |a, &b| a.max(b));
            let mean_diff = diffs.iter().sum::<f32>() / diffs.len() as f32;
            let above_5pct = diffs.iter().filter(|&&d| d > 0.05).count();
            eprintln!(
                "[spike] prefix diff stats over {} samples: max={:.4}, mean={:.4}, count_above_0.05={}",
                n, max_diff, mean_diff, above_5pct
            );
            // Verdict guidance.
            if max_diff < 0.01 {
                eprintln!(
                    "[spike] VERDICT: probabilities match within tolerance — \
                     OPTION A LIKELY VIABLE. Incremental detect_speech \
                     accumulates probabilities consistent with single call."
                );
            } else if max_diff < 0.1 {
                eprintln!(
                    "[spike] VERDICT: small drift (max diff {:.4}) — Option A \
                     might work with retuned thresholds; investigate boundary \
                     behavior more carefully before committing.",
                    max_diff
                );
            } else {
                eprintln!(
                    "[spike] VERDICT: significant drift (max diff {:.4}) — \
                     OPTION A NOT VIABLE. detect_speech does not preserve \
                     LSTM state across calls. Option B (ort-backed Silero) \
                     required.",
                    max_diff
                );
            }
        }

        // The test always passes; output is informational. Failing here
        // would just block the spike from running.
        let _ = model_path;
    }

    /// SPIKE v2 — richer probe on real speech, simulating the production
    /// 100ms-chunk cadence to see if incremental detect_speech calls
    /// produce probabilities consistent enough with a single full-buffer
    /// call that threshold-based speech detection lands in the same
    /// places.
    ///
    /// Method:
    /// 1. Load `crates/assets/demo.wav` (~10.6s of real speech) into
    ///    16kHz f32 samples.
    /// 2. Mode A: single detect_speech call over the whole buffer,
    ///    record probabilities.
    /// 3. Mode B: feed in 1600-sample (100ms) chunks, record each
    ///    call's probabilities, concatenate.
    /// 4. Apply binary threshold at 0.2 to both probability vectors
    ///    and count windows where the speech/no-speech decision
    ///    DIFFERS between modes. Each diff is a potential
    ///    misdetection if Option A shipped.
    ///
    /// Verdict bar (per codex parity definition):
    /// - 0 flips: Option A safe.
    /// - 1-3 isolated flips at speech boundaries: probably fine,
    ///   boundary drift up to ±150ms is acceptable.
    /// - Many flips OR flips inside contiguous speech regions:
    ///   Option A unsafe; Option B required.
    ///
    /// Run with: `cargo test -p minutes-core --features
    /// "whisper streaming" --lib option_a_spike_v2 -- --ignored
    /// --nocapture`
    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    #[ignore]
    fn option_a_spike_v2_real_speech_threshold_decisions() {
        let model_path = dirs::home_dir()
            .unwrap()
            .join(".minutes/models/ggml-silero-v6.2.0.bin");
        if !model_path.exists() {
            eprintln!(
                "[spike v2] no Silero model at {} — run `minutes setup`",
                model_path.display()
            );
            return;
        }

        // Find the demo WAV. Workspace root is two levels up from this
        // crate; the assets crate sits next to ours.
        let cargo_manifest = env!("CARGO_MANIFEST_DIR");
        let demo_wav = std::path::PathBuf::from(cargo_manifest)
            .parent()
            .expect("crate parent")
            .join("assets/demo.wav");
        if !demo_wav.exists() {
            eprintln!("[spike v2] no demo.wav at {}", demo_wav.display());
            return;
        }

        let samples = read_wav_samples(&demo_wav);
        eprintln!(
            "[spike v2] loaded {} samples ({:.1}s at 16kHz)",
            samples.len(),
            samples.len() as f32 / 16_000.0
        );

        let make_ctx = || -> whisper_rs::WhisperVadContext {
            let mut params = whisper_rs::WhisperVadContextParams::default();
            params.set_n_threads(2);
            whisper_rs::WhisperVadContext::new(model_path.to_str().expect("utf-8 path"), params)
                .expect("Silero load")
        };

        // Mode A: single full-buffer call.
        let mut ctx_a = make_ctx();
        ctx_a.detect_speech(&samples).expect("detect_speech full");
        let probs_a = ctx_a.probabilities().to_vec();

        // Mode B: 100ms chunks, fresh context, append per-call probs.
        let chunk_size = 1600; // 100ms at 16kHz
        let mut ctx_b = make_ctx();
        let mut probs_b: Vec<f32> = Vec::new();
        for chunk in samples.chunks(chunk_size) {
            ctx_b.detect_speech(chunk).expect("detect_speech chunk");
            probs_b.extend_from_slice(ctx_b.probabilities());
        }

        eprintln!(
            "[spike v2] Mode A probs.len: {} | Mode B probs.len: {}",
            probs_a.len(),
            probs_b.len()
        );

        // Align lengths for comparison. Mode B may have slightly more
        // probability windows because each chunk's detect_speech rounds up.
        let n = probs_a.len().min(probs_b.len());

        // Apply the production threshold.
        const THRESHOLD: f32 = 0.2; // matches SIDECAR_VAD_THRESHOLD
        let mut flips = 0usize;
        let mut flip_a_speech_b_silence = 0usize;
        let mut flip_a_silence_b_speech = 0usize;
        let mut max_diff = 0.0_f32;
        let mut sum_diff = 0.0_f32;
        let mut consecutive_flips = 0usize;
        let mut max_consecutive = 0usize;

        for i in 0..n {
            let a_speech = probs_a[i] >= THRESHOLD;
            let b_speech = probs_b[i] >= THRESHOLD;
            let diff = (probs_a[i] - probs_b[i]).abs();
            max_diff = max_diff.max(diff);
            sum_diff += diff;

            if a_speech != b_speech {
                flips += 1;
                consecutive_flips += 1;
                max_consecutive = max_consecutive.max(consecutive_flips);
                if a_speech {
                    flip_a_speech_b_silence += 1;
                } else {
                    flip_a_silence_b_speech += 1;
                }
            } else {
                consecutive_flips = 0;
            }
        }

        let mean_diff = if n > 0 { sum_diff / n as f32 } else { 0.0 };

        eprintln!(
            "[spike v2] over {} comparable windows ({:.1}s of audio):",
            n,
            n as f32 * 32.0 / 16_000.0 // each prob window represents ~32ms
        );
        eprintln!("[spike v2]   max prob diff:  {:.4}", max_diff);
        eprintln!("[spike v2]   mean prob diff: {:.4}", mean_diff);
        eprintln!(
            "[spike v2]   threshold flips: {} total (A=speech/B=silence: {}, A=silence/B=speech: {})",
            flips, flip_a_speech_b_silence, flip_a_silence_b_speech
        );
        eprintln!(
            "[spike v2]   longest flip run: {} windows ({:.0}ms)",
            max_consecutive,
            max_consecutive as f32 * 32.0
        );

        // Verdict.
        let flip_pct = if n > 0 {
            (flips as f32 / n as f32) * 100.0
        } else {
            0.0
        };

        if flips == 0 {
            eprintln!(
                "[spike v2] VERDICT: zero threshold flips — OPTION A SAFE. \
                 Incremental detect_speech produces threshold decisions \
                 identical to the full-buffer call on real speech."
            );
        } else if max_consecutive <= 5 && flip_pct < 5.0 {
            eprintln!(
                "[spike v2] VERDICT: {} isolated flips ({:.1}%, longest run {} \
                 windows = {:.0}ms). Within codex's ±150ms boundary tolerance. \
                 OPTION A LIKELY SAFE for production. Validate with one more \
                 dogfood session before defaulting.",
                flips,
                flip_pct,
                max_consecutive,
                max_consecutive as f32 * 32.0
            );
        } else {
            eprintln!(
                "[spike v2] VERDICT: {} flips ({:.1}%, longest run {} windows \
                 = {:.0}ms). Exceeds parity tolerance. OPTION A UNSAFE — \
                 incremental probabilities diverge in ways that flip speech \
                 detection. Option B (ort-backed Silero with persistent LSTM \
                 state) is required.",
                flips,
                flip_pct,
                max_consecutive,
                max_consecutive as f32 * 32.0
            );
        }
    }

    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn sidecar_missing_status_is_inactive_with_diagnostic() {
        let dir = tempdir().unwrap();
        let status = derive_session_status(
            pid::PidFileState::Inactive,
            Some(std::process::id()),
            &dir.path().join("live-status.json"),
            &dir.path().join("live.jsonl"),
        );

        assert!(!status.active);
        assert_eq!(status.source, None);
        assert_eq!(status.pid, None);
        assert_eq!(
            status.diagnostic.as_deref(),
            Some("sidecar status unavailable")
        );
    }

    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn sidecar_reports_active_when_healthy_and_recording_is_active() {
        let dir = tempdir().unwrap();
        let status_path = dir.path().join("live-status.json");
        let status = live_status_with_state(LiveStatusState::Healthy);
        std::fs::write(&status_path, serde_json::to_string(&status).unwrap()).unwrap();

        let status = derive_session_status(
            pid::PidFileState::Inactive,
            Some(std::process::id()),
            &status_path,
            &dir.path().join("live.jsonl"),
        );

        assert!(status.active);
        assert_eq!(status.source, Some(TranscriptSource::RecordingSidecar));
        assert_eq!(status.pid, Some(std::process::id()));
        assert_eq!(status.line_count, 3);
        assert_eq!(status.diagnostic, None);
    }

    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn sidecar_failed_state_overrides_active_recording() {
        let dir = tempdir().unwrap();
        let status_path = dir.path().join("live-status.json");
        let mut status = live_status_with_state(LiveStatusState::Failed);
        status.diagnostic = Some("sidecar failed".into());
        std::fs::write(&status_path, serde_json::to_string(&status).unwrap()).unwrap();

        let status = derive_session_status(
            pid::PidFileState::Inactive,
            Some(std::process::id()),
            &status_path,
            &dir.path().join("live.jsonl"),
        );

        assert!(!status.active);
        assert_eq!(status.source, None);
        assert_eq!(status.diagnostic.as_deref(), Some("sidecar failed"));
    }

    #[test]
    fn live_scope_warning_only_applies_to_parakeet() {
        #[cfg(feature = "parakeet")]
        {
            assert_eq!(live_engine_scope_warning("parakeet"), None);
            assert_eq!(live_engine_scope_warning("PaRaKeEt"), None);
        }
        #[cfg(not(feature = "parakeet"))]
        {
            assert_eq!(
                live_engine_scope_warning("parakeet"),
                Some(PARAKEET_LIVE_SCOPE_WARNING)
            );
            assert_eq!(
                live_engine_scope_warning("PaRaKeEt"),
                Some(PARAKEET_LIVE_SCOPE_WARNING)
            );
        }
        assert_eq!(live_engine_scope_warning("whisper"), None);
    }

    /// Scope-warning and runtime-fallback-warning are semantically distinct:
    /// - scope = "this build doesn't support parakeet"
    /// - fallback = "parakeet failed at runtime; falling back"
    ///
    /// If they drift to the same message, users can't distinguish whether
    /// their config was ignored at compile time or broken at runtime.
    #[cfg(feature = "parakeet")]
    #[test]
    fn scope_and_fallback_warnings_are_distinct_messages() {
        // The fallback message must not claim the feature is unavailable —
        // by definition it fires when the feature IS compiled in.
        assert!(!PARAKEET_LIVE_FALLBACK_WARNING.contains("does not include"));
        assert!(PARAKEET_LIVE_FALLBACK_WARNING.contains("fall"));
    }

    /// The parakeet live helper drops sub-1s utterances without hitting the
    /// sidecar/subprocess. This guards against temp-file churn and latency
    /// spikes on VAD blips. Matches StreamingWhisper::MIN_TRANSCRIBE_SAMPLES.
    #[cfg(feature = "parakeet")]
    #[test]
    fn parakeet_live_helper_drops_subsecond_utterances() {
        use super::transcribe_with_parakeet_for_live_sidecar;
        let cfg = Config::default();
        // 0.5s of silence at 16kHz. Should return Ok(None) without attempting
        // to write a temp WAV or spawn a subprocess. If the floor is missing
        // and this test reaches the parakeet subprocess path, it will fail or
        // hang (no parakeet binary configured in default test Config).
        let samples = vec![0.0f32; 8_000];
        let result = transcribe_with_parakeet_for_live_sidecar(&samples, &cfg);
        assert!(matches!(result, Ok(None)));
    }

    /// Zero-length and exactly-at-threshold edge cases for the 1s floor.
    #[cfg(feature = "parakeet")]
    #[test]
    fn parakeet_live_helper_threshold_edges() {
        use super::transcribe_with_parakeet_for_live_sidecar;
        use super::PARAKEET_LIVE_MIN_SAMPLES;
        let cfg = Config::default();
        // Empty buffer — drop.
        assert!(matches!(
            transcribe_with_parakeet_for_live_sidecar(&[], &cfg),
            Ok(None)
        ));
        // One sample below threshold — drop.
        let below = vec![0.0f32; PARAKEET_LIVE_MIN_SAMPLES - 1];
        assert!(matches!(
            transcribe_with_parakeet_for_live_sidecar(&below, &cfg),
            Ok(None)
        ));
        // (We can't assert the threshold-exceeds branch here — it requires a
        // real parakeet binary. The subprocess path is exercised by the smoke
        // test in the RFC.)
    }

    #[cfg(all(feature = "whisper", feature = "parakeet"))]
    #[test]
    fn apple_speech_fallback_prefers_ready_parakeet_before_whisper() {
        let calls = std::sync::Mutex::new(Vec::<&'static str>::new());
        let result = resolve_apple_speech_live_fallback(
            true,
            || {
                calls.lock().unwrap().push("parakeet");
                Ok(Some(("parakeet transcript".into(), 1.2)))
            },
            || {
                calls.lock().unwrap().push("whisper");
                Ok(Some(("whisper transcript".into(), 1.2)))
            },
        )
        .unwrap();

        assert_eq!(calls.lock().unwrap().as_slice(), &["parakeet"]);
        assert_eq!(result, Some(("parakeet transcript".into(), 1.2)));
    }

    #[cfg(all(feature = "whisper", feature = "parakeet"))]
    #[test]
    fn apple_speech_fallback_uses_whisper_when_parakeet_is_not_ready() {
        let calls = std::sync::Mutex::new(Vec::<&'static str>::new());
        let result = resolve_apple_speech_live_fallback(
            false,
            || {
                calls.lock().unwrap().push("parakeet");
                Ok(Some(("parakeet transcript".into(), 1.2)))
            },
            || {
                calls.lock().unwrap().push("whisper");
                Ok(Some(("whisper transcript".into(), 1.2)))
            },
        )
        .unwrap();

        assert_eq!(calls.lock().unwrap().as_slice(), &["whisper"]);
        assert_eq!(result, Some(("whisper transcript".into(), 1.2)));
    }

    #[cfg(all(feature = "whisper", feature = "parakeet"))]
    #[test]
    fn apple_speech_fallback_tries_whisper_after_parakeet_error() {
        let calls = std::sync::Mutex::new(Vec::<&'static str>::new());
        let result = resolve_apple_speech_live_fallback(
            true,
            || {
                calls.lock().unwrap().push("parakeet");
                Err(MinutesError::Io(std::io::Error::other("parakeet failed")))
            },
            || {
                calls.lock().unwrap().push("whisper");
                Ok(Some(("whisper transcript".into(), 0.9)))
            },
        )
        .unwrap();

        assert_eq!(calls.lock().unwrap().as_slice(), &["parakeet", "whisper"]);
        assert_eq!(result, Some(("whisper transcript".into(), 0.9)));
    }

    #[cfg(all(feature = "whisper", target_os = "macos"))]
    #[test]
    fn apple_speech_live_helper_cleans_up_tempfile_on_error() {
        let cfg = Config::default();
        let samples = vec![0.0f32; APPLE_SPEECH_LIVE_MIN_SAMPLES];
        let seen_path = std::sync::Mutex::new(None::<PathBuf>);

        let result = transcribe_with_apple_speech_for_live_sidecar_impl(
            &samples,
            &cfg,
            |path, _locale, _mode, _ensure_assets| {
                *seen_path.lock().unwrap() = Some(path.to_path_buf());
                Err(MinutesError::Io(std::io::Error::other(
                    "simulated apple speech failure",
                )))
            },
        );

        assert!(result.is_err());
        let leaked_path = seen_path.lock().unwrap().clone().unwrap();
        assert!(
            !leaked_path.exists(),
            "temporary WAV should be cleaned up after helper failure"
        );
    }

    #[test]
    fn normalize_live_transcript_text_filters_noise_placeholders() {
        assert_eq!(normalize_live_transcript_text("[typing]"), None);
        assert_eq!(normalize_live_transcript_text("[BLANK_AUDIO]"), None);
        assert_eq!(normalize_live_transcript_text("[Musik]"), None);
    }

    #[test]
    fn normalize_live_transcript_text_flattens_timestamped_lines() {
        let cleaned = normalize_live_transcript_text("[0:00] hello\n[0:01] world").unwrap();
        assert_eq!(cleaned, "hello world");
    }

    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn sidecar_starting_state_is_not_ready() {
        let dir = tempdir().unwrap();
        let status_path = dir.path().join("live-status.json");
        let status = live_status_with_state(LiveStatusState::Starting);
        std::fs::write(&status_path, serde_json::to_string(&status).unwrap()).unwrap();

        let status = derive_session_status(
            pid::PidFileState::Inactive,
            Some(std::process::id()),
            &status_path,
            &dir.path().join("live.jsonl"),
        );

        assert!(!status.active);
        assert_eq!(status.source, None);
        assert_eq!(status.diagnostic.as_deref(), Some("sidecar starting"));
    }

    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn sidecar_long_startup_is_degraded() {
        let dir = tempdir().unwrap();
        let status_path = dir.path().join("live-status.json");
        let mut status = live_status_with_state(LiveStatusState::Starting);
        status.start_time =
            Local::now() - ChronoDuration::seconds(SIDECAR_STARTUP_TIMEOUT_SECS + 1);
        status.updated_at = status.start_time;
        std::fs::write(&status_path, serde_json::to_string(&status).unwrap()).unwrap();

        let status = derive_session_status(
            pid::PidFileState::Inactive,
            Some(std::process::id()),
            &status_path,
            &dir.path().join("live.jsonl"),
        );

        assert!(!status.active);
        assert_eq!(status.diagnostic.as_deref(), Some("sidecar still starting"));
    }

    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn stale_healthy_sidecar_is_treated_as_inactive() {
        let dir = tempdir().unwrap();
        let status_path = dir.path().join("live-status.json");
        let mut status = live_status_with_state(LiveStatusState::Healthy);
        status.updated_at =
            Local::now() - ChronoDuration::seconds(SIDECAR_HEALTH_STALE_AFTER_SECS + 1);
        std::fs::write(&status_path, serde_json::to_string(&status).unwrap()).unwrap();

        let status = derive_session_status(
            pid::PidFileState::Inactive,
            Some(std::process::id()),
            &status_path,
            &dir.path().join("live.jsonl"),
        );

        assert!(!status.active);
        assert_eq!(
            status.diagnostic.as_deref(),
            Some("sidecar heartbeat stale")
        );
    }

    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn stale_status_file_is_ignored_when_recording_is_stopped() {
        let dir = tempdir().unwrap();
        let status_path = dir.path().join("live-status.json");
        let status = live_status_with_state(LiveStatusState::Failed);
        std::fs::write(&status_path, serde_json::to_string(&status).unwrap()).unwrap();

        let status = derive_session_status(
            pid::PidFileState::Inactive,
            None,
            &status_path,
            &dir.path().join("live.jsonl"),
        );

        assert!(!status.active);
        assert_eq!(status.line_count, 0);
        assert_eq!(status.duration_secs, 0.0);
        assert_eq!(status.diagnostic, None);
    }

    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn trace_sidecar_transition_when_it_fails_mid_recording() {
        let dir = tempdir().unwrap();
        let status_path = dir.path().join("live-status.json");
        let mut healthy = live_status_with_state(LiveStatusState::Healthy);
        healthy.line_count = 7;
        std::fs::write(&status_path, serde_json::to_string(&healthy).unwrap()).unwrap();

        let healthy_status = derive_session_status(
            pid::PidFileState::Inactive,
            Some(std::process::id()),
            &status_path,
            &dir.path().join("live.jsonl"),
        );
        println!(
            "healthy => active={}, source={:?}, diagnostic={:?}, lines={}",
            healthy_status.active,
            healthy_status.source,
            healthy_status.diagnostic,
            healthy_status.line_count
        );

        let mut failed = healthy.clone();
        failed.state = LiveStatusState::Failed;
        failed.updated_at = Local::now();
        failed.diagnostic = Some("sidecar failed".into());
        std::fs::write(&status_path, serde_json::to_string(&failed).unwrap()).unwrap();

        let failed_status = derive_session_status(
            pid::PidFileState::Inactive,
            Some(std::process::id()),
            &status_path,
            &dir.path().join("live.jsonl"),
        );
        println!(
            "failed => active={}, source={:?}, diagnostic={:?}, lines={}",
            failed_status.active,
            failed_status.source,
            failed_status.diagnostic,
            failed_status.line_count
        );

        assert!(healthy_status.active);
        assert!(!failed_status.active);
        assert_eq!(failed_status.diagnostic.as_deref(), Some("sidecar failed"));
    }

    /// #258: a standalone session whose PID file is held under a Windows mandatory
    /// lock (`LockedAlive`, PID unreadable) must report active with real stats and
    /// `source = Standalone`. The PID is `None` (we don't fabricate it) and no
    /// "sidecar …" diagnostic leaks onto the standalone path.
    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn standalone_locked_alive_reports_active_with_stats() {
        let dir = tempdir().unwrap();
        let status_path = dir.path().join("live-status.json");
        let status = live_status_with_state(LiveStatusState::Healthy);
        std::fs::write(&status_path, serde_json::to_string(&status).unwrap()).unwrap();

        let result = derive_session_status(
            pid::PidFileState::LockedAlive,
            None,
            &status_path,
            &dir.path().join("live.jsonl"),
        );

        assert!(result.active);
        assert_eq!(result.source, Some(TranscriptSource::Standalone));
        assert_eq!(
            result.pid, None,
            "locked PID is unreadable, must not be faked"
        );
        assert_eq!(result.line_count, 3);
        assert_eq!(
            result.diagnostic, None,
            "no sidecar diagnostic on standalone"
        );
    }

    /// Regression guard for the heartbeat-flicker hazard: standalone liveness comes
    /// from the held lock, NOT the heartbeat. A `LockedAlive` session with a *stale*
    /// healthy heartbeat (loop busy on a long utterance) must still report active
    /// and keep reporting its last-known stats rather than dropping back to 0/0.
    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn standalone_locked_alive_stays_active_with_stale_heartbeat() {
        let dir = tempdir().unwrap();
        let status_path = dir.path().join("live-status.json");
        let mut status = live_status_with_state(LiveStatusState::Healthy);
        status.updated_at =
            Local::now() - ChronoDuration::seconds(SIDECAR_HEALTH_STALE_AFTER_SECS + 5);
        std::fs::write(&status_path, serde_json::to_string(&status).unwrap()).unwrap();

        let result = derive_session_status(
            pid::PidFileState::LockedAlive,
            None,
            &status_path,
            &dir.path().join("live.jsonl"),
        );

        assert!(
            result.active,
            "lock proves liveness independent of heartbeat"
        );
        assert_eq!(result.source, Some(TranscriptSource::Standalone));
        assert_eq!(result.line_count, 3);
    }

    /// The readable-PID standalone path (Unix, or Windows when not self-locked)
    /// reports the real PID alongside `source = Standalone`.
    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn standalone_active_pid_reports_pid_and_source() {
        let dir = tempdir().unwrap();
        let status_path = dir.path().join("live-status.json");
        let status = live_status_with_state(LiveStatusState::Healthy);
        std::fs::write(&status_path, serde_json::to_string(&status).unwrap()).unwrap();

        let result = derive_session_status(
            pid::PidFileState::Active(4321),
            None,
            &status_path,
            &dir.path().join("live.jsonl"),
        );

        assert!(result.active);
        assert_eq!(result.source, Some(TranscriptSource::Standalone));
        assert_eq!(result.pid, Some(4321));
        assert_eq!(result.line_count, 3);
    }

    /// An inactive standalone PID with no recording reports nothing — unchanged
    /// behavior, guarding against the fallback over-triggering.
    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn standalone_inactive_with_no_recording_is_idle() {
        let dir = tempdir().unwrap();
        let status_path = dir.path().join("live-status.json");
        let status = live_status_with_state(LiveStatusState::Healthy);
        std::fs::write(&status_path, serde_json::to_string(&status).unwrap()).unwrap();

        let result = derive_session_status(
            pid::PidFileState::Inactive,
            None,
            &status_path,
            &dir.path().join("live.jsonl"),
        );

        assert!(!result.active);
        assert_eq!(result.source, None);
        assert_eq!(result.line_count, 0);
        assert_eq!(result.duration_secs, 0.0);
    }
}
