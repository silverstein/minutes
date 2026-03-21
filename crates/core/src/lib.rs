pub mod calendar;
pub mod capture;
pub mod config;
pub mod daily_notes;
pub mod diarize;
pub mod error;
pub mod events;
pub mod health;
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
pub mod watch;

// Streaming audio API (for Prompter and other real-time consumers)
#[cfg(feature = "streaming")]
pub mod streaming;
#[cfg(feature = "streaming")]
pub mod vad;

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
