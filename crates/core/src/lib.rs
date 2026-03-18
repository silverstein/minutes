pub mod config;
pub mod error;
pub mod logging;
pub mod markdown;
pub mod pid;
pub mod pipeline;
pub mod search;
pub mod transcribe;
pub mod watch;

// Re-export commonly used types
pub use config::Config;
pub use error::{MinutesError, Result};
pub use markdown::{ContentType, WriteResult};
pub use pipeline::process;
