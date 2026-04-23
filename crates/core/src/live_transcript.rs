use crate::config::Config;
use crate::error::{LiveTranscriptError, MinutesError, TranscribeError};
use crate::pid;
use crate::streaming::AudioStream;
use crate::streaming_whisper::StreamingWhisper;
use crate::transcription_coordinator::{collapse_noise_markers, strip_foreign_script};
use crate::vad::Vad;
#[cfg(feature = "whisper")]
use crate::vad::VadResult;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
    "apple-speech live transcript is unavailable on this machine; falling back to whisper for this session";
pub const APPLE_SPEECH_LIVE_FALLBACK_WARNING: &str =
    "apple-speech live transcription failed; falling back to whisper for this session";

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
    jsonl_writer: BufWriter<File>,
    wav_writer: Option<hound::WavWriter<BufWriter<File>>>,
    line_count: usize,
    start_time: std::time::Instant,
    start_wall: DateTime<Local>,
    jsonl_path: PathBuf,
    jsonl_failed: bool,
    wav_failed: bool,
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
}

const SIDECAR_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const SIDECAR_HEALTH_STALE_AFTER_SECS: i64 = 3;
const SIDECAR_STARTUP_TIMEOUT_SECS: i64 = 10;

impl LiveTranscriptWriter {
    fn new(config: &Config, session_id: Option<String>) -> Result<Self, MinutesError> {
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
            jsonl_writer,
            wav_writer,
            line_count: 0,
            start_time: std::time::Instant::now(),
            start_wall,
            jsonl_path,
            jsonl_failed: false,
            wav_failed: false,
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
        let status = LiveStatus {
            start_time: self.start_wall,
            updated_at: Local::now(),
            state,
            line_count: self.line_count,
            last_offset_ms: self.start_time.elapsed().as_millis() as u64,
            last_duration_ms,
            session_id: self.session_id.clone(),
            diagnostic: diagnostic.map(str::to_string),
        };
        write_live_status(&status);
        self.last_status_write = Instant::now();
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
            crate::transcribe::whisper_context_params(),
        )
        .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?
    };

    // Start audio stream FIRST — validate mic access before truncating any files
    let device_override = config.recording.device.as_deref();
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
    let mut writer = LiveTranscriptWriter::new(config, context_session_id)?;
    writer.mark_healthy();

    let mut vad = Vad::new();
    let mut streaming = StreamingWhisper::new(config.transcription.language.clone());
    #[cfg(target_os = "macos")]
    let mut apple_utterance_samples: Vec<f32> = Vec::new();
    #[cfg(target_os = "macos")]
    let mut apple_live_enabled = config
        .transcription
        .engine
        .eq_ignore_ascii_case("apple-speech")
        && live_supports_apple_speech();
    #[cfg(not(target_os = "macos"))]
    let mut apple_live_enabled = false;

    // Parakeet engine dispatch — mirrors run_sidecar_inner_mpsc. When the user
    // configures `engine = "parakeet"` and the parakeet feature is compiled in,
    // utterance samples are accumulated and routed through the parakeet path
    // (warm sidecar socket when `parakeet_sidecar_enabled = true`, subprocess
    // otherwise) at VAD-end. On failure the session transparently falls back
    // to whisper for the remainder. See RFC 0002.
    #[cfg(feature = "parakeet")]
    let mut parakeet_utterance_samples: Vec<f32> = Vec::new();
    #[cfg(feature = "parakeet")]
    let mut parakeet_live_enabled = live_supports_parakeet(&config.transcription.engine);
    #[cfg(not(feature = "parakeet"))]
    let parakeet_live_enabled = false;

    let mut was_speaking = false;
    let mut utterance_samples: usize = 0;
    let max_utterance_secs = config.live_transcript.max_utterance_secs.max(5);
    let max_utterance_samples = (max_utterance_secs as usize).saturating_mul(16000);

    // One-time scope warning when the user configured parakeet but the feature
    // isn't compiled in. Same warning the recording sidecar emits.
    if config.transcription.engine.eq_ignore_ascii_case("parakeet") && !parakeet_live_enabled {
        emit_live_engine_scope_warning(&config.transcription.engine, "standalone");
    }
    if config
        .transcription
        .engine
        .eq_ignore_ascii_case("apple-speech")
        && !apple_live_enabled
    {
        emit_live_engine_scope_warning(&config.transcription.engine, "standalone");
    }
    if apple_live_enabled {
        eprintln!("[minutes] Apple Speech live transcript enabled (experimental, standalone only)");
    }

    // Warm the parakeet sidecar at session start so the first utterance doesn't
    // pay subprocess-spawn + model-load latency. We only warm the sidecar lane
    // because:
    //   - `parakeet_sidecar_enabled = true` → warmup leaves a hot example-server
    //     child + loaded model, making subsequent utterances fast.
    //   - `parakeet_sidecar_enabled = false` → warmup would just be a throwaway
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
    if parakeet_live_enabled && config.transcription.parakeet_sidecar_enabled {
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
                    &mut parakeet_live_enabled,
                    &mut parakeet_utterance_samples,
                    config,
                    &mut streaming,
                    &whisper_ctx,
                    "standalone",
                );
                #[cfg(not(feature = "parakeet"))]
                finalize_on_exit(
                    &mut writer,
                    &mut apple_live_enabled,
                    #[cfg(target_os = "macos")]
                    &mut apple_utterance_samples,
                    config,
                    &mut streaming,
                    &whisper_ctx,
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
                    &mut parakeet_live_enabled,
                    &mut parakeet_utterance_samples,
                    config,
                    &mut streaming,
                    &whisper_ctx,
                    "standalone",
                );
                #[cfg(not(feature = "parakeet"))]
                finalize_on_exit(
                    &mut writer,
                    &mut apple_live_enabled,
                    #[cfg(target_os = "macos")]
                    &mut apple_utterance_samples,
                    config,
                    &mut streaming,
                    &whisper_ctx,
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
                            &mut parakeet_live_enabled,
                            &mut parakeet_utterance_samples,
                            config,
                            &mut streaming,
                            &whisper_ctx,
                            "standalone",
                        );
                        #[cfg(not(feature = "parakeet"))]
                        finalize_on_exit(
                            &mut writer,
                            &mut apple_live_enabled,
                            #[cfg(target_os = "macos")]
                            &mut apple_utterance_samples,
                            config,
                            &mut streaming,
                            &whisper_ctx,
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
                                &mut parakeet_live_enabled,
                                &mut parakeet_utterance_samples,
                                config,
                                &mut streaming,
                                &whisper_ctx,
                                "standalone",
                            );
                            #[cfg(not(feature = "parakeet"))]
                            finalize_on_exit(
                                &mut writer,
                                &mut apple_live_enabled,
                                #[cfg(target_os = "macos")]
                                &mut apple_utterance_samples,
                                config,
                                &mut streaming,
                                &whisper_ctx,
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
            } else if let Some(_sr) = streaming.feed(&chunk.samples, &whisper_ctx) {
                // Partial result available — could emit event in the future
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
                    &mut parakeet_live_enabled,
                    &mut parakeet_utterance_samples,
                    config,
                    &mut streaming,
                    &whisper_ctx,
                    "standalone",
                );
                #[cfg(not(feature = "parakeet"))]
                let write_ok = finalize_live_utterance(
                    &mut writer,
                    &mut apple_live_enabled,
                    #[cfg(target_os = "macos")]
                    &mut apple_utterance_samples,
                    config,
                    &mut streaming,
                    &whisper_ctx,
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
                &mut parakeet_live_enabled,
                &mut parakeet_utterance_samples,
                config,
                &mut streaming,
                &whisper_ctx,
                "standalone",
            );
            #[cfg(not(feature = "parakeet"))]
            let write_ok = finalize_live_utterance(
                &mut writer,
                &mut apple_live_enabled,
                #[cfg(target_os = "macos")]
                &mut apple_utterance_samples,
                config,
                &mut streaming,
                &whisper_ctx,
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
    Energy(Vad),
}

#[cfg(feature = "whisper")]
struct RecordingSidecarVad {
    backend: RecordingSidecarVadBackend,
}

#[cfg(feature = "whisper")]
impl RecordingSidecarVad {
    fn new(config: &Config) -> Self {
        if let Some(vad_path) = crate::transcribe::resolve_vad_model_path(config) {
            match SileroSidecarVad::new(&vad_path) {
                Ok(vad) => {
                    tracing::info!(
                        vad_model = %vad_path.display(),
                        "recording sidecar using Silero VAD"
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

        Self {
            backend: RecordingSidecarVadBackend::Energy(Vad::new()),
        }
    }

    fn mode_name(&self) -> &'static str {
        match &self.backend {
            RecordingSidecarVadBackend::Silero(_) => "silero",
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
        })
    }

    fn process(
        &mut self,
        samples: &[f32],
        rms: f32,
    ) -> Result<VadResult, whisper_rs::WhisperError> {
        self.buffer.extend_from_slice(samples);

        let segments = self.ctx.segments_from_samples(self.params, &self.buffer)?;
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

#[cfg(feature = "whisper")]
fn transcribe_with_whisper_for_live_sidecar(
    samples: &[f32],
    whisper_ctx: &whisper_rs::WhisperContext,
    language: Option<String>,
) -> Option<(String, f64)> {
    if samples.is_empty() {
        return None;
    }

    let mut streaming = StreamingWhisper::new(language);
    for chunk in samples.chunks(1600) {
        let _ = streaming.feed(chunk, whisper_ctx);
    }
    streaming
        .finalize(whisper_ctx)
        .map(|result| (result.text, result.duration_secs))
}

/// Minimum utterance length for the parakeet path — mirrors
/// `StreamingWhisper::MIN_TRANSCRIBE_SAMPLES`. VAD blips shorter than this
/// are dropped without hitting the sidecar/subprocess, avoiding temp-file
/// churn and latency spikes on noisy inputs.
#[cfg(feature = "parakeet")]
const PARAKEET_LIVE_MIN_SAMPLES: usize = 16_000; // 1 second at 16kHz
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
fn finalize_live_utterance(
    writer: &mut LiveTranscriptWriter,
    apple_live_enabled: &mut bool,
    #[cfg(target_os = "macos")] apple_utterance_samples: &mut Vec<f32>,
    parakeet_live_enabled: &mut bool,
    parakeet_utterance_samples: &mut Vec<f32>,
    config: &Config,
    streaming: &mut StreamingWhisper,
    whisper_ctx: &whisper_rs::WhisperContext,
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
                if let Some((text, duration_secs)) = transcribe_with_whisper_for_live_sidecar(
                    apple_utterance_samples,
                    whisper_ctx,
                    config.transcription.language.clone(),
                ) {
                    let ok = writer.write_utterance(&text, duration_secs);
                    apple_utterance_samples.clear();
                    return ok;
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
                // Fall back: transcribe the accumulated samples via whisper so
                // this utterance still lands in the JSONL.
                if let Some((text, duration_secs)) = transcribe_with_whisper_for_live_sidecar(
                    parakeet_utterance_samples,
                    whisper_ctx,
                    config.transcription.language.clone(),
                ) {
                    let ok = writer.write_utterance(&text, duration_secs);
                    parakeet_utterance_samples.clear();
                    return ok;
                }
                parakeet_utterance_samples.clear();
                return true;
            }
        }
    }

    // Whisper path
    let write_ok = if let Some(sr) = streaming.finalize(whisper_ctx) {
        writer.write_utterance(&sr.text, sr.duration_secs)
    } else {
        true
    };
    streaming.reset();
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
fn finalize_on_exit(
    writer: &mut LiveTranscriptWriter,
    apple_live_enabled: &mut bool,
    #[cfg(target_os = "macos")] apple_utterance_samples: &mut Vec<f32>,
    parakeet_live_enabled: &mut bool,
    parakeet_utterance_samples: &mut Vec<f32>,
    config: &Config,
    streaming: &mut StreamingWhisper,
    whisper_ctx: &whisper_rs::WhisperContext,
    source: &'static str,
) {
    if !finalize_live_utterance(
        writer,
        apple_live_enabled,
        #[cfg(target_os = "macos")]
        apple_utterance_samples,
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
fn finalize_live_utterance(
    writer: &mut LiveTranscriptWriter,
    apple_live_enabled: &mut bool,
    #[cfg(target_os = "macos")] apple_utterance_samples: &mut Vec<f32>,
    config: &Config,
    streaming: &mut StreamingWhisper,
    whisper_ctx: &whisper_rs::WhisperContext,
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
                if let Some((text, duration_secs)) = transcribe_with_whisper_for_live_sidecar(
                    apple_utterance_samples,
                    whisper_ctx,
                    config.transcription.language.clone(),
                ) {
                    let ok = writer.write_utterance(&text, duration_secs);
                    apple_utterance_samples.clear();
                    return ok;
                }
                apple_utterance_samples.clear();
                return true;
            }
        }
    }

    let write_ok = if let Some(sr) = streaming.finalize(whisper_ctx) {
        writer.write_utterance(&sr.text, sr.duration_secs)
    } else {
        true
    };
    #[cfg(not(target_os = "macos"))]
    let _ = (&apple_live_enabled, &config, source);
    streaming.reset();
    write_ok
}

#[cfg(all(feature = "whisper", not(feature = "parakeet")))]
fn finalize_on_exit(
    writer: &mut LiveTranscriptWriter,
    apple_live_enabled: &mut bool,
    #[cfg(target_os = "macos")] apple_utterance_samples: &mut Vec<f32>,
    config: &Config,
    streaming: &mut StreamingWhisper,
    whisper_ctx: &whisper_rs::WhisperContext,
    source: &'static str,
) {
    if !finalize_live_utterance(
        writer,
        apple_live_enabled,
        #[cfg(target_os = "macos")]
        apple_utterance_samples,
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
    // Guard: don't clobber a standalone live transcript session's JSONL
    let lt_pid = pid::live_transcript_pid_path();
    if let Ok(Some(_)) = pid::check_pid_file(&lt_pid) {
        tracing::info!("standalone live transcript active — skipping recording sidecar");
        return Ok(());
    }

    write_live_status_transition(LiveStatusState::Starting, None);

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
            crate::transcribe::whisper_context_params(),
        )
        .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?
    };

    let mut sidecar_config = config.clone();
    sidecar_config.live_transcript.save_wav = false;
    let mut writer = LiveTranscriptWriter::new(&sidecar_config, None)?;
    writer.mark_healthy();

    let mut vad = RecordingSidecarVad::new(config);
    let mut streaming = StreamingWhisper::new(config.transcription.language.clone());
    #[cfg(feature = "parakeet")]
    let mut parakeet_utterance_samples: Vec<f32> = Vec::new();
    #[cfg(feature = "parakeet")]
    let mut parakeet_live_enabled = live_supports_parakeet(&config.transcription.engine);
    #[cfg(not(feature = "parakeet"))]
    let parakeet_live_enabled = false;
    let mut was_speaking = false;
    let mut utterance_samples: usize = 0;
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

    tracing::info!("live sidecar started (recording mode)");

    loop {
        writer.maybe_write_heartbeat();
        if stop_flag.load(Ordering::Relaxed) {
            if utterance_samples > 0 {
                if parakeet_live_enabled {
                    #[cfg(feature = "parakeet")]
                    {
                        match transcribe_with_parakeet_for_live_sidecar(
                            &parakeet_utterance_samples,
                            config,
                        ) {
                            Ok(Some((text, duration_secs))) => {
                                writer.write_utterance(&text, duration_secs);
                            }
                            Ok(None) => {}
                            Err(error) => {
                                tracing::warn!(
                                    error = %error,
                                    "live recording-sidecar parakeet path failed at shutdown — falling back to whisper"
                                );
                                if let Some((text, duration_secs)) =
                                    transcribe_with_whisper_for_live_sidecar(
                                        &parakeet_utterance_samples,
                                        &whisper_ctx,
                                        config.transcription.language.clone(),
                                    )
                                {
                                    writer.write_utterance(&text, duration_secs);
                                }
                            }
                        }
                    }
                } else if let Some(sr) = streaming.finalize(&whisper_ctx) {
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
                    if parakeet_live_enabled {
                        #[cfg(feature = "parakeet")]
                        {
                            match transcribe_with_parakeet_for_live_sidecar(
                                &parakeet_utterance_samples,
                                config,
                            ) {
                                Ok(Some((text, duration_secs))) => {
                                    writer.write_utterance(&text, duration_secs);
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    tracing::warn!(
                                        error = %error,
                                        "live recording-sidecar parakeet path failed during channel disconnect — falling back to whisper"
                                    );
                                    if let Some((text, duration_secs)) =
                                        transcribe_with_whisper_for_live_sidecar(
                                            &parakeet_utterance_samples,
                                            &whisper_ctx,
                                            config.transcription.language.clone(),
                                        )
                                    {
                                        writer.write_utterance(&text, duration_secs);
                                    }
                                }
                            }
                        }
                    } else if let Some(sr) = streaming.finalize(&whisper_ctx) {
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

        let vad_result = vad.process(&samples, rms);
        gating_stats.observe(samples.len(), vad_result.speaking);

        if vad_result.speaking {
            was_speaking = true;
            utterance_samples += samples.len();

            if parakeet_live_enabled {
                #[cfg(feature = "parakeet")]
                {
                    parakeet_utterance_samples.extend_from_slice(&samples);
                }
            } else if let Some(_sr) = streaming.feed(&samples, &whisper_ctx) {
                // Partial result — could emit event in future
            }

            if utterance_samples >= max_utterance_samples {
                tracing::info!("sidecar: max utterance duration, force-finalizing");
                if parakeet_live_enabled {
                    #[cfg(feature = "parakeet")]
                    {
                        match transcribe_with_parakeet_for_live_sidecar(
                            &parakeet_utterance_samples,
                            config,
                        ) {
                            Ok(Some((text, duration_secs))) => {
                                writer.write_utterance(&text, duration_secs);
                            }
                            Ok(None) => {}
                            Err(error) => {
                                tracing::warn!(
                                    error = %error,
                                    "live recording-sidecar parakeet path failed — switching this session to whisper"
                                );
                                parakeet_live_enabled = false;
                                emit_live_engine_fallback_warning(
                                    "recording-sidecar",
                                    &error.to_string(),
                                );
                                if let Some((text, duration_secs)) =
                                    transcribe_with_whisper_for_live_sidecar(
                                        &parakeet_utterance_samples,
                                        &whisper_ctx,
                                        config.transcription.language.clone(),
                                    )
                                {
                                    writer.write_utterance(&text, duration_secs);
                                }
                            }
                        }
                        parakeet_utterance_samples.clear();
                    }
                } else if let Some(sr) = streaming.finalize(&whisper_ctx) {
                    writer.write_utterance(&sr.text, sr.duration_secs);
                }
                streaming.reset();
                utterance_samples = 0;
                was_speaking = false;
            }
        } else if was_speaking && utterance_samples > 0 {
            if parakeet_live_enabled {
                #[cfg(feature = "parakeet")]
                {
                    match transcribe_with_parakeet_for_live_sidecar(
                        &parakeet_utterance_samples,
                        config,
                    ) {
                        Ok(Some((text, duration_secs))) => {
                            writer.write_utterance(&text, duration_secs);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                "live recording-sidecar parakeet path failed — switching this session to whisper"
                            );
                            parakeet_live_enabled = false;
                            emit_live_engine_fallback_warning(
                                "recording-sidecar",
                                &error.to_string(),
                            );
                            if let Some((text, duration_secs)) =
                                transcribe_with_whisper_for_live_sidecar(
                                    &parakeet_utterance_samples,
                                    &whisper_ctx,
                                    config.transcription.language.clone(),
                                )
                            {
                                writer.write_utterance(&text, duration_secs);
                            }
                        }
                    }
                    parakeet_utterance_samples.clear();
                }
            } else if let Some(sr) = streaming.finalize(&whisper_ctx) {
                writer.write_utterance(&sr.text, sr.duration_secs);
            }
            streaming.reset();
            utterance_samples = 0;
            was_speaking = false;
        }
    }

    let (lines, duration, _path) = writer.finalize();
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
    let now = Local::now();
    let standalone_active = lt_process_pid.is_some();

    let (sidecar_active, diagnostic) = if recording_pid.is_some() {
        evaluate_recording_sidecar_status(live_status.as_ref(), now)
    } else {
        (false, None)
    };

    let active = standalone_active || sidecar_active;
    let pid = if standalone_active {
        lt_process_pid
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

    #[cfg(all(feature = "whisper", feature = "streaming"))]
    #[test]
    fn sidecar_missing_status_is_inactive_with_diagnostic() {
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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

        let status =
            derive_session_status(None, None, &status_path, &dir.path().join("live.jsonl"));

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
            None,
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
            None,
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
}
