use crate::config::Config;
use crate::dictation_cleanup::{clean_dictation_text, CleanupEngine, CleanupOptions};
use crate::error::{DictationError, MinutesError, TranscribeError};
use crate::markdown::{ContentType, Frontmatter, OutputStatus};
use crate::pid;
use crate::streaming::AudioStream;
use crate::streaming_whisper::{StreamingResult, StreamingWhisper};
use crate::vad::Vad;
use chrono::Local;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

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

fn startup_debug(event: &str, model: Option<&str>, elapsed_ms: Option<u128>, note: Option<&str>) {
    let payload = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "event": event,
        "model": model,
        "elapsedMs": elapsed_ms,
        "note": note,
    });
    let path = Config::minutes_dir().join("dictation-startup-debug.jsonl");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{payload}");
    }
}

/// Preload the whisper model for dictation in the background.
/// Call this on app startup. Safe to call multiple times — skips if
/// the same model is already cached.
#[cfg(feature = "whisper")]
pub fn preload_model(config: &Config) -> Result<(), MinutesError> {
    let model_name = config.dictation.model.clone();
    let preload_start = Instant::now();
    startup_debug("preload_start", Some(&model_name), None, None);

    // Check if already cached with the same model
    if let Ok(cache) = MODEL_CACHE.lock() {
        if let Some(ref cached) = *cache {
            if cached.model_name == model_name {
                tracing::info!(model = %model_name, "dictation model already preloaded");
                startup_debug(
                    "preload_cache_hit",
                    Some(&model_name),
                    Some(preload_start.elapsed().as_millis()),
                    None,
                );
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
        crate::transcribe::whisper_context_params(),
    )
    .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?;

    if let Ok(mut cache) = MODEL_CACHE.lock() {
        *cache = Some(CachedModel {
            ctx,
            model_name: model_name.clone(),
        });
    }

    tracing::info!(model = %model_name, "dictation model preloaded successfully");
    startup_debug(
        "preload_done",
        Some(&model_name),
        Some(preload_start.elapsed().as_millis()),
        None,
    );
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
    pub raw_text: String,
    pub text: String,
    pub duration_secs: f64,
    pub destination: String,
    pub file_path: Option<PathBuf>,
    pub daily_note_appended: bool,
}

/// Callback for dictation events (used by Tauri UI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictationEvent {
    Listening,
    Accumulating,
    Processing,
    /// Partial transcription (streaming mode) — text updates progressively.
    PartialText(String),
    /// Input level 0-100 for lightweight UI feedback.
    AudioLevel(u32),
    /// Silence countdown: total timeout ms, remaining ms.
    SilenceCountdown {
        total_ms: u64,
        remaining_ms: u64,
    },
    Success,
    Error,
    /// The configured dictation model is missing or incomplete.
    ModelMissing {
        /// Model identifier accepted by the corresponding setup command.
        model: String,
        /// Path where Minutes expected to find the model.
        expected_path: String,
        /// Exact command that installs or repairs the model.
        setup_command: String,
    },
    Cancelled,
    Yielded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DictationFinalBackend {
    Whisper,
    AppleSpeech,
    Parakeet,
}

impl DictationFinalBackend {
    fn needs_utterance_samples(self) -> bool {
        matches!(self, Self::AppleSpeech | Self::Parakeet)
    }
}

/// Check that the configured dictation model is installed and usable.
///
/// This preflight performs filesystem-only checks and never opens an audio
/// device. A missing or truncated model is returned as the same event used by
/// interactive dictation surfaces.
pub fn preflight_model(config: &Config) -> Result<(), DictationEvent> {
    if config.dictation.backend == "parakeet" {
        let model = config.transcription.parakeet_model.clone();
        if crate::parakeet::resolve_model_file(config, &model).is_some() {
            return Ok(());
        }
        let expected_path = crate::parakeet::install_dir(config, &model)
            .join(crate::parakeet::default_model_filename(&model));
        return Err(DictationEvent::ModelMissing {
            setup_command: crate::transcription_coordinator::parakeet_setup_command(&model),
            model,
            expected_path: expected_path.display().to_string(),
        });
    }

    #[cfg(feature = "whisper")]
    {
        match crate::transcribe::resolve_model_path_for_dictation(config) {
            Ok(_) => Ok(()),
            Err(TranscribeError::ModelTruncated {
                path, model_name, ..
            }) => Err(DictationEvent::ModelMissing {
                setup_command: format!("rm \"{}\" && minutes setup --model {}", path, model_name),
                model: model_name,
                expected_path: path,
            }),
            Err(TranscribeError::ModelNotFound(_)) => {
                let model = config.dictation.model.clone();
                let expected_path = config
                    .transcription
                    .model_path
                    .join(format!("ggml-{model}.bin"));
                Err(DictationEvent::ModelMissing {
                    setup_command: format!("minutes setup --model {model}"),
                    model,
                    expected_path: expected_path.display().to_string(),
                })
            }
            Err(_) => Ok(()),
        }
    }

    #[cfg(not(feature = "whisper"))]
    Ok(())
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
    if let Err(event) = preflight_model(config) {
        on_event(event);
        return Ok(());
    }

    // Check for conflicts: recording must not be active
    if let Ok(Some(_)) = pid::check_recording() {
        return Err(DictationError::RecordingActive.into());
    }

    // Check for conflicts: live transcript must not be active. `inspect_pid_file`
    // so a session holding the PID under a mandatory Windows lock is detected. #258.
    let lt_pid = pid::live_transcript_pid_path();
    if pid::inspect_pid_file(&lt_pid).is_active() {
        return Err(DictationError::LiveTranscriptActive.into());
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
    let run_start = Instant::now();
    startup_debug("run_inner_start", Some(&config.dictation.model), None, None);
    let final_backend = dictation_final_backend(config);

    // Try to use preloaded model, fall back to loading on demand
    #[cfg(feature = "whisper")]
    let model_name = config.dictation.model.clone();
    #[cfg(feature = "whisper")]
    let whisper_ctx = if let Some(ctx) = take_cached_model(&model_name) {
        tracing::info!(model = %model_name, "using preloaded whisper model");
        startup_debug(
            "model_cache_hit",
            Some(&model_name),
            Some(run_start.elapsed().as_millis()),
            None,
        );
        Some(ctx)
    } else {
        let load_start = Instant::now();
        startup_debug(
            "model_cache_miss_load_start",
            Some(&model_name),
            Some(run_start.elapsed().as_millis()),
            None,
        );
        match crate::transcribe::resolve_model_path_for_dictation(config) {
            Ok(model_path) => {
                tracing::info!(model = %model_path.display(), "loading whisper model on demand");
                let context = whisper_rs::WhisperContext::new_with_params(
                    model_path
                        .to_str()
                        .ok_or_else(|| TranscribeError::ModelLoadError("invalid path".into()))?,
                    crate::transcribe::whisper_context_params(),
                );
                match context {
                    Ok(context) => {
                        tracing::info!("whisper model loaded for dictation session");
                        startup_debug(
                            "model_load_done",
                            Some(&model_name),
                            Some(load_start.elapsed().as_millis()),
                            None,
                        );
                        Some(context)
                    }
                    Err(error) if final_backend == DictationFinalBackend::Parakeet => {
                        tracing::warn!(
                            error = %error,
                            "whisper partial model could not load; continuing with configured Parakeet dictation"
                        );
                        None
                    }
                    Err(error) => {
                        return Err(TranscribeError::ModelLoadError(error.to_string()).into())
                    }
                }
            }
            Err(error) if final_backend == DictationFinalBackend::Parakeet => {
                tracing::info!(
                    error = %error,
                    "whisper partials unavailable; continuing with configured Parakeet dictation"
                );
                None
            }
            Err(error) => return Err(error.into()),
        }
    };

    #[cfg(not(feature = "whisper"))]
    return Err(
        TranscribeError::ModelLoadError("dictation requires the whisper feature".into()).into(),
    );

    // Start audio stream
    #[cfg(feature = "whisper")]
    {
        let device_override = config.recording.device.as_deref();
        let permission_preflight = crate::capture::preflight_microphone_only();
        if let Some(reason) = permission_preflight.blocking_reason {
            return Err(DictationError::PermissionBlocked(reason).into());
        }
        for warning in permission_preflight.warnings {
            tracing::warn!(warning = %warning, "dictation microphone permission preflight warning");
        }

        let audio_start = Instant::now();
        startup_debug(
            "audio_stream_start",
            Some(&model_name),
            Some(run_start.elapsed().as_millis()),
            device_override,
        );
        let mut stream = AudioStream::start(device_override)?;
        startup_debug(
            "audio_stream_done",
            Some(&model_name),
            Some(audio_start.elapsed().as_millis()),
            Some(&stream.device_name),
        );
        tracing::info!(device = %stream.device_name, "dictation audio stream started");
        crate::events::append_event(crate::events::recording_started_event(
            None,
            "dictation",
            ["audio.capture", "dictation"],
        ));

        // Device change monitor for auto-reconnection. Pinned when the user
        // supplied an explicit device override.
        let mut device_monitor = if device_override.is_some() {
            crate::device_monitor::DeviceMonitor::pinned(&stream.device_name)
        } else {
            crate::device_monitor::DeviceMonitor::new(&stream.device_name)
        };

        let mut vad = Vad::new();
        let mut streaming = StreamingWhisper::with_partial_max_secs(
            config.transcription.language.clone(),
            config.transcription.partial_max_secs,
        );
        let mut final_utterance_samples: Vec<f32> = Vec::new();
        let mut accumulated_results: Vec<DictationResult> = Vec::new();
        let mut was_speaking = false;
        let mut has_spoken = false;
        let mut total_silence_ms: u64 = 0;
        let mut utterance_samples: usize = 0;
        let max_utterance_samples = config.dictation.max_utterance_secs as usize * 16000;

        on_event(DictationEvent::Listening);
        startup_debug(
            "listening_emitted",
            Some(&model_name),
            Some(run_start.elapsed().as_millis()),
            Some(&stream.device_name),
        );

        loop {
            // Check stop flag (Esc / Ctrl-C / MCP stop)
            if stop_flag.load(Ordering::Relaxed) {
                // Finalize any in-progress transcription before exiting
                if utterance_samples > 0 {
                    on_event(DictationEvent::Processing);
                    if let Some(sr) = finalize_dictation_transcription(
                        config,
                        final_backend,
                        &final_utterance_samples,
                        &mut streaming,
                        whisper_ctx.as_ref(),
                    ) {
                        handle_utterance(
                            &sr.text,
                            sr.duration_secs,
                            config,
                            &mut accumulated_results,
                            on_result,
                        );
                        on_event(DictationEvent::Success);
                    }
                    final_utterance_samples.clear();
                }
                flush_accumulated_results(config, &mut accumulated_results, on_event, on_result);
                on_event(DictationEvent::Cancelled);
                break;
            }

            // Check if recording started (yield to recording)
            if let Ok(Some(_)) = pid::check_recording() {
                tracing::info!("recording started — yielding dictation");
                if utterance_samples > 0 {
                    on_event(DictationEvent::Processing);
                    if let Some(sr) = finalize_dictation_transcription(
                        config,
                        final_backend,
                        &final_utterance_samples,
                        &mut streaming,
                        whisper_ctx.as_ref(),
                    ) {
                        handle_utterance(
                            &sr.text,
                            sr.duration_secs,
                            config,
                            &mut accumulated_results,
                            on_result,
                        );
                        on_event(DictationEvent::Success);
                    }
                    final_utterance_samples.clear();
                }
                flush_accumulated_results(config, &mut accumulated_results, on_event, on_result);
                on_event(DictationEvent::Yielded);
                break;
            }

            // Check for stream error or device change — attempt reconnection
            if stream.has_error() || device_monitor.has_device_changed() {
                let old_name = stream.device_name.clone();
                tracing::info!(device = %old_name, "dictation stream error or device change — reconnecting");
                drop(stream);
                match AudioStream::start(device_override) {
                    Ok(new_stream) => {
                        tracing::info!(
                            old = %old_name, new = %new_stream.device_name,
                            "dictation audio stream reconnected"
                        );
                        device_monitor.update_device(&new_stream.device_name);
                        stream = new_stream;
                        continue;
                    }
                    Err(e) => {
                        tracing::error!("dictation reconnect failed: {}", e);
                        break;
                    }
                }
            }

            // Receive audio chunk (100ms timeout to allow stop checks)
            let chunk = match stream
                .receiver
                .recv_timeout(std::time::Duration::from_millis(100))
            {
                Ok(chunk) => chunk,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    // Stream died — try to reconnect
                    let old_name = stream.device_name.clone();
                    tracing::warn!("dictation audio stream disconnected — attempting reconnect");
                    match AudioStream::start(device_override) {
                        Ok(new_stream) => {
                            tracing::info!(
                                old = %old_name, new = %new_stream.device_name,
                                "dictation audio stream reconnected after disconnect"
                            );
                            device_monitor.update_device(&new_stream.device_name);
                            stream = new_stream;
                            continue;
                        }
                        Err(_) => break,
                    }
                }
            };

            let vad_result = vad.process(chunk.rms);
            on_event(DictationEvent::AudioLevel(
                crate::streaming::stream_audio_level(),
            ));

            if vad_result.speaking {
                if !was_speaking {
                    on_event(DictationEvent::Accumulating);
                    total_silence_ms = 0;
                }
                was_speaking = true;
                has_spoken = true;
                utterance_samples += chunk.samples.len();
                if final_backend.needs_utterance_samples() {
                    final_utterance_samples.extend_from_slice(&chunk.samples);
                }

                // Feed to streaming whisper — may emit a partial result
                if let Some(context) = whisper_ctx.as_ref() {
                    if let Some(sr) = streaming.feed(&chunk.samples, context) {
                        on_event(DictationEvent::PartialText(sr.text));
                    }
                }

                // Force-finalize if max utterance reached
                if utterance_samples >= max_utterance_samples {
                    tracing::info!("max utterance duration reached, force-processing");
                    on_event(DictationEvent::Processing);
                    if let Some(sr) = finalize_dictation_transcription(
                        config,
                        final_backend,
                        &final_utterance_samples,
                        &mut streaming,
                        whisper_ctx.as_ref(),
                    ) {
                        handle_utterance(
                            &sr.text,
                            sr.duration_secs,
                            config,
                            &mut accumulated_results,
                            on_result,
                        );
                        on_event(DictationEvent::Success);
                    }
                    final_utterance_samples.clear();
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
                    if let Some(sr) = finalize_dictation_transcription(
                        config,
                        final_backend,
                        &final_utterance_samples,
                        &mut streaming,
                        whisper_ctx.as_ref(),
                    ) {
                        handle_utterance(
                            &sr.text,
                            sr.duration_secs,
                            config,
                            &mut accumulated_results,
                            on_result,
                        );
                        on_event(DictationEvent::Success);
                    }
                    final_utterance_samples.clear();
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
                    flush_accumulated_results(
                        config,
                        &mut accumulated_results,
                        on_event,
                        on_result,
                    );
                    break;
                }
            }
        }

        // Return model to cache for next session
        if let Some(context) = whisper_ctx {
            return_model_to_cache(context, model_name);
        }

        Ok(())
    }
}

fn dictation_final_backend(config: &Config) -> DictationFinalBackend {
    match config.dictation.backend.as_str() {
        "apple-speech" => {
            if apple_speech_dictation_ready(config) {
                DictationFinalBackend::AppleSpeech
            } else {
                DictationFinalBackend::Whisper
            }
        }
        "parakeet" => {
            if parakeet_dictation_ready(config) {
                DictationFinalBackend::Parakeet
            } else {
                DictationFinalBackend::Whisper
            }
        }
        "whisper" => DictationFinalBackend::Whisper,
        other => {
            tracing::warn!(
                backend = other,
                "unknown dictation backend; using whisper fallback"
            );
            DictationFinalBackend::Whisper
        }
    }
}

fn apple_speech_dictation_ready(_config: &Config) -> bool {
    #[cfg(target_os = "macos")]
    {
        match crate::apple_speech::probe_capabilities() {
            Ok(report) => {
                let ready = report.runtime_supported
                    && report.dictation_transcriber.asset_status != "unsupported";
                if !ready {
                    tracing::warn!(
                        asset_status = %report.dictation_transcriber.asset_status,
                        "apple-speech dictation requested but DictationTranscriber is not ready; using whisper fallback"
                    );
                }
                ready
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "apple-speech dictation capability probe failed; using whisper fallback"
                );
                false
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        tracing::warn!(
            "apple-speech dictation requested on a non-macOS build; using whisper fallback"
        );
        false
    }
}

fn parakeet_dictation_ready(config: &Config) -> bool {
    let status = crate::transcription_coordinator::parakeet_backend_status(config);
    if !status.ready {
        tracing::warn!(
            issues = %status.issues.join(", "),
            "parakeet dictation requested but Parakeet is not ready; using whisper fallback"
        );
    }
    status.ready
}

#[cfg(feature = "whisper")]
fn finalize_dictation_transcription(
    _config: &Config,
    _final_backend: DictationFinalBackend,
    _final_utterance_samples: &[f32],
    streaming: &mut StreamingWhisper,
    whisper_ctx: Option<&whisper_rs::WhisperContext>,
) -> Option<StreamingResult> {
    #[cfg(target_os = "macos")]
    if _final_backend == DictationFinalBackend::AppleSpeech {
        match transcribe_utterance_with_apple_speech(_final_utterance_samples, _config) {
            Ok(Some(result)) => return Some(result),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "apple-speech dictation failed; using whisper fallback for this utterance"
                );
            }
        }
    }

    #[cfg(feature = "parakeet")]
    if _final_backend == DictationFinalBackend::Parakeet {
        match transcribe_utterance_with_parakeet(_final_utterance_samples, _config) {
            Ok(Some(result)) => return Some(result),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "parakeet dictation failed; using whisper fallback for this utterance"
                );
            }
        }
    }

    whisper_ctx.and_then(|context| streaming.finalize(context))
}

#[cfg(target_os = "macos")]
fn transcribe_utterance_with_apple_speech(
    samples: &[f32],
    config: &Config,
) -> Result<Option<StreamingResult>, MinutesError> {
    if samples.len() < 16000 {
        return Ok(None);
    }

    let tmp_wav = tempfile::Builder::new()
        .prefix("minutes-dictation-apple-speech-")
        .suffix(".wav")
        .tempfile()
        .map_err(TranscribeError::Io)?;
    crate::transcribe::write_wav_16k_mono(tmp_wav.path(), samples)?;

    let locale = crate::apple_speech::live_locale_hint(config.transcription.language.as_deref());
    let result = crate::apple_speech::transcribe_with_apple_speech(
        tmp_wav.path(),
        locale.as_deref(),
        crate::apple_speech::AppleSpeechMode::Dictation,
        true,
    )?;

    if let Some(error) = result.error {
        return Err(MinutesError::Io(std::io::Error::other(error)));
    }

    let text = result.transcript.trim().to_string();
    if text.is_empty() {
        return Ok(None);
    }

    Ok(Some(StreamingResult {
        text,
        is_final: true,
        duration_secs: samples.len() as f64 / 16000.0,
    }))
}

#[cfg(feature = "parakeet")]
fn transcribe_utterance_with_parakeet(
    samples: &[f32],
    config: &Config,
) -> Result<Option<StreamingResult>, MinutesError> {
    if samples.len() < 16000 {
        return Ok(None);
    }

    let tmp_wav = tempfile::Builder::new()
        .prefix("minutes-dictation-parakeet-")
        .suffix(".wav")
        .tempfile()
        .map_err(TranscribeError::Io)?;
    crate::transcribe::write_wav_16k_mono(tmp_wav.path(), samples)?;

    let mut parakeet_config = config.clone();
    parakeet_config.transcription.engine = "parakeet".into();
    match crate::transcribe::transcribe(tmp_wav.path(), &parakeet_config) {
        Ok(result) => {
            let Some(text) = normalize_final_dictation_text(&result.text) else {
                return Ok(None);
            };
            Ok(Some(StreamingResult {
                text,
                is_final: true,
                duration_secs: samples.len() as f64 / 16000.0,
            }))
        }
        Err(TranscribeError::EmptyAudio) | Err(TranscribeError::EmptyTranscript(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(any(test, feature = "parakeet"))]
fn normalize_final_dictation_text(text: &str) -> Option<String> {
    let text = text
        .lines()
        .filter_map(|line| {
            let body = dictation_text_part(line).trim();
            (!body.is_empty()).then(|| body.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ");
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

#[cfg(any(test, feature = "parakeet"))]
fn dictation_text_part(line: &str) -> &str {
    line.find("] ")
        .map(|index| &line[index + 2..])
        .unwrap_or(line)
}

fn handle_utterance<G>(
    text: &str,
    duration_secs: f64,
    config: &Config,
    accumulated_results: &mut Vec<DictationResult>,
    on_result: &mut G,
) where
    G: FnMut(DictationResult),
{
    let Some(result) = prepare_result(text, duration_secs, config) else {
        return;
    };

    if config.dictation.accumulate {
        accumulated_results.push(result.clone());
        on_result(result);
        return;
    }

    if let Some(result) = write_result_outputs(result, config) {
        on_result(result);
    }
}

fn flush_accumulated_results<F, G>(
    config: &Config,
    accumulated_results: &mut Vec<DictationResult>,
    on_event: &mut F,
    on_result: &mut G,
) where
    F: FnMut(DictationEvent),
    G: FnMut(DictationResult),
{
    if !config.dictation.accumulate || accumulated_results.is_empty() {
        return;
    }

    if let Some(result) = finish_session(accumulated_results.as_slice(), config) {
        on_event(DictationEvent::Success);
        if config.dictation.destination != "stdout" {
            on_result(result);
        }
    }
    accumulated_results.clear();
}

/// Finish a transcribed utterance: write to clipboard, file, daily note.
/// Called after StreamingWhisper produces a final result.
fn finish_utterance(text: &str, duration_secs: f64, config: &Config) -> Option<DictationResult> {
    let result = prepare_result(text, duration_secs, config)?;
    write_result_outputs(result, config)
}

fn prepare_result(text: &str, duration_secs: f64, config: &Config) -> Option<DictationResult> {
    let raw_text = text.trim().to_string();
    if raw_text.is_empty() {
        return None;
    }

    // Polish the raw ASR output (capitalization, fillers, optional vocab/punctuation)
    // before it reaches the clipboard/file/daily-note. `raw_text` keeps the original.
    // If cleanup empties the text (e.g. an all-filler utterance), fall back to the raw
    // text rather than silently dropping content the user actually spoke.
    let cleaned = clean_dictation_text(&raw_text, &build_cleanup_options(config));
    let text = if cleaned.is_empty() {
        raw_text.clone()
    } else {
        cleaned
    };

    tracing::info!(
        words = text.split_whitespace().count(),
        duration = format!("{:.1}s", duration_secs),
        "dictation utterance finalized"
    );

    Some(DictationResult {
        raw_text,
        text,
        duration_secs,
        destination: config.dictation.destination.clone(),
        file_path: None,
        daily_note_appended: false,
    })
}

/// Build cleanup options from config, loading vocabulary replacements when enabled.
fn build_cleanup_options(config: &Config) -> CleanupOptions {
    let d = &config.dictation;
    let engine = CleanupEngine::parse(&d.cleanup_engine);
    let replacements = if d.cleanup_apply_vocabulary && engine != CleanupEngine::None {
        load_vocab_replacements()
    } else {
        Vec::new()
    };
    CleanupOptions {
        engine,
        remove_fillers: d.cleanup_remove_fillers,
        spoken_punctuation: d.cleanup_spoken_punctuation,
        replacements,
    }
    .with_sorted_replacements()
}

/// Load vocabulary-derived `(surface, canonical)` replacements fresh from the store.
///
/// Read on each utterance finalize (only when vocabulary cleanup is enabled, which is
/// off by default) so edits to the vocabulary store are picked up without restarting.
/// Restricted to `Term`/`Acronym` entries so dictation casing fixes (e.g. "gpt" ->
/// "GPT") apply, while person/org/project names (meant for transcription biasing) are
/// not substituted into prose. Empty if the store is missing or unreadable.
fn load_vocab_replacements() -> Vec<(String, String)> {
    let Ok(store) = crate::vocabulary::load() else {
        return Vec::new();
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    for entry in &store.entries {
        if !matches!(
            entry.kind,
            crate::vocabulary::VocabularyKind::Term | crate::vocabulary::VocabularyKind::Acronym
        ) {
            continue;
        }
        let canonical = entry.canonical.trim().to_string();
        if canonical.is_empty() {
            continue;
        }
        for surface in entry.surface_forms() {
            let surface = surface.trim();
            if !surface.is_empty() {
                pairs.push((surface.to_string(), canonical.clone()));
            }
        }
    }
    pairs
}

fn write_result_outputs(mut result: DictationResult, config: &Config) -> Option<DictationResult> {
    let destination = result.destination.as_str();
    if destination == "clipboard" || destination.is_empty() {
        if let Err(e) = write_to_clipboard(&result.text) {
            tracing::error!("clipboard write failed: {}", e);
        }
    }

    result.file_path = if destination != "daily_note" {
        write_dictation_file(&result.text, result.duration_secs, config)
    } else {
        None
    };

    if config.dictation.daily_note_log {
        result.daily_note_appended = append_dictation_to_daily_note(&result.text, config);
    }

    Some(result)
}

fn finish_session(results: &[DictationResult], config: &Config) -> Option<DictationResult> {
    let mut combined = combine_results(results, config)?;
    combined.file_path = if combined.destination != "daily_note" {
        write_dictation_file(&combined.text, combined.duration_secs, config)
    } else {
        None
    };

    if combined.destination == "clipboard" || combined.destination.is_empty() {
        if let Err(e) = write_to_clipboard(&combined.text) {
            tracing::error!("clipboard write failed: {}", e);
        }
    }

    if config.dictation.daily_note_log {
        combined.daily_note_appended = append_dictation_to_daily_note(&combined.text, config);
    }

    Some(combined)
}

fn combine_results(results: &[DictationResult], config: &Config) -> Option<DictationResult> {
    let parts: Vec<&str> = results
        .iter()
        .map(|result| result.text.trim())
        .filter(|text| !text.is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }
    let raw_parts: Vec<&str> = results
        .iter()
        .map(|result| result.raw_text.trim())
        .filter(|text| !text.is_empty())
        .collect();

    Some(DictationResult {
        raw_text: raw_parts.join(" "),
        text: parts.join(" "),
        duration_secs: results.iter().map(|result| result.duration_secs).sum(),
        destination: config.dictation.destination.clone(),
        file_path: None,
        daily_note_appended: false,
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
    use std::process::Stdio;

    let mut child = crate::engine_process::command("pbcopy")
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
    use std::process::Stdio;

    let mut child = crate::engine_process::command("clip")
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

#[cfg(target_os = "linux")]
fn write_to_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Stdio;

    let candidates = linux_clipboard_write_candidates();
    let mut errors = Vec::new();

    for (program, args) in candidates {
        if which::which(program).is_err() {
            continue;
        }

        let mut child = crate::engine_process::command(program)
            .args(args)
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to spawn {program}: {error}"))?;

        if let Some(mut stdin) = child.stdin.take() {
            if let Err(error) = stdin.write_all(text.as_bytes()) {
                let _ = child.wait();
                errors.push(format!("failed to write to {program}: {error}"));
                continue;
            }
        }

        match child.wait() {
            Ok(status) if status.success() => {
                tracing::debug!(len = text.len(), program, "text written to clipboard");
                return Ok(());
            }
            Ok(_) => errors.push(format!("{program} failed to update the clipboard")),
            Err(error) => errors.push(format!("failed to finish {program}: {error}")),
        }
    }

    if errors.is_empty() {
        Err("clipboard write requires wl-clipboard on Wayland or xclip/xsel on X11".into())
    } else {
        Err(format!("clipboard write failed: {}", errors.join("; ")))
    }
}

#[cfg(target_os = "linux")]
fn linux_clipboard_write_candidates() -> Vec<(&'static str, Vec<&'static str>)> {
    let mut candidates = Vec::new();
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        candidates.push(("wl-copy", Vec::new()));
    }
    if std::env::var_os("DISPLAY").is_some() {
        candidates.push(("xclip", vec!["-selection", "clipboard"]));
        candidates.push(("xsel", vec!["--clipboard", "--input"]));
    }
    candidates
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
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
        processing_warnings: Vec::new(),
        tags: vec![],
        attendees: vec![],
        attendees_raw: None,
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
        capture: None,
        sensitivity: None,
        debrief: None,
        consent: None,
        consent_notice: None,
        visibility: None,
        speaker_map: vec![],
        name_corrections: Vec::new(),
        recording_health: None,
        speaker_mapping: None,
        template: None,
        filter_diagnosis: None,
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
fn append_dictation_to_daily_note(text: &str, config: &Config) -> bool {
    use std::io::Write;

    if !config.daily_notes.enabled {
        return false;
    }

    let note_dir = &config.daily_notes.path;
    if std::fs::create_dir_all(note_dir).is_err() {
        return false;
    }

    let now = Local::now();
    let note_path = note_dir.join(format!("{}.md", now.format("%Y-%m-%d")));

    // Create file with header if it doesn't exist
    if !note_path.exists() {
        if let Err(e) = std::fs::write(&note_path, format!("# {}\n", now.format("%Y-%m-%d"))) {
            tracing::error!("failed to create daily note: {}", e);
            return false;
        }
    }

    // Append-only open to avoid read-modify-write race
    let entry = format!("\n### ~{} - Dictation\n- {}\n", now.format("%H:%M"), text);
    match std::fs::OpenOptions::new().append(true).open(&note_path) {
        Ok(mut f) => {
            if let Err(e) = f.write_all(entry.as_bytes()) {
                tracing::error!("failed to append to daily note: {}", e);
                false
            } else {
                true
            }
        }
        Err(e) => {
            tracing::error!("failed to open daily note for append: {}", e);
            false
        }
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
    whisper_guard::params::num_cpus()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config(root: &std::path::Path) -> Config {
        Config {
            output_dir: root.join("meetings"),
            daily_notes: crate::config::DailyNotesConfig {
                enabled: true,
                path: root.join("daily"),
            },
            dictation: crate::config::DictationConfig {
                destination: "daily_note".into(),
                accumulate: true,
                ..crate::config::DictationConfig::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn model_preflight_returns_model_missing_when_whisper_file_is_absent() {
        let dir = TempDir::new().unwrap();
        let mut config = test_config(dir.path());
        config.transcription.model_path = dir.path().join("models");
        config.dictation.model = "test-model".into();

        assert_eq!(
            preflight_model(&config),
            Err(DictationEvent::ModelMissing {
                model: "test-model".into(),
                expected_path: config
                    .transcription
                    .model_path
                    .join("ggml-test-model.bin")
                    .display()
                    .to_string(),
                setup_command: "minutes setup --model test-model".into(),
            })
        );
    }

    #[test]
    fn model_preflight_proceeds_when_whisper_file_is_present() {
        let dir = TempDir::new().unwrap();
        let mut config = test_config(dir.path());
        config.transcription.model_path = dir.path().join("models");
        config.dictation.model = "test-model".into();
        std::fs::create_dir_all(&config.transcription.model_path).unwrap();
        std::fs::write(
            config.transcription.model_path.join("ggml-test-model.bin"),
            b"test model",
        )
        .unwrap();

        assert_eq!(preflight_model(&config), Ok(()));
    }

    #[test]
    fn model_preflight_surfaces_interrupted_whisper_download_command() {
        let dir = TempDir::new().unwrap();
        let mut config = test_config(dir.path());
        config.transcription.model_path = dir.path().join("models");
        config.dictation.model = "tiny".into();
        std::fs::create_dir_all(&config.transcription.model_path).unwrap();
        let model_path = config.transcription.model_path.join("ggml-tiny.bin");
        std::fs::write(&model_path, b"partial").unwrap();

        assert_eq!(
            preflight_model(&config),
            Err(DictationEvent::ModelMissing {
                model: "tiny".into(),
                expected_path: model_path.display().to_string(),
                setup_command: format!(
                    "rm \"{}\" && minutes setup --model tiny",
                    model_path.display()
                ),
            })
        );
    }

    #[test]
    fn model_preflight_checks_configured_parakeet_model() {
        let dir = TempDir::new().unwrap();
        let mut config = test_config(dir.path());
        config.transcription.model_path = dir.path().join("models");
        config.transcription.parakeet_model = "tdt-600m".into();
        config.dictation.backend = "parakeet".into();

        assert_eq!(
            preflight_model(&config),
            Err(DictationEvent::ModelMissing {
                model: "tdt-600m".into(),
                expected_path: crate::parakeet::install_dir(&config, "tdt-600m")
                    .join(crate::parakeet::default_model_filename("tdt-600m"))
                    .display()
                    .to_string(),
                setup_command: "minutes setup --parakeet --parakeet-model tdt-600m".into(),
            })
        );
    }

    #[test]
    fn run_emits_model_missing_and_returns_before_capture() {
        let dir = TempDir::new().unwrap();
        let mut config = test_config(dir.path());
        config.transcription.model_path = dir.path().join("models");
        config.dictation.model = "test-model".into();
        let mut events = Vec::new();

        let result = run(
            Arc::new(AtomicBool::new(false)),
            &config,
            |event| events.push(event),
            |_| panic!("missing-model preflight must not produce dictation results"),
        );

        assert!(result.is_ok());
        assert_eq!(
            events,
            vec![DictationEvent::ModelMissing {
                model: "test-model".into(),
                expected_path: config
                    .transcription
                    .model_path
                    .join("ggml-test-model.bin")
                    .display()
                    .to_string(),
                setup_command: "minutes setup --model test-model".into(),
            }]
        );
    }

    #[test]
    fn combine_results_joins_text_and_duration() {
        let mut config = Config::default();
        config.dictation.destination = "clipboard".into();
        let results = vec![
            DictationResult {
                raw_text: "first sentence.".into(),
                text: "first sentence.".into(),
                duration_secs: 1.25,
                destination: "clipboard".into(),
                file_path: None,
                daily_note_appended: false,
            },
            DictationResult {
                raw_text: "second sentence.".into(),
                text: "second sentence.".into(),
                duration_secs: 2.75,
                destination: "clipboard".into(),
                file_path: None,
                daily_note_appended: false,
            },
        ];

        let combined = combine_results(&results, &config).unwrap();
        assert_eq!(combined.text, "first sentence. second sentence.");
        assert!((combined.duration_secs - 4.0).abs() < f64::EPSILON);
        assert_eq!(combined.destination, "clipboard");
        assert!(combined.file_path.is_none());
    }

    #[test]
    fn normalize_final_dictation_text_flattens_timestamped_backend_output() {
        let text = normalize_final_dictation_text("[0:00] hello\n[0:01] world").unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn normalize_final_dictation_text_rejects_empty_timestamped_lines() {
        assert_eq!(normalize_final_dictation_text("[0:00] \n\n"), None);
    }

    #[test]
    fn finish_session_writes_one_daily_note_entry_for_combined_text() {
        let dir = TempDir::new().unwrap();
        let config = test_config(dir.path());
        let results = vec![
            DictationResult {
                raw_text: "first sentence.".into(),
                text: "first sentence.".into(),
                duration_secs: 1.0,
                destination: "daily_note".into(),
                file_path: None,
                daily_note_appended: false,
            },
            DictationResult {
                raw_text: "second sentence.".into(),
                text: "second sentence.".into(),
                duration_secs: 2.0,
                destination: "daily_note".into(),
                file_path: None,
                daily_note_appended: false,
            },
        ];

        let final_result = finish_session(&results, &config).unwrap();
        assert_eq!(final_result.text, "first sentence. second sentence.");
        assert!(final_result.file_path.is_none());

        let note_name = format!("{}.md", Local::now().format("%Y-%m-%d"));
        let note_path = config.daily_notes.path.join(note_name);
        let note = std::fs::read_to_string(note_path).unwrap();

        assert!(note.contains("- first sentence. second sentence."));
        assert!(!note.contains("- first sentence.\n"));
        assert!(!note.contains("- second sentence.\n"));
    }
}
