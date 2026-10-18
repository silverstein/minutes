//! Explicit, bounded recovery of RF64/BW64 or oversized native WAV stems.
//!
//! The original is never modified. Each part retains the exact PCM bytes,
//! sample rate and channels and fits both RIFF and Minutes' processing budget.
pub use crate::stem_probe::{recover_stem, RecoveredStem, RecoveredStemPart};
