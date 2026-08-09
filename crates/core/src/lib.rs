#![warn(clippy::disallowed_methods)]

// `engine-sherpa` and `diarize` cannot share one macOS binary (issue #683).
//
// sherpa-onnx's static bundle and pyannote's dependencies each vendor two of
// the same native libraries: ONNX Runtime (sherpa 1.17.1 vs ort-sys 1.22.0)
// and kaldi-native-fbank (both literally named libkaldi-native-fbank-core.a).
// A Mach-O image has one definition per symbol, so whichever copy wins the
// link serves both consumers. Measured on macOS arm64, every resolution is
// broken in one direction or the other:
//
//   ORT winner   kaldi winner   pyannote            sherpa
//   1.17.1       sherpa's       API 17/22 error     works
//   1.22.0       sherpa's       SIGABRT in fbank    works
//   1.22.0       knf-rs's       works               SIGABRT
//
// ORT could be unified upward because its C API is version-stable, but
// kaldi-native-fbank has no ABI contract, and identical mangled names with
// divergent layouts are undefined behavior, not a clean failure. Refusing to
// link is the only honest option until the sherpa stack is isolated behind
// its own namespace (dlopen plugin or worker process; see the tracking issue
// referenced from #683).
//
// Linux is verified unaffected: with both stacks linked into one binary
// (sherpa via libsherpa-onnx-c-api.so, pyannote via static ort 1.22.0),
// enrollment saves a voice profile and sherpa transcribes, in one process
// (x86_64, 2026-08-09). The dynamic sherpa library resolves ONNX Runtime from
// its own DT_NEEDED libonnxruntime.so rather than the executable, which does
// not export the colliding symbols. Windows is expected safe by the same
// dynamic-linking structure (per-DLL imports), but has not been executed in
// this combination; if sherpa ever links statically off macOS, this guard's
// scope must widen.
//
// `vad-ort` is included because it also pulls `dep:ort`: the same two static
// ONNX Runtimes collide even without pyannote in the picture, and which copy
// wins is a link-order coin flip. The measured 1.17.1-wins outcome hard-fails
// every ort-rs consumer at init with the API 17/22 error.
//
// For a sherpa CLI build without diarization:
//   cargo build --release -p minutes-cli --no-default-features \
//     --features whisper,engine-sherpa,metal
#[cfg(all(
    target_os = "macos",
    feature = "engine-sherpa",
    any(feature = "diarize", feature = "vad-ort")
))]
compile_error!(
    "engine-sherpa cannot share a macOS binary with diarize or vad-ort: sherpa-onnx's static \
     bundle vendors its own ONNX Runtime (and kaldi-native-fbank), and whichever copy wins \
     the link breaks the other consumer (issue #683). Build sherpa without them: \
     cargo build -p minutes-cli --no-default-features --features whisper,engine-sherpa,metal"
);

pub mod apple_fm;
pub mod apple_speech;
pub mod apple_speech_worker;
pub(crate) mod audio_budget;
pub mod audio_decode_worker;
pub mod autoresearch;
pub(crate) mod bounded_child;
#[cfg(target_os = "macos")]
pub(crate) mod macos_graph_xpc;

/// Activate a previously validated MCP process-audio outer process group.
/// The CLI must also hold the helper's live supervisor capability; the core
/// repeats the Unix parent/group topology checks before changing child-launch
/// behavior.
#[cfg(unix)]
pub fn install_validated_outer_process_group(process_group: i32) -> std::io::Result<()> {
    bounded_child::install_validated_outer_process_group(process_group)
}
#[cfg(any(test, all(feature = "streaming", feature = "whisper")))]
pub(crate) mod bounded_inference;
pub mod calendar;
pub mod capture;
pub mod config;
pub mod context_store;
pub mod copilot;
pub mod daily_notes;
pub mod derived;
pub mod desktop_context;
pub mod desktop_control;
pub mod device_monitor;
pub mod diarize;
pub mod dictation_cleanup;
pub mod dictation_memory;
pub(crate) mod engine_process;
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
pub mod graph_worker;
pub mod health;
pub mod i18n;
pub mod jobs;
pub mod knowledge;
pub mod knowledge_extract;
pub mod live_partials;
pub mod live_session;
pub mod live_sidekick;
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
pub mod ollama;
pub mod overlays;
pub mod palette;
pub mod parakeet;
pub mod parakeet_sidecar;
pub(crate) mod person_identity;
pub mod pid;
pub mod pipeline;
pub mod policy_fs;
pub mod process_trace;
pub mod resummarize;
pub mod retention;
// Shared mono-downmix + decimation resampler (used by capture and streaming)
pub(crate) mod resample;
pub mod screen;
#[cfg(any(target_os = "macos", target_os = "windows", all(test, unix)))]
pub(crate) mod sealed_audio;
pub mod search;
pub mod search_index;
pub mod sensitive;
// Always compiled: the path/resolution helpers are pure std/Config so `setup`
// can install models without the engine. Only `transcribe_samples` is gated.
pub mod sherpa_engine;
pub(crate) mod stem_probe;
/// Incremental reader for stems that are still being written (#576).
pub mod stem_tail;
#[cfg(feature = "streaming-diarize")]
pub mod streaming_diarize;
pub mod summarize;
pub mod system_audio_backend;
pub mod template;
/// Test scaffolding shared by unit and integration tests. Not public API.
#[doc(hidden)]
pub mod test_support;
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
/// Whether a worker-capable binary sits beside the test harness, answered once.
///
/// Several tests assert this as a precondition rather than skipping silently, and
/// each answer copies the ~250 MB debug executable into an immutable snapshot.
///
/// WHAT THIS ACTUALLY SAVES, measured rather than assumed, because the first
/// version of this comment cited a 26% suite regression that does not reproduce:
/// about 0.4 s across the three call sites, two avoided binds at roughly 0.2 s
/// each. Run-to-run noise on the same machine is larger than that. It is kept
/// because it is free and directionally right, not because it rescued anything.
///
/// "The answer cannot change during a run" would be too strong: `bind` can fail
/// transiently under memory pressure or a full temp filesystem, which is the
/// documented flake behind track-1 item 10. What is true is that a transient
/// first-call failure caches `false` and then fails all three preconditions
/// loudly, which is the safe direction.
#[cfg(test)]
pub(crate) fn test_worker_binary_is_available() -> bool {
    use std::sync::OnceLock;

    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        crate::audio_decode_worker::bounded_decode_fallback_available(
            &crate::config::Config::default(),
        )
    })
}

#[cfg(test)]
pub(crate) fn test_home_env_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::test_support::home_env_lock()
}
