//! # whisper-guard
//!
//! Anti-hallucination toolkit for [whisper-rs](https://crates.io/crates/whisper-rs).
//!
//! Whisper's decoder can hallucinate — repeating phrases, generating phantom
//! `[music]` tags, or looping on silence. This crate provides battle-tested
//! defenses at two levels:
//!
//! **Pre-transcription audio preparation** (`audio` module):
//! - Silence stripping with adaptive noise floor
//! - Auto-normalization for quiet microphones
//! - Windowed-sinc resampling (32-tap Hann, alias-free)
//!
//! **Post-transcription segment cleaning** (`segments` module):
//! - Consecutive repetition detection (3+ similar segments collapsed)
//! - Interleaved A/B/A/B hallucination pattern detection
//! - Foreign script hallucination detection (e.g., CJK in a Latin transcript)
//! - Trailing noise trimming (`[music]`, `[BLANK_AUDIO]`, filler)
//!
//! **Whisper parameter presets** (`params` module, requires `whisper` feature):
//! - Batch transcription params matching whisper-cli defaults
//! - Low-latency streaming params
//!
//! ## Quick Start
//!
//! ```rust
//! use whisper_guard::segments::clean_transcript;
//!
//! let raw_transcript = "[0:00] Hello world\n[0:03] Hello world\n[0:06] Hello world\n[0:09] Hello world\n[0:12] Something different\n";
//! let (cleaned, stats) = clean_transcript(raw_transcript);
//! assert!(stats.lines_removed > 0);
//! ```

pub mod audio;
pub mod segments;

#[cfg(feature = "whisper")]
pub mod params;

// Re-export the most common entry points
pub use audio::{normalize_audio, resample, strip_silence};
pub use segments::{clean_transcript, CleanStats};
