pub mod calendar;
pub mod capture;
pub mod config;
pub mod daily_notes;
pub mod device_monitor;
pub mod diarize;
pub mod error;
pub mod events;
pub mod graph;
pub mod health;
pub mod jobs;
pub mod logging;
pub mod markdown;
pub mod notes;
pub mod pid;
pub mod pipeline;
pub mod screen;
pub mod search;
pub mod summarize;
pub mod transcribe;
pub mod vault;
pub mod voice;
pub mod watch;

// Streaming audio API (for Prompter and other real-time consumers)
#[cfg(feature = "streaming")]
pub mod streaming;
#[cfg(feature = "streaming")]
pub mod vad;

// Streaming whisper (progressive transcription)
#[cfg(feature = "streaming")]
pub mod streaming_whisper;

// Dictation mode (requires streaming + whisper)
#[cfg(feature = "streaming")]
pub mod dictation;

// Live transcript mode (requires streaming + whisper)
#[cfg(feature = "streaming")]
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

#[cfg(feature = "streaming")]
pub use streaming::{AudioChunk, AudioStream};
#[cfg(feature = "streaming")]
pub use vad::{Vad, VadResult};

#[cfg(test)]
pub(crate) fn test_home_env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
