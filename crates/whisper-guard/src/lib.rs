//! # whisper-guard
//!
//! The post-processing layer Whisper should have shipped with.
//!
//! [whisper.cpp](https://github.com/ggerganov/whisper.cpp) and its bindings
//! ([whisper-rs](https://crates.io/crates/whisper-rs), and many forks) hallucinate in
//! predictable, well-documented ways: looping on silence, generating phantom `[music]`
//! tags, drifting into a foreign script when the audio is too quiet, gluing voice
//! commands like *"stop recording"* onto the end of every transcript.
//!
//! whisper-guard catches the common patterns, with defaults tuned in production by
//! [Minutes](https://github.com/silverstein/minutes), an OSS meeting-memory tool that
//! processes meeting and voice-memo audio across multiple languages.
//!
//! ## Quick start
//!
//! If you already have `Vec<String>` segments from a transcription engine
//! (`whisper_state.get_segment(i).to_str()`, a parakeet sidecar, a fork - anything):
//!
//! ```
//! use whisper_guard::clean_segments;
//!
//! let raw = vec![
//!     "Thank you.".to_string(),
//!     "Thank you.".to_string(),
//!     "Thank you.".to_string(),
//!     "Thank you.".to_string(),
//!     "What's the budget for this quarter?".to_string(),
//! ];
//!
//! let (cleaned, stats) = clean_segments(&raw);
//! assert!(cleaned.iter().any(|s| s.contains("budget")));
//! println!("{}", stats.summary());
//! // → whisper-guard: 5 → 3 segments (2 removed)
//! //   (the loop collapses to first-occurrence + an annotation line; see CleanOptions)
//! ```
//!
//! That's the whole API for the common case. No builders, no setup, no engine
//! coupling. Six guards run in a fixed order; opt out individually via
//! [`CleanOptions`] when you have a good reason.
//!
//! ## Works with any whisper variant
//!
//! whisper-guard's segment/audio modules are **pure Rust with no whisper-rs dependency**.
//! If you depend on a forked or pinned `whisper-rs` (common for Metal/CUDA tuning,
//! GPU patches, or model compatibility), use:
//!
//! ```toml
//! whisper-guard = { version = "0.2", default-features = false }
//! ```
//!
//! …and the cleaning pipeline works regardless of which whisper-rs is in your tree.
//! The optional `whisper` feature only adds [`params`] presets that wrap
//! `whisper_rs::FullParams`.
//!
//! ## What it catches
//!
//! **Pre-transcription audio prep** ([`audio`]):
//! - Silence stripping with adaptive noise floor
//! - Auto-normalization for quiet microphones
//! - Windowed-sinc resampling (32-tap Hann, alias-free) for `44.1k → 16k`
//!
//! **Post-transcription segment cleaning** ([`segments`]):
//! - Consecutive repetition (3+ similar segments collapsed)
//! - Interleaved A/B/A/B hallucination patterns
//! - Bracketed noise marker collapse (`[Śmiech]`, `[music]`, `[risas]`, any language)
//! - Foreign-script hallucination detection (e.g., CJK in a Latin transcript)
//! - Trailing noise trimming (`[music]`, `[BLANK_AUDIO]`, filler at the end)
//! - Voice command stripping (`stop recording`, `end recording` at the tail)
//!
//! **Whisper parameter presets** ([`params`], requires `whisper` feature):
//! - Batch transcription params matching `whisper-cli` defaults
//! - Low-latency streaming params

pub mod audio;
pub mod segments;

#[cfg(feature = "whisper")]
pub mod params;

// Re-export the most common entry points
pub use audio::{normalize_audio, resample, strip_silence};
pub use segments::{
    clean_segments, clean_segments_with_options, clean_transcript, strip_trailing_commands,
    CleanOptions, CleanStats,
};
