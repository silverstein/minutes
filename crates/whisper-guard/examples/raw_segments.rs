//! The most common whisper-guard call site: take raw segments out of a transcription
//! engine, run them through the default cleaning pipeline, and emit the result.
//!
//! Run with: `cargo run --example raw_segments`
//!
//! This example uses synthetic segments so it has no whisper-rs dependency. In a
//! real consumer, the `raw_segments` vector would come from
//! `whisper_state.get_segment(i).to_str()` (or whatever your engine surfaces).

use whisper_guard::clean_segments;

fn main() {
    // Pretend these came from whisper transcribing a 30-second clip.
    // Includes a five-segment hallucination loop and a trailing voice command.
    let raw_segments: Vec<String> = vec![
        " Good morning, everyone.".into(),
        " Let's start with the engineering update.".into(),
        " Thank you.".into(), // <- whisper hallucination loop on a quiet beat
        " Thank you.".into(),
        " Thank you.".into(),
        " Thank you.".into(),
        " Thank you.".into(),
        " Marcus, you're up.".into(),
        " The new pipeline is live in staging.".into(),
        " Stop recording.".into(), // <- voice command at the tail
    ];

    let (cleaned, stats) = clean_segments(&raw_segments);

    println!("=== Cleaned transcript ===");
    for line in &cleaned {
        println!("{line}");
    }

    println!();
    println!("=== Stats ===");
    println!("{}", stats.summary());
    println!("  original:           {}", stats.original_lines);
    println!("  after dedup:        {}", stats.after_consecutive_dedup);
    println!("  after script:       {}", stats.after_script_filter);
    println!("  after noise marker: {}", stats.after_noise_markers);
    println!("  after trailing:     {}", stats.after_trailing_trim);
    println!("  after command:      {}", stats.after_command_strip);
    println!("  total removed:      {}", stats.lines_removed);
}
