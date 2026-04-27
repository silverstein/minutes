//! End-to-end integration test that exercises `clean_segments` the way a typical
//! whisper-rs caller would: collect raw segment text from the model and pipe it
//! through whisper-guard before joining into a transcript.
//!
//! This file uses ONLY the public API and does NOT pull in the `whisper` feature,
//! exercising the same path a fork-using consumer (e.g. screenpipe-audio) would
//! follow with `default-features = false`.

use whisper_guard::{clean_segments, clean_segments_with_options, CleanOptions};

/// Simulated whisper output that exercises every guard at once: a consecutive loop,
/// trailing noise, and a "stop recording" voice command at the tail.
fn realistic_whisper_output() -> Vec<String> {
    vec![
        " Welcome to the standup.".into(),
        " Quick updates from each of you.".into(),
        " Thank you.".into(),
        " Thank you.".into(),
        " Thank you.".into(),
        " Thank you.".into(),
        " Thank you.".into(),
        " Marcus shipped the inference pipeline.".into(),
        " Priya finished the dashboard.".into(),
        " Action items are in the doc.".into(),
        " [music]".into(),
        " [music]".into(),
        " Stop recording.".into(),
    ]
}

#[test]
fn fork_user_path_collapses_loops_and_strips_trailing_command() {
    let raw = realistic_whisper_output();
    let (cleaned, stats) = clean_segments(&raw);

    // Real content survives.
    let joined = cleaned.join(" ");
    assert!(joined.contains("Marcus shipped"), "real content removed");
    assert!(joined.contains("Priya finished"), "real content removed");
    assert!(joined.contains("Action items"), "real content removed");

    // Trailing voice command was stripped.
    assert!(
        !joined.contains("Stop recording"),
        "voice command should be stripped"
    );

    // The "Thank you." x5 loop was collapsed to first-occurrence + annotation.
    let thank_you_lines = cleaned.iter().filter(|s| s.trim() == "Thank you.").count();
    assert_eq!(
        thank_you_lines, 1,
        "expected loop to collapse to one occurrence"
    );
    assert!(
        cleaned.iter().any(|s| s.contains("repeated audio removed")),
        "expected dedup annotation line"
    );

    // Statistics are sensible.
    assert_eq!(stats.original_lines, 13);
    // 4 from the 5-segment dedup (5 → 2, removing 3) + 1 from "Stop recording." command strip.
    assert!(
        stats.lines_removed >= 4,
        "expected ≥4 removals, got {}",
        stats.lines_removed
    );
}

#[test]
fn opt_out_of_voice_command_strip() {
    // Some users may want to keep the voice command in the transcript.
    let raw = realistic_whisper_output();
    let opts = CleanOptions {
        strip_trailing_commands: false,
        ..CleanOptions::default()
    };
    let (cleaned, _) = clean_segments_with_options(&raw, &opts);
    assert!(
        cleaned.iter().any(|s| s.contains("Stop recording")),
        "command should survive when guard is disabled"
    );
}

#[test]
fn idempotent_under_repeated_application() {
    // Running clean_segments twice should be a no-op the second time.
    let raw = realistic_whisper_output();
    let (first, _) = clean_segments(&raw);
    let (second, second_stats) = clean_segments(&first);
    assert_eq!(first, second);
    assert_eq!(second_stats.lines_removed, 0);
}

#[test]
fn handles_pathological_input_without_panic() {
    // 100k all-identical segments — common silent-input failure mode.
    let raw: Vec<String> = (0..100_000).map(|_| " Thank you.".into()).collect();
    let (cleaned, stats) = clean_segments(&raw);
    assert_eq!(stats.original_lines, 100_000);
    assert!(cleaned.len() < 10, "loop should collapse to a tiny output");
}

#[test]
fn preserves_segment_count_when_no_hallucinations_present() {
    let raw: Vec<String> = vec![
        " Welcome to the meeting.".into(),
        " Three updates from each team.".into(),
        " Engineering shipped the new pipeline.".into(),
        " Sales closed two enterprise deals.".into(),
        " Marketing is launching the campaign Tuesday.".into(),
    ];
    let (cleaned, stats) = clean_segments(&raw);
    assert_eq!(cleaned, raw, "clean input must be untouched");
    assert_eq!(stats.lines_removed, 0);
}

/// Regression guard for the pipeline-order fix in 0.2.0:
/// noise markers hidden behind a trailing voice command must be cleaned up.
///
/// Before 0.2.0 this scenario left `[music]` in the output because trim ran
/// before command-strip and was blocked by the trailing command. Now command-
/// strip runs first, exposing the trailing noise to trim. Combined with
/// dedup-skips-noise (always-noise tokens flow past dedup unchanged), this
/// works for any count of trailing markers.
#[test]
fn trailing_noise_behind_command_is_cleaned_up() {
    let raw: Vec<String> = vec![
        " Real content.".into(),
        " More real content.".into(),
        " [music]".into(),
        " [music]".into(),
        " [music]".into(),
        " Stop recording.".into(),
    ];
    let (cleaned, _stats) = clean_segments(&raw);
    let joined = cleaned.join(" ");
    assert!(
        !joined.contains("Stop recording"),
        "voice command should be stripped"
    );
    assert!(
        !joined.contains("[music]"),
        "trailing noise behind command must be trimmed (was a 0.1.x limitation)"
    );
    assert!(joined.contains("Real content"), "real content must survive");
}

/// Same scenario, but with only ONE trailing noise marker. The 0.2.0 pipeline
/// fix (split is_always_noise from is_filler in trim_trailing_noise) means
/// even a single trailing always-noise marker now gets cleaned up — no 5-line
/// floor for unambiguous noise tokens.
#[test]
fn single_trailing_always_noise_marker_is_trimmed() {
    let raw: Vec<String> = vec![
        " Real content.".into(),
        " More real content.".into(),
        " [music]".into(),
    ];
    let (cleaned, _stats) = clean_segments(&raw);
    let joined = cleaned.join(" ");
    assert!(
        !joined.contains("[music]"),
        "single trailing always-noise marker should still be trimmed"
    );
}

/// Filler words ("Yeah.", "Okay.", "Thanks.") at the end of a transcript MUST
/// survive — they're often legitimate one-word closings. Only a 5+ run of
/// filler triggers the trim. Critical regression guard.
#[test]
fn legitimate_terse_closing_is_preserved() {
    let raw: Vec<String> = vec![" That's a wrap on this sprint.".into(), " Yeah.".into()];
    let (cleaned, _) = clean_segments(&raw);
    assert!(
        cleaned.iter().any(|s| s.contains("Yeah")),
        "single-filler closing must survive trim"
    );
}
