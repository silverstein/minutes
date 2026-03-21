pub mod calendar;
pub mod capture;
pub mod config;
pub mod daily_notes;
pub mod diarize;
pub mod error;
pub mod events;
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

// Re-export commonly used types
pub use config::Config;
pub use error::{MinutesError, Result};
pub use markdown::{ContentType, WriteResult};
pub use pid::CaptureMode;
pub use pipeline::process;
