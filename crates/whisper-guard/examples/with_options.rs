//! Advanced configuration: opt out of behaviors that don't fit your pipeline.
//!
//! Run with: `cargo run --example with_options`
//!
//! Real-world reasons to do this:
//! - Live captions where every utterance matters: disable `dedup_consecutive` so
//!   genuine repetition (`"go go go"`) isn't collapsed.
//! - Output stream is going to an LLM: disable `keep_dedup_annotations` so the
//!   `[...] [repeated audio removed]` markers don't pollute the prompt.
//! - Music podcast: disable `collapse_noise_markers` so `[music]` annotations stay.

use whisper_guard::{clean_segments, clean_segments_with_options, CleanOptions};

fn main() {
    // A hallucination loop in the middle of real content.
    let segments: Vec<String> = vec![
        " Welcome to the standup.".into(),
        " Quick updates from each of you.".into(),
        " Thank you.".into(),
        " Thank you.".into(),
        " Thank you.".into(),
        " Thank you.".into(),
        " Thank you.".into(),
        " Marcus shipped the inference pipeline.".into(),
        " Action items are in the doc.".into(),
    ];

    println!("=== Default behavior: dedup keeps annotation line ===");
    let (cleaned, _) = clean_segments(&segments);
    for line in &cleaned {
        println!("  {line}");
    }

    println!();
    println!("=== Suppress annotations (clean output for an LLM prompt) ===");
    let opts = CleanOptions {
        keep_dedup_annotations: false,
        ..CleanOptions::default()
    };
    let (cleaned, stats) = clean_segments_with_options(&segments, &opts);
    for line in &cleaned {
        println!("  {line}");
    }
    println!();
    println!("Net removed: {}", stats.lines_removed);
}
