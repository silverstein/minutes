pub mod apple_fm;
pub mod apple_speech;
pub mod autoresearch;
pub mod calendar;
pub mod capture;
pub mod config;
pub mod context_store;
pub mod daily_notes;
pub mod desktop_context;
pub mod desktop_control;
pub mod device_monitor;
pub mod diarize;
pub mod dictation_cleanup;
pub mod dictation_memory;
/// Person entity-resolution clustering (issue #385, class 3): suggestion-only
/// grouping of name-variant fragments. Never merges.
pub(crate) mod entity_cluster;
/// Entity-resolution evaluation harness (cluster-level). Test-only; the
/// measurement contract for the entity-clustering lever (issue #385 / #371).
#[cfg(test)]
mod entity_resolution_eval;
pub mod error;
pub mod events;
pub mod ffmpeg;
pub mod graph;
pub mod health;
pub mod jobs;
pub mod knowledge;
pub mod knowledge_extract;
pub mod live_session;
pub mod logging;
pub mod macos_permissions;
pub mod markdown;
/// Post-pass name correction (config-gated, off by default): the big lever of
/// the name-accuracy epic (bead minutes-25x3.4).
pub mod name_correction;
/// Name-accuracy evaluation harness (text-level). Test-only; the measurement
/// contract for the post-pass name-correction lever (bead minutes-25x3.4).
#[cfg(test)]
mod name_eval;
pub mod notes;
pub mod overlays;
pub mod palette;
pub mod parakeet;
pub mod parakeet_sidecar;
pub(crate) mod person_identity;
pub mod pid;
pub mod pipeline;
pub mod process_trace;
pub mod retention;
// Shared mono-downmix + decimation resampler (used by capture and streaming)
pub(crate) mod resample;
pub mod screen;
pub mod search;
pub mod search_index;
pub mod sensitive;
// Always compiled: the path/resolution helpers are pure std/Config so `setup`
// can install models without the engine. Only `transcribe_samples` is gated.
pub mod sherpa_engine;
#[cfg(feature = "streaming-diarize")]
pub mod streaming_diarize;
pub mod summarize;
pub mod system_audio_backend;
pub mod template;
pub mod transcribe;
pub mod transcription_coordinator;
pub mod vault;
pub mod vocabulary;
pub mod voice;
pub mod watch;

// Streaming audio API (for Prompter and other real-time consumers)
#[cfg(feature = "streaming")]
pub mod streaming;
#[cfg(feature = "streaming")]
pub mod vad;

// Silero VAD smoothing FSM. Independent of ort so unit tests cover
// the smoothing logic with synthetic probability streams; the ort
// session in `silero_vad` (when the `vad-ort` feature is on) feeds
// real probabilities into the same FSM.
#[cfg(feature = "streaming")]
pub mod silero_smoothing;

// Streaming Silero VAD via ort (ONNX Runtime). Only compiled when
// the user opts into the `vad-ort` feature; default builds keep
// using whisper-rs's bundled Silero (`SileroSidecarVad` in
// `live_transcript`).
#[cfg(feature = "vad-ort")]
pub mod silero_vad;

// Streaming whisper (progressive transcription) — requires both features.
// These modules use whisper_rs + whisper_guard::params internally, so they
// can only compile when the whisper backend is enabled. Downstream consumers
// that enable `streaming` alone (e.g. Prompter, which does its own whisper
// via whisper-rs directly) must not pull these in. The `all(...)` gate
// matches the existing pattern at capture.rs:803.
#[cfg(all(feature = "streaming", feature = "whisper"))]
pub mod streaming_whisper;

// Dictation mode (requires streaming + whisper)
#[cfg(all(feature = "streaming", feature = "whisper"))]
pub mod dictation;

// Live transcript mode (requires streaming + whisper)
#[cfg(all(feature = "streaming", feature = "whisper"))]
pub mod live_transcript;

// Native macOS hotkey monitoring via CGEventTap
#[cfg(target_os = "macos")]
pub mod hotkey_macos;

// Re-export commonly used types
pub use config::Config;
pub use error::{MinutesError, Result};
pub use markdown::{ContentType, WriteResult};
pub use pid::CaptureMode;
pub use pipeline::process;
pub use template::{Template, TemplateResolver, TemplateSource, DEFAULT_TEMPLATE_SLUG};

#[cfg(feature = "streaming")]
pub use streaming::{AudioChunk, AudioStream};
#[cfg(feature = "streaming")]
pub use vad::{Vad, VadEngine, VadResult};

/// Route whisper.cpp + ggml C-level logs through the Rust `tracing`
/// subscriber instead of leaking to raw stderr. Without this hook the C
/// loggers bypass every filter the host process sets up, which is what
/// made the `whisper_vad_detect_speech: detect speech (X.XXs duration)`
/// line flood terminals during a recording (issue #163).
///
/// **Call exactly once at process startup, before any whisper context is
/// created.** The underlying `whisper_rs::install_logging_hooks()` wires a
/// global C-level trampoline that is permanent for the life of the
/// process and cannot be replaced. Subsequent calls are silently ignored,
/// so this is safe to call defensively from multiple entry points (CLI
/// main, Tauri main, MCP server) but each entry point should call it at
/// most once.
///
/// If the host process has no tracing subscriber, the C log events
/// become events with no recipient and are silently dropped. That is
/// fine for the bug we are fixing here. If a future change adds a
/// tracing subscriber to that process, set `whisper_rs=warn` and
/// `ggml=warn` in the filter so the chatty INFO logs do not flood again.
///
/// On builds without the `whisper` feature this is a no-op so callers can
/// invoke it unconditionally.
pub fn install_whisper_logging_hooks() {
    #[cfg(feature = "whisper")]
    whisper_rs::install_logging_hooks();
}

#[cfg(test)]
pub(crate) fn test_home_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
