//! Print whisper word timings for one file (#1413).
//!
//! `cargo run -p minutes-core --example word_timings -- <audio> [model]`
//!
//! Runs the ordinary whole-file whisper path and prints each segment's words.
//! A "word timings dropped" warning in the log means token times were not on
//! the segment clock, so no words were kept.

use minutes_core::Config;
use std::path::PathBuf;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "minutes_core=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let mut args = std::env::args().skip(1);
    let audio = PathBuf::from(args.next().expect("usage: word_timings <audio> [model]"));
    let mut config = Config::default();
    if let Some(model) = args.next() {
        config.transcription.model = model;
    }
    let result = minutes_core::transcribe::transcribe(&audio, &config).expect("transcribe");
    let with_words = result
        .segments
        .iter()
        .filter(|s| !s.words.is_empty())
        .count();
    println!(
        "{} segments, {} with words",
        result.segments.len(),
        with_words
    );
    for segment in &result.segments {
        println!(
            "[{:7.2} - {:7.2}] {}",
            segment.start, segment.end, segment.text
        );
        for word in &segment.words {
            println!("    {:7.2} - {:7.2}  {}", word.start, word.end, word.text);
        }
    }
}
