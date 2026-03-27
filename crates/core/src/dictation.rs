use crate::config::Config;
use crate::error::{DictationError, MinutesError, TranscribeError};
use crate::markdown::{ContentType, Frontmatter, OutputStatus};
use crate::pid;
use crate::streaming::AudioStream;
use crate::streaming_whisper::StreamingWhisper;
use crate::vad::Vad;
use chrono::Local;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ── Model preload cache ──────────────────────────────────────
//
// The whisper model takes 1-15s to load depending on size and system load.
// Preloading on app startup moves this cost to a background thread so the
// first dictation press goes straight to "Listening..." with zero delay.
//
// The cache holds one model at a time. If the user changes the dictation
// model in settings, the next preload_model() call replaces it.
// The model is taken out during a session and returned when done.

#[cfg(feature = "whisper")]
struct CachedModel {
    ctx: whisper_rs::WhisperContext,
    model_name: String,
}

#[cfg(feature = "whisper")]
static MODEL_CACHE: std::sync::LazyLock<std::sync::Mutex<Option<CachedModel>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

/// Preload the whisper model for dictation in the background.
/// Call this on app startup. Safe to call multiple times — skips if
/// the same model is already cached.
#[cfg(feature = "whisper")]
pub fn preload_model(config: &Config) -> Result<(), MinutesError> {
    let model_name = config.dictation.model.clone();

    // Check if already cached with the same model
    if let Ok(cache) = MODEL_CACHE.lock() {
        if let Some(ref cached) = *cache {
            if cached.model_name == model_name {
                tracing::info!(model = %model_name, "dictation model already preloaded");
                return Ok(());
            }
        }
    }

    let model_path = crate::transcribe::resolve_model_path_for_dictation(config)?;
    tracing::info!(model = %model_path.display(), "preloading whisper model for dictation");

    let ctx = whisper_rs::WhisperContext::new_with_params(
        model_path
            .to_str()
            .ok_or_else(|| TranscribeError::ModelLoadError("invalid path".into()))?,
        whisper_rs::WhisperContextParameters::default(),
    )
    .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?;

    if let Ok(mut cache) = MODEL_CACHE.lock() {
        *cache = Some(CachedModel {
            ctx,
            model_name: model_name.clone(),
        });
    }

    tracing::info!(model = %model_name, "dictation model preloaded successfully");
    Ok(())
}

/// Preload stub when whisper feature is disabled.
#[cfg(not(feature = "whisper"))]
pub fn preload_model(_config: &Config) -> Result<(), MinutesError> {
    Ok(())
}

/// Take the cached model out for use during a dictation session.
/// Returns None if no model is cached or the model name doesn't match.
#[cfg(feature = "whisper")]
fn take_cached_model(model_name: &str) -> Option<whisper_rs::WhisperContext> {
    let mut cache = MODEL_CACHE.lock().ok()?;
    let cached = cache.as_ref()?;
    if cached.model_name == model_name {
        cache.take().map(|c| c.ctx)
    } else {
        None
    }
}

/// Return a model to the cache after a dictation session.
#[cfg(feature = "whisper")]
fn return_model_to_cache(ctx: whisper_rs::WhisperContext, model_name: String) {
    if let Ok(mut cache) = MODEL_CACHE.lock() {
        *cache = Some(CachedModel { ctx, model_name });
    }
}

// ──────────────────────────────────────────────────────────────
// Dictation pipeline:
//
//   ┌─────────────┐
//   │ AudioStream  │──▶ 100ms chunks at 16kHz
//   └──────┬───────┘
//          │
//          ▼
//   ┌─────────────┐
//   │ VAD loop     │──▶ speaking? → accumulate Vec<f32>
//   │              │    silence?  → process_utterance()
//   │              │    yield?    → check recording.pid
//   └──────┬───────┘
//          │
//          ▼
//   ┌─────────────────────────────────┐
//   │ process_utterance()              │
//   │  ├─ batch whisper (preloaded)    │
//   │  ├─ write to destination         │
//   │  ├─ append daily note            │
//   │  ├─ save dictation file          │
//   │  └─ spawn async: LLM cleanup    │
//   └──────────────────────────────────┘
//
// State machine:
//   [Idle] ──start()──▶ [Listening] ──speech──▶ [Accumulating]
//     ▲                      │                       │
//     │                      │silence (no speech)     │silence
//     │                      │                       ▼
//     │                      │              [Processing]
//     │                      │                  │
//     │◀─────stop()/Esc──────┤◀─────────────────┘
//     │◀──recording.pid──────┘   (back to Listening)
// ──────────────────────────────────────────────────────────────

/// Result from processing a single dictation utterance.
#[derive(Debug, Clone)]
pub struct DictationResult {
    pub text: String,
    pub duration_secs: f64,
    pub destination: String,
    pub file_path: Option<PathBuf>,
}

/// Callback for dictation events (used by Tauri UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictationEvent {
    Listening,
    Accumulating,
    Processing,
    /// Partial transcription (streaming mode) — text updates progressively.
    PartialText(String),
    /// Silence countdown: total timeout ms, remaining ms.
    SilenceCountdown {
        total_ms: u64,
        remaining_ms: u64,
    },
    Success,
    Error,
    Cancelled,
    Yielded,
}

/// Run the dictation pipeline. Blocks until stopped or silence timeout.
///
/// `stop_flag`: set to true to stop the session (Esc key, Ctrl-C, MCP stop).
/// `on_event`: callback for UI state updates.
/// `on_result`: callback when an utterance is processed (text + metadata).
pub fn run<F, G>(
    stop_flag: Arc<AtomicBool>,
    config: &Config,
    mut on_event: F,
    mut on_result: G,
) -> Result<(), MinutesError>
where
    F: FnMut(DictationEvent),
    G: FnMut(DictationResult),
{
    // Check for conflicts: recording must not be active
    if let Ok(Some(_)) = pid::check_recording() {
        return Err(DictationError::RecordingActive.into());
    }

    // Check for conflicts: another dictation must not be active
    let dict_pid = pid::dictation_pid_path();
    if let Ok(Some(existing)) = pid::check_pid_file(&dict_pid) {
        return Err(DictationError::AlreadyActive(existing).into());
    }

    // Acquire dictation PID
    pid::create_pid_file(&dict_pid)?;

    // Ensure cleanup on all exit paths
    let result = run_inner(stop_flag, config, &mut on_event, &mut on_result);

    // Release PID
    pid::remove_pid_file(&dict_pid).ok();

    result
}

fn run_inner<F, G>(
    stop_flag: Arc<AtomicBool>,
    config: &Config,
    on_event: &mut F,
    on_result: &mut G,
) -> Result<(), MinutesError>
where
    F: FnMut(DictationEvent),
    G: FnMut(DictationResult),
{
    // Try to use preloaded model, fall back to loading on demand
    #[cfg(feature = "whisper")]
    let model_name = config.dictation.model.clone();
    #[cfg(feature = "whisper")]
    let whisper_ctx = if let Some(ctx) = take_cached_model(&model_name) {
        tracing::info!(model = %model_name, "using preloaded whisper model");
        ctx
    } else {
        let model_path = crate::transcribe::resolve_model_path_for_dictation(config)?;
        tracing::info!(model = %model_path.display(), "loading whisper model on demand");
        let ctx = whisper_rs::WhisperContext::new_with_params(
            model_path
                .to_str()
                .ok_or_else(|| TranscribeError::ModelLoadError("invalid path".into()))?,
            whisper_rs::WhisperContextParameters::default(),
        )
        .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?;
        tracing::info!("whisper model loaded for dictation session");
        ctx
    };

    #[cfg(not(feature = "whisper"))]
    return Err(
        TranscribeError::ModelLoadError("dictation requires the whisper feature".into()).into(),
    );

    // Start audio stream
    #[cfg(feature = "whisper")]
    {
        let stream = AudioStream::start()?;
        tracing::info!(device = %stream.device_name, "dictation audio stream started");

        let mut vad = Vad::new();
        let mut streaming = StreamingWhisper::new(
            config.transcription.language.clone(),
            config.transcription.initial_prompt.clone(),
        );
        let mut was_speaking = false;
        let mut has_spoken = false;
        let mut total_silence_ms: u64 = 0;
        let mut utterance_samples: usize = 0;
        let max_utterance_samples = config.dictation.max_utterance_secs as usize * 16000;

        on_event(DictationEvent::Listening);

        loop {
            // Check stop flag (Esc / Ctrl-C / MCP stop)
            if stop_flag.load(Ordering::Relaxed) {
                // Finalize any in-progress transcription before exiting
                if utterance_samples > 0 {
                    on_event(DictationEvent::Processing);
                    if let Some(sr) = streaming.finalize(&whisper_ctx) {
                        let result = finish_utterance(&sr.text, sr.duration_secs, config);
                        if let Some(result) = result {
                            on_event(DictationEvent::Success);
                            on_result(result);
                        }
                    }
                }
                on_event(DictationEvent::Cancelled);
                break;
            }

            // Check if recording started (yield to recording)
            if let Ok(Some(_)) = pid::check_recording() {
                tracing::info!("recording started — yielding dictation");
                if utterance_samples > 0 {
                    on_event(DictationEvent::Processing);
                    if let Some(sr) = streaming.finalize(&whisper_ctx) {
                        let result = finish_utterance(&sr.text, sr.duration_secs, config);
                        if let Some(result) = result {
                            on_event(DictationEvent::Success);
                            on_result(result);
                        }
                    }
                }
                on_event(DictationEvent::Yielded);
                break;
            }

            // Receive audio chunk (100ms timeout to allow stop checks)
            let chunk = match stream
                .receiver
                .recv_timeout(std::time::Duration::from_millis(100))
            {
                Ok(chunk) => chunk,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            };

            let vad_result = vad.process(chunk.rms);

            if vad_result.speaking {
                if !was_speaking {
                    on_event(DictationEvent::Accumulating);
                    total_silence_ms = 0;
                }
                was_speaking = true;
                has_spoken = true;
                utterance_samples += chunk.samples.len();

                // Feed to streaming whisper — may emit a partial result
                if let Some(sr) = streaming.feed(&chunk.samples, &whisper_ctx) {
                    on_event(DictationEvent::PartialText(sr.text));
                }

                // Force-finalize if max utterance reached
                if utterance_samples >= max_utterance_samples {
                    tracing::info!("max utterance duration reached, force-processing");
                    on_event(DictationEvent::Processing);
                    if let Some(sr) = streaming.finalize(&whisper_ctx) {
                        let result = finish_utterance(&sr.text, sr.duration_secs, config);
                        if let Some(result) = result {
                            on_event(DictationEvent::Success);
                            on_result(result);
                        }
                    }
                    streaming.reset();
                    utterance_samples = 0;
                    was_speaking = false;
                    on_event(DictationEvent::Listening);
                }
            } else {
                // Silence
                if was_speaking && utterance_samples > 0 {
                    // Speech just ended — finalize the streaming transcription
                    on_event(DictationEvent::Processing);
                    if let Some(sr) = streaming.finalize(&whisper_ctx) {
                        let result = finish_utterance(&sr.text, sr.duration_secs, config);
                        if let Some(result) = result {
                            on_event(DictationEvent::Success);
                            on_result(result);
                        }
                    }
                    streaming.reset();
                    utterance_samples = 0;
                    was_speaking = false;
                    total_silence_ms = 0;
                    on_event(DictationEvent::Listening);
                }

                total_silence_ms += 100;
                if has_spoken
                    && !was_speaking
                    && total_silence_ms < config.dictation.silence_timeout_ms
                {
                    let remaining = config.dictation.silence_timeout_ms - total_silence_ms;
                    on_event(DictationEvent::SilenceCountdown {
                        total_ms: config.dictation.silence_timeout_ms,
                        remaining_ms: remaining,
                    });
                }
                if has_spoken
                    && !was_speaking
                    && total_silence_ms >= config.dictation.silence_timeout_ms
                {
                    tracing::info!(
                        silence_ms = total_silence_ms,
                        "silence timeout — ending dictation"
                    );
                    break;
                }
            }
        }

        // Return model to cache for next session
        return_model_to_cache(whisper_ctx, model_name);

        Ok(())
    }
}

/// Finish a transcribed utterance: write to clipboard, file, daily note.
/// Called after StreamingWhisper produces a final result.
fn finish_utterance(text: &str, duration_secs: f64, config: &Config) -> Option<DictationResult> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }

    tracing::info!(
        words = text.split_whitespace().count(),
        duration = format!("{:.1}s", duration_secs),
        "dictation utterance finalized"
    );

    // Write to clipboard
    let destination = config.dictation.destination.as_str();
    if destination == "clipboard" || destination.is_empty() {
        if let Err(e) = write_to_clipboard(&text) {
            tracing::error!("clipboard write failed: {}", e);
        }
    }

    // Write dictation file
    let file_path = if destination != "daily_note" {
        write_dictation_file(&text, duration_secs, config)
    } else {
        None
    };

    // Append to daily note
    if config.dictation.daily_note_log {
        append_dictation_to_daily_note(&text, config);
    }

    Some(DictationResult {
        text,
        duration_secs,
        destination: destination.to_string(),
        file_path,
    })
}

/// Legacy batch process: transcribe → output. Kept for fallback/testing.
#[cfg(feature = "whisper")]
#[allow(dead_code)]
fn process_utterance(
    samples: &[f32],
    ctx: &whisper_rs::WhisperContext,
    config: &Config,
    duration_secs: f64,
) -> Option<DictationResult> {
    let mut state = ctx.create_state().ok()?;

    let mut params = crate::transcribe::default_whisper_params(None);
    params.set_n_threads(num_cpus());
    params.set_language(config.transcription.language.as_deref());

    if let Err(e) = state.full(params, samples) {
        tracing::error!("whisper transcription failed: {}", e);
        save_failed_audio(samples);
        return None;
    }

    let num_segments = state.full_n_segments();
    let mut text = String::new();
    for i in 0..num_segments {
        if let Some(seg) = state.get_segment(i) {
            if let Ok(t) = seg.to_str_lossy() {
                let t = t.trim();
                if !t.is_empty() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(t);
                }
            }
        }
    }

    let text = text.trim().to_string();
    if text.is_empty() {
        tracing::debug!("whisper returned empty text — discarding");
        return None;
    }

    // Delegate to finish_utterance for output
    finish_utterance(&text, duration_secs, config)
}

/// Write text to the system clipboard.
#[cfg(target_os = "macos")]
fn write_to_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn pbcopy: {}", e))?;

    let write_result = if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())
    } else {
        Ok(())
    };

    // Always wait for the child to prevent zombies
    let _ = child.wait();

    write_result.map_err(|e| format!("failed to write to pbcopy: {}", e))?;
    tracing::debug!(len = text.len(), "text written to clipboard");
    Ok(())
}

#[cfg(target_os = "windows")]
fn write_to_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("clip")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn clip.exe: {}", e))?;

    let write_result = if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())
    } else {
        Ok(())
    };

    let _ = child.wait();

    write_result.map_err(|e| format!("failed to write to clip.exe: {}", e))?;
    tracing::debug!(len = text.len(), "text written to clipboard");
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn write_to_clipboard(_text: &str) -> Result<(), String> {
    Err("clipboard write not implemented on this platform".into())
}

/// Write a dictation file to ~/meetings/dictations/.
fn write_dictation_file(text: &str, duration_secs: f64, config: &Config) -> Option<PathBuf> {
    let now = Local::now();
    let duration_str = if duration_secs < 60.0 {
        format!("{}s", duration_secs as u32)
    } else {
        format!(
            "{}m {}s",
            (duration_secs / 60.0) as u32,
            (duration_secs % 60.0) as u32
        )
    };

    let frontmatter = Frontmatter {
        title: first_words(text, 8),
        r#type: ContentType::Dictation,
        date: now,
        duration: duration_str,
        source: Some("dictation".into()),
        status: Some(OutputStatus::Complete),
        tags: vec![],
        attendees: vec![],
        calendar_event: None,
        people: vec![],
        entities: crate::markdown::EntityLinks::default(),
        device: None,
        captured_at: None,
        context: None,
        action_items: vec![],
        decisions: vec![],
        intents: vec![],
        recorded_by: config.identity.name.clone(),
        visibility: None,
        speaker_map: vec![],
    };

    match crate::markdown::write(&frontmatter, text, None, None, config) {
        Ok(result) => {
            tracing::info!(path = %result.path.display(), "dictation file written");
            Some(result.path)
        }
        Err(e) => {
            tracing::error!("failed to write dictation file: {}", e);
            None
        }
    }
}

/// Append a dictation entry to the daily note.
fn append_dictation_to_daily_note(text: &str, config: &Config) {
    use std::io::Write;

    if !config.daily_notes.enabled {
        return;
    }

    let note_dir = &config.daily_notes.path;
    if std::fs::create_dir_all(note_dir).is_err() {
        return;
    }

    let now = Local::now();
    let note_path = note_dir.join(format!("{}.md", now.format("%Y-%m-%d")));

    // Create file with header if it doesn't exist
    if !note_path.exists() {
        if let Err(e) = std::fs::write(&note_path, format!("# {}\n", now.format("%Y-%m-%d"))) {
            tracing::error!("failed to create daily note: {}", e);
            return;
        }
    }

    // Append-only open to avoid read-modify-write race
    let entry = format!("\n### ~{} - Dictation\n- {}\n", now.format("%H:%M"), text);
    match std::fs::OpenOptions::new().append(true).open(&note_path) {
        Ok(mut f) => {
            if let Err(e) = f.write_all(entry.as_bytes()) {
                tracing::error!("failed to append to daily note: {}", e);
            }
        }
        Err(e) => tracing::error!("failed to open daily note for append: {}", e),
    }
}

/// Save failed audio to disk for recovery.
fn save_failed_audio(samples: &[f32]) {
    let failed_dir = crate::config::Config::minutes_dir().join("dictation-failed");
    if std::fs::create_dir_all(&failed_dir).is_err() {
        return;
    }
    let path = failed_dir.join(format!("{}.wav", Local::now().format("%Y%m%d-%H%M%S")));
    if let Ok(mut writer) = hound::WavWriter::create(
        &path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    ) {
        for &s in samples {
            let _ = writer.write_sample((s * 32767.0) as i16);
        }
        let _ = writer.finalize();
        tracing::warn!(path = %path.display(), "failed audio saved for recovery");
    }
}

/// Extract first N words for title.
fn first_words(text: &str, n: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().take(n).collect();
    let title = words.join(" ");
    if text.split_whitespace().count() > n {
        format!("{}...", title)
    } else {
        title
    }
}

fn num_cpus() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4)
        .min(8) // Cap at 8 — diminishing returns beyond that for whisper
}
