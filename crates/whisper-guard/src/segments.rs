//! Post-transcription segment cleaning for whisper output.
//!
//! Whisper's decoder can hallucinate in several patterns:
//! - **Consecutive repetition**: the same phrase repeated 5-50 times
//! - **Interleaved repetition**: A/B/A/B patterns with filler words between
//! - **Trailing noise**: `[music]`, `[BLANK_AUDIO]` tags after speech ends
//!
//! This module detects and removes all three patterns. The main entry point
//! is [`clean_transcript`], which chains all cleaning passes.

/// Extract the text portion after the timestamp bracket.
/// Lines look like `[0:00] some text` or plain text.
fn text_part(line: &str) -> &str {
    line.find("] ").map(|i| &line[i + 2..]).unwrap_or(line)
}

/// Whisper noise tokens that are NEVER legitimate transcript content.
/// These can be trimmed at any count and should be skipped by the dedup pass
/// so the dedicated noise handlers (`collapse_noise_markers`, `trim_trailing_noise`)
/// can deal with them.
fn is_always_noise(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    t == "[music]" || t == "[blank_audio]" || t == "[silence]" || t == "music"
}

/// Statistics from transcript cleaning.
///
/// Each `after_*` field records the segment count *after* that pass ran. If a pass
/// is disabled in [`CleanOptions`], its field carries the count from the previous
/// (enabled) pass - making it safe to compute pass-level deltas like
/// `stats.original_lines - stats.after_consecutive_dedup` without checking which
/// passes ran.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct CleanStats {
    pub original_lines: usize,
    pub after_consecutive_dedup: usize,
    pub after_interleaved_dedup: usize,
    pub after_script_filter: usize,
    pub after_noise_markers: usize,
    pub after_trailing_trim: usize,
    pub after_command_strip: usize,
    /// **Net** segment count delta (`original_lines - after_noise_markers`,
    /// where `after_noise_markers` is the final output count post-pipeline),
    /// not the raw count of input lines that were dropped.
    ///
    /// The dedup pass inserts a single `[...] [repeated audio removed]`
    /// annotation line in place of each collapsed run, so collapsing 5
    /// inputs to 1 occurrence + 1 annotation produces a net change of `-3`.
    /// To get the cleaner "input minus output" count, suppress the annotations
    /// via [`CleanOptions::keep_dedup_annotations`] = `false`.
    pub lines_removed: usize,
}

impl CleanStats {
    /// Compact one-line summary for logging.
    ///
    /// ```
    /// use whisper_guard::segments::{clean_segments, CleanStats};
    ///
    /// let (_, stats) = clean_segments(&[
    ///     "Thank you.".into(),
    ///     "Thank you.".into(),
    ///     "Thank you.".into(),
    ///     "Real content here.".into(),
    /// ]);
    /// assert!(stats.summary().starts_with("whisper-guard:"));
    /// ```
    pub fn summary(&self) -> String {
        // `after_noise_markers` is the final segment count post-pipeline
        // (collapse_noise_markers runs last; see clean_segments_with_options).
        format!(
            "whisper-guard: {} → {} segments ({} removed)",
            self.original_lines, self.after_noise_markers, self.lines_removed,
        )
    }
}

/// Toggles for each cleaning pass.
///
/// All passes default to enabled - `CleanOptions::default()` matches the production
/// configuration used by [Minutes](https://github.com/silverstein/minutes).
/// Use [`CleanOptions::none`] as a starting point if you want to enable only specific passes.
///
/// Passes always run in this order (fixed; the order matters for correctness):
///
/// 1. `dedup_consecutive` - collapse runs of repeated real-content segments.
///    Always-noise tokens (`[music]`, `[blank_audio]`, `[silence]`, `music`) are
///    skipped here so the noise-aware passes downstream can handle them.
/// 2. `dedup_interleaved` - collapse A/B/A/B hallucination patterns
/// 3. `strip_foreign_script` - drop segments in unrelated writing systems
/// 4. `strip_trailing_commands` - strip `stop recording`-style voice commands.
///    Runs BEFORE trim so noise markers hidden behind a trailing command get
///    exposed to the trim pass.
/// 5. `trim_trailing_noise` - trim noise markers off the end. Always-noise
///    tokens get trimmed at any count; filler words (`yeah.`, `okay.`, `you`)
///    need a 5+ run to trigger.
/// 6. `collapse_noise_markers` - collapse middle-of-transcript `[music]`/
///    `[Śmiech]`/etc. runs. Runs LAST so trim has first crack at trailing
///    noise; whatever survives in the middle gets collapsed cleanly.
///
/// ```
/// use whisper_guard::segments::{clean_segments_with_options, CleanOptions};
///
/// // Only run the two dedup passes; leave foreign script and noise markers alone.
/// let opts = CleanOptions {
///     dedup_consecutive: true,
///     dedup_interleaved: true,
///     ..CleanOptions::none()
/// };
///
/// let (cleaned, stats) = clean_segments_with_options(
///     &["Hello.".into(), "Hello.".into(), "Hello.".into(), "World.".into()],
///     &opts,
/// );
/// // 3 "Hello." + 1 "World." → "Hello." + dedup-annotation + "World."
/// assert_eq!(cleaned.len(), 3);
/// assert!(cleaned.iter().any(|s| s.contains("World")));
/// assert!(stats.lines_removed >= 1);
/// ```
// NOTE: deliberately NOT `#[non_exhaustive]`. Functional record update
// (`..CleanOptions::default()`) is the primary ergonomic pattern for this struct,
// and `#[non_exhaustive]` blocks it from external crates. New fields will be
// added as minor-version bumps with a CHANGELOG entry.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CleanOptions {
    pub dedup_consecutive: bool,
    pub dedup_interleaved: bool,
    pub strip_foreign_script: bool,
    pub collapse_noise_markers: bool,
    pub trim_trailing_noise: bool,
    pub strip_trailing_commands: bool,
    /// Keep the `[...] [repeated audio removed - N identical segments collapsed]`
    /// annotation lines that the consecutive-dedup pass inserts.
    ///
    /// `true` (default) preserves them as a human-readable trail of what was
    /// stripped. Set to `false` for a cleaner output stream - useful when the
    /// segments are about to be fed to an LLM, joined into a flat string, or
    /// otherwise consumed by code rather than read by a human.
    pub keep_dedup_annotations: bool,
}

impl Default for CleanOptions {
    /// All passes enabled and dedup annotations preserved.
    /// Matches the production tuning used by Minutes.
    fn default() -> Self {
        Self {
            dedup_consecutive: true,
            dedup_interleaved: true,
            strip_foreign_script: true,
            collapse_noise_markers: true,
            trim_trailing_noise: true,
            strip_trailing_commands: true,
            keep_dedup_annotations: true,
        }
    }
}

impl CleanOptions {
    /// All passes enabled. Equivalent to `CleanOptions::default()`.
    pub fn all() -> Self {
        Self::default()
    }

    /// All passes disabled. Useful as a base when you want to enable specific
    /// passes from scratch via the `..CleanOptions::none()` shorthand.
    ///
    /// Note: `keep_dedup_annotations` stays `true` here - it controls how dedup
    /// emits its output, not whether dedup runs. If you opt back into
    /// `dedup_consecutive`, you get the same annotation-emitting behavior as
    /// the default config; suppress annotations explicitly with
    /// `keep_dedup_annotations: false` if you don't want them.
    pub fn none() -> Self {
        Self {
            dedup_consecutive: false,
            dedup_interleaved: false,
            strip_foreign_script: false,
            collapse_noise_markers: false,
            trim_trailing_noise: false,
            strip_trailing_commands: false,
            keep_dedup_annotations: true,
        }
    }
}

/// Prefix used to identify dedup annotation lines so they can be filtered.
const DEDUP_ANNOTATION_PREFIX: &str = "[...] [repeated audio removed";

/// Clean a list of raw transcript segments.
///
/// **This is the entry point if you're calling whisper-rs directly** (or any other
/// transcription engine that hands you `Vec<String>` segments). It runs every
/// hallucination guard with default settings and returns the cleaned segments plus
/// statistics about what was removed.
///
/// Idempotent: running it twice produces the same output.
///
/// # When to use this vs. [`clean_transcript`]
///
/// - Use [`clean_segments`] if you have raw segment text (the common case for
///   `whisper_state.get_segment(i).to_str()` callers).
/// - Use [`clean_transcript`] if you have a single string with timestamped lines
///   like `[0:00] hello world`.
///
/// # Example: cleaning whisper-rs output
///
/// ```
/// use whisper_guard::segments::clean_segments;
///
/// // Whisper hallucination pattern: same phrase repeated on silence
/// let raw = vec![
///     "Thank you.".to_string(),
///     "Thank you.".to_string(),
///     "Thank you.".to_string(),
///     "Thank you.".to_string(),
///     "What's the budget for this quarter?".to_string(),
/// ];
///
/// let (cleaned, stats) = clean_segments(&raw);
///
/// // Consecutive dedup keeps the first occurrence + an annotation line
/// // showing what was removed, so 4 repeats collapse to 2 segments.
/// assert!(stats.lines_removed >= 2);
/// assert!(cleaned.iter().any(|s| s.contains("budget")));
/// ```
pub fn clean_segments(segments: &[String]) -> (Vec<String>, CleanStats) {
    clean_segments_with_options(segments, &CleanOptions::default())
}

/// Clean a list of raw transcript segments with caller-controlled passes.
///
/// Like [`clean_segments`], but lets you disable specific passes if they cause
/// false positives in your pipeline. Pass order is fixed - see [`CleanOptions`]
/// for the rationale.
///
/// # Example: opt out of foreign-script filtering for multilingual transcripts
///
/// ```
/// use whisper_guard::segments::{clean_segments_with_options, CleanOptions};
///
/// let opts = CleanOptions {
///     strip_foreign_script: false,  // we expect mixed scripts
///     ..CleanOptions::default()
/// };
///
/// let segments = vec![
///     "Hello world".to_string(),
///     "你好世界".to_string(),  // would normally be filtered as foreign script
/// ];
/// let (cleaned, _stats) = clean_segments_with_options(&segments, &opts);
/// assert_eq!(cleaned.len(), 2);
/// ```
pub fn clean_segments_with_options(
    segments: &[String],
    opts: &CleanOptions,
) -> (Vec<String>, CleanStats) {
    let original_count = segments.len();
    let mut lines: Vec<String> = segments.to_vec();

    if opts.dedup_consecutive {
        lines = dedup_segments(&lines);
        if !opts.keep_dedup_annotations {
            lines.retain(|s| !s.starts_with(DEDUP_ANNOTATION_PREFIX));
        }
    }
    let after_consecutive = lines.len();

    if opts.dedup_interleaved {
        lines = dedup_interleaved(&lines);
    }
    let after_interleaved = lines.len();

    if opts.strip_foreign_script {
        lines = strip_foreign_script(&lines);
    }
    let after_script = lines.len();

    // Pipeline ordering rationale (matters for correctness):
    //
    //   1. dedup_consecutive (already ran above) - skips always-noise tokens
    //      so they flow downstream as a run for the noise-aware passes.
    //   2. dedup_interleaved (already ran above).
    //   3. strip_foreign_script (already ran above).
    //   4. strip_trailing_commands ← here. Runs BEFORE trim so that any
    //      always-noise markers hidden behind a trailing voice command
    //      (e.g. "…content [music] [music] Stop recording.") are exposed.
    //   5. trim_trailing_noise ← here. Catches all-noise tails at any count
    //      (always-noise tokens) and 5+ filler runs (`yeah.`, `okay.`, `you`).
    //   6. collapse_noise_markers ← runs LAST, so middle-of-transcript noise
    //      runs that survived trim get collapsed cleanly. If this ran earlier
    //      it would convert trailing `[music]` runs into `[music] + annotation`
    //      and trim would be blocked from cleaning them up.
    //
    // Each `after_X` stat reports the count after pass X ran, regardless of
    // chronological position in the pipeline. So `after_command_strip <=
    // after_trailing_trim` and `after_noise_markers >= after_trailing_trim`
    // are no longer guaranteed - the field name maps to a pass, not an
    // ordinal position.
    if opts.strip_trailing_commands {
        lines = strip_trailing_commands(&lines);
    }
    let after_command = lines.len();

    if opts.trim_trailing_noise {
        lines = trim_trailing_noise(&lines);
    }
    let after_trim = lines.len();

    if opts.collapse_noise_markers {
        lines = collapse_noise_markers(&lines);
    }
    let after_noise = lines.len();

    let stats = CleanStats {
        original_lines: original_count,
        after_consecutive_dedup: after_consecutive,
        after_interleaved_dedup: after_interleaved,
        after_script_filter: after_script,
        after_noise_markers: after_noise,
        after_trailing_trim: after_trim,
        after_command_strip: after_command,
        // Net change from input to final output. `collapse_noise_markers`
        // runs last (per the pipeline-order rationale above), so
        // `after_noise_markers` is the final segment count.
        lines_removed: original_count.saturating_sub(after_noise),
    };

    (lines, stats)
}

/// Clean an existing transcript by running all post-processing dedup layers.
///
/// **Use this if your transcript is already a single string with timestamped lines**
/// like `[0:00] some text`. For raw segments straight from whisper, prefer
/// [`clean_segments`] - it skips the unnecessary parse/format round-trip.
///
/// Idempotent: running it on already-cleaned text produces the same output.
///
/// # Example
///
/// ```
/// use whisper_guard::segments::clean_transcript;
///
/// let raw = "[0:00] Hello world\n[0:03] Hello world\n[0:06] Hello world\n[0:09] Real content";
/// let (cleaned, stats) = clean_transcript(raw);
/// assert!(stats.lines_removed > 0);
/// assert!(cleaned.contains("Real content"));
/// ```
pub fn clean_transcript(transcript: &str) -> (String, CleanStats) {
    let lines: Vec<String> = transcript.lines().map(|l| l.to_string()).collect();
    let (cleaned, stats) = clean_segments(&lines);
    (cleaned.join("\n"), stats)
}

/// Detect and remove repetition loops from whisper output.
///
/// Whisper's decoder can get stuck repeating the same text across consecutive segments,
/// especially on non-English audio. This function detects runs of 3+ consecutive segments
/// with >80% text overlap and collapses them to the first occurrence.
pub fn dedup_segments(lines: &[String]) -> Vec<String> {
    if lines.len() < 3 {
        return lines.to_vec();
    }

    // Simple text similarity: ratio of matching chars to total chars (normalized)
    fn similarity(a: &str, b: &str) -> f64 {
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        let a_lower = a.to_lowercase();
        let b_lower = b.to_lowercase();
        if a_lower == b_lower {
            return 1.0;
        }
        // Use longest common substring ratio as a fast similarity measure
        let (short, long) = if a_lower.len() <= b_lower.len() {
            (&a_lower, &b_lower)
        } else {
            (&b_lower, &a_lower)
        };
        if long.contains(short.as_str()) {
            return short.len() as f64 / long.len() as f64;
        }
        // Count matching words as fallback
        let a_words: Vec<&str> = a_lower.split_whitespace().collect();
        let b_words: Vec<&str> = b_lower.split_whitespace().collect();
        let matching = a_words.iter().filter(|w| b_words.contains(w)).count();
        let total = a_words.len().max(b_words.len());
        if total == 0 {
            return 0.0;
        }
        matching as f64 / total as f64
    }

    let mut result = Vec::with_capacity(lines.len());
    let mut i = 0;

    while i < lines.len() {
        let base_text = text_part(&lines[i]);

        // Always-noise tokens are NOT collapsed by dedup - they're handed to
        // collapse_noise_markers / trim_trailing_noise which know how to treat
        // them as a class. Collapsing here would prematurely turn a noise run
        // into "marker + annotation", which then can't be trimmed even if it's
        // entirely trailing.
        if is_always_noise(base_text) {
            result.push(lines[i].clone());
            i += 1;
            continue;
        }

        let mut run_end = i + 1;

        while run_end < lines.len() {
            let candidate = text_part(&lines[run_end]);
            if similarity(base_text, candidate) >= 0.8 {
                run_end += 1;
            } else {
                break;
            }
        }

        let run_len = run_end - i;

        if run_len >= 3 {
            tracing::debug!(
                first_segment = i,
                repeated_count = run_len,
                text = base_text,
                "detected repetition loop in whisper output - collapsing {} segments",
                run_len
            );
            result.push(lines[i].clone());
            result.push(format!(
                "{} - {} identical segments collapsed]",
                DEDUP_ANNOTATION_PREFIX,
                run_len - 1
            ));
            i = run_end;
        } else {
            result.push(lines[i].clone());
            i += 1;
        }
    }

    result
}

/// Detect interleaved repetition patterns that escape consecutive dedup.
///
/// Whisper often hallucinates alternating patterns like:
///   "So I'm going to pick his brain" / "Okay." / "So I'm going to pick his brain" / "Okay."
/// or inserts short filler between repeated phrases. The consecutive dedup misses these
/// because no two adjacent lines are similar.
///
/// Strategy: use a sliding window to detect when a single phrase dominates a region.
/// If any phrase appears in >=50% of lines within a 10-line window, and the window
/// contains at least 5 such occurrences, collapse the entire dominated region.
pub fn dedup_interleaved(lines: &[String]) -> Vec<String> {
    if lines.len() < 6 {
        return lines.to_vec();
    }

    fn normalize(text: &str) -> String {
        text.to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Short filler phrases that whisper inserts between hallucinated repetitions.
    fn is_filler(text: &str) -> bool {
        let normalized = text.trim().to_lowercase();
        let normalized = normalized.trim_matches(|c: char| !c.is_alphanumeric());
        matches!(
            normalized,
            "okay"
                | "ok"
                | "yeah"
                | "yes"
                | "right"
                | "so"
                | "and"
                | "but"
                | "well"
                | "uh"
                | "um"
                | "hmm"
                | "mhm"
        )
    }

    // Build normalized text for each line
    let texts: Vec<String> = lines.iter().map(|l| normalize(text_part(l))).collect();
    let fillers: Vec<bool> = texts.iter().map(|t| is_filler(t)).collect();

    // Mark lines that are part of a hallucination region.
    let mut remove = vec![false; lines.len()];

    let window_size = 10;
    let min_occurrences = 5;

    let mut i = 0;
    while i + window_size <= lines.len() {
        // Count phrase frequencies in this window (excluding fillers)
        let mut freq: std::collections::BTreeMap<&str, Vec<usize>> =
            std::collections::BTreeMap::new();
        for j in i..i + window_size {
            if !fillers[j] && !texts[j].is_empty() {
                freq.entry(&texts[j]).or_default().push(j);
            }
        }

        // Find the dominant phrase (BTreeMap for deterministic iteration order)
        let dominant = freq
            .iter()
            .max_by(|(phrase_a, pos_a), (phrase_b, pos_b)| {
                pos_a
                    .len()
                    .cmp(&pos_b.len())
                    .then_with(|| phrase_a.cmp(phrase_b))
            })
            .filter(|(_, positions)| positions.len() >= min_occurrences);

        if let Some((phrase, _)) = dominant {
            let phrase = phrase.to_string();
            // Extend the region: keep scanning forward while the phrase keeps appearing
            let mut region_end = i + window_size;
            while region_end < lines.len() {
                let t = &texts[region_end];
                if *t == phrase || fillers[region_end] {
                    region_end += 1;
                } else {
                    let mut gap = 0;
                    let mut found_resume = false;
                    for t in texts
                        .iter()
                        .take(lines.len().min(region_end + 3))
                        .skip(region_end)
                    {
                        if *t == phrase {
                            found_resume = true;
                            break;
                        }
                        gap += 1;
                    }
                    if found_resume && gap <= 2 {
                        region_end += gap + 1;
                    } else {
                        break;
                    }
                }
            }

            let region_len = region_end - i;
            let actual_count = (i..region_end).filter(|&j| texts[j] == phrase).count();

            if actual_count >= min_occurrences && region_len >= 6 {
                tracing::debug!(
                    region_start = i,
                    region_end = region_end,
                    occurrences = actual_count,
                    filler_count = (i..region_end).filter(|&j| fillers[j]).count(),
                    phrase = phrase,
                    "detected interleaved hallucination loop - marking {} lines for removal",
                    region_len
                );
                let mut kept_first = false;
                for j in i..region_end {
                    if !kept_first && texts[j] == phrase {
                        kept_first = true;
                    } else {
                        remove[j] = true;
                    }
                }
                i = region_end;
                continue;
            }
        }

        i += 1;
    }

    let removed_count = remove.iter().filter(|&&r| r).count();
    if removed_count > 0 {
        let mut result = Vec::with_capacity(lines.len() - removed_count + 1);
        let mut in_removed_run = false;

        for (idx, line) in lines.iter().enumerate() {
            if remove[idx] {
                if !in_removed_run {
                    in_removed_run = true;
                    let run_len = (idx..lines.len()).take_while(|&j| remove[j]).count();
                    result.push(format!(
                        "[...] [hallucinated repetition removed - {} lines collapsed]",
                        run_len
                    ));
                }
            } else {
                in_removed_run = false;
                result.push(line.clone());
            }
        }

        tracing::info!(
            original = lines.len(),
            removed = removed_count,
            remaining = result.len(),
            "interleaved dedup complete"
        );
        result
    } else {
        lines.to_vec()
    }
}

/// Collapse runs of bracketed non-speech markers in any language.
///
/// Whisper emits non-speech audio events as bracketed text: `[music]`, `[laughter]`,
/// `[applause]`, `[BLANK_AUDIO]`, etc. In non-English audio these appear in the
/// source language: `[Śmiech]` (Polish laughter), `[Musik]` (German music),
/// `[risas]` (Spanish laughter), etc.
///
/// The existing `trim_trailing_noise` only catches trailing English markers. This
/// function is language-agnostic - it detects any line whose text (after timestamp)
/// is a short bracketed expression `[word(s)]` and collapses consecutive runs of 3+.
/// It also collapses scattered patterns when >50% of a window are noise markers.
pub fn collapse_noise_markers(lines: &[String]) -> Vec<String> {
    if lines.len() < 3 {
        return lines.to_vec();
    }

    /// Return true if the text (after timestamp) is a bracketed non-speech marker.
    ///
    /// Matches patterns like `[music]`, `[Śmiech]`, `[BLANK_AUDIO]`, `[risas]`.
    /// Excludes timestamp-like content `[0:00]` and collapse markers from prior
    /// dedup passes `[...] [repeated ...]`.
    fn is_noise_marker(text: &str) -> bool {
        let t = text.trim();
        if t.is_empty() {
            return false;
        }
        // Collapse markers from prior passes are not noise
        if t.starts_with("[...]") {
            return false;
        }
        // Must start with '[' and end with ']' (optionally with trailing '.')
        let t = t.strip_suffix('.').unwrap_or(t);
        if !(t.starts_with('[') && t.ends_with(']')) {
            return false;
        }
        let inner = &t[1..t.len() - 1];
        // Reject timestamp-like patterns (digits and colons only)
        if inner.chars().all(|c| c.is_ascii_digit() || c == ':') {
            return false;
        }
        // Must be short (1-4 words, ≤40 chars) - non-speech markers are brief
        let word_count = inner.split_whitespace().count();
        (1..=4).contains(&word_count) && inner.len() <= 40
    }

    let markers: Vec<bool> = lines
        .iter()
        .map(|l| is_noise_marker(text_part(l)))
        .collect();

    // Pass 1: Collapse consecutive runs of 3+ noise markers
    let mut result = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if markers[i] {
            let run_start = i;
            while i < lines.len() && markers[i] {
                i += 1;
            }
            let run_len = i - run_start;
            if run_len >= 3 {
                result.push(lines[run_start].clone());
                result.push(format!(
                    "[...] [non-speech audio removed - {} markers collapsed]",
                    run_len - 1
                ));
                tracing::debug!(
                    run_start = run_start,
                    collapsed = run_len - 1,
                    sample = text_part(&lines[run_start]),
                    "collapsed consecutive noise markers"
                );
            } else {
                // Short run (1-2): keep as-is
                for line in lines.iter().take(i).skip(run_start) {
                    result.push(line.clone());
                }
            }
        } else {
            result.push(lines[i].clone());
            i += 1;
        }
    }

    // Pass 2: Ratio check - if ≥2/3 of remaining lines are noise markers, strip them all.
    // After pass 1 collapses consecutive runs, scattered markers that still dominate
    // the transcript are almost certainly hallucination. Real recordings rarely have
    // this density (e.g., a comedy show might have 30-40% [laughter] annotations, not 66%+).
    let remaining_markers = result
        .iter()
        .filter(|l| is_noise_marker(text_part(l)))
        .count();
    let content_lines = result.len().saturating_sub(remaining_markers);
    if remaining_markers > 0 && content_lines > 0 {
        let ratio = remaining_markers as f64 / result.len() as f64;
        if ratio >= 0.66 && remaining_markers >= 8 {
            tracing::info!(
                markers = remaining_markers,
                total = result.len(),
                ratio = format!("{:.0}%", ratio * 100.0),
                "high noise marker density - stripping scattered markers"
            );
            let mut stripped = Vec::with_capacity(content_lines + 1);
            let mut removed = 0usize;
            for line in &result {
                if is_noise_marker(text_part(line)) {
                    removed += 1;
                } else {
                    stripped.push(line.clone());
                }
            }
            stripped.push(format!(
                "[{} scattered non-speech markers removed]",
                removed
            ));
            return stripped;
        }
    }

    let removed = lines.len() - result.len();
    if removed > 0 {
        tracing::info!(
            original = lines.len(),
            removed = removed,
            "collapsed noise markers"
        );
    }

    result
}

/// Detect and remove lines with hallucinated foreign script.
///
/// When whisper processes silence or very low-signal audio, it often hallucinates
/// text in scripts unrelated to the actual audio - most commonly CJK characters
/// (Japanese/Chinese/Korean), Arabic, or Cyrillic in an otherwise Latin transcript.
///
/// This function determines the dominant script of the transcript and removes lines
/// that are primarily in a different script. It is conservative: it only acts when
/// there is a clear majority script (≥70% of lines) and only removes lines where
/// ≥50% of alphabetic characters are in a foreign script.
///
/// This is language-agnostic: a Japanese transcript with a few hallucinated Latin
/// lines would have the Latin lines removed, and vice versa. Also handles
/// Cyrillic, Arabic, and other scripts via the `Script::Other` bucket.
pub fn strip_foreign_script(lines: &[String]) -> Vec<String> {
    if lines.len() < 2 {
        return lines.to_vec();
    }

    // Classify each line's dominant script
    let classifications: Vec<Script> = lines
        .iter()
        .map(|l| classify_script(text_part(l)))
        .collect();

    // Count lines per script (ignoring Unknown/empty)
    let mut latin_count = 0usize;
    let mut cjk_count = 0usize;
    let mut other_count = 0usize;
    for s in &classifications {
        match s {
            Script::Latin => latin_count += 1,
            Script::Cjk => cjk_count += 1,
            Script::Other => other_count += 1,
            Script::Unknown => {}
        }
    }

    let meaningful = latin_count + cjk_count + other_count;
    if meaningful < 2 {
        return lines.to_vec();
    }

    // Determine majority script (must be ≥70% of meaningful lines)
    let majority = if latin_count as f64 / meaningful as f64 >= 0.7 {
        Script::Latin
    } else if cjk_count as f64 / meaningful as f64 >= 0.7 {
        Script::Cjk
    } else if other_count as f64 / meaningful as f64 >= 0.7 {
        Script::Other
    } else {
        return lines.to_vec(); // No clear majority - don't filter
    };

    let mut result = Vec::with_capacity(lines.len());
    let mut removed = 0usize;

    for (i, line) in lines.iter().enumerate() {
        let dominated_by_foreign = match (&classifications[i], &majority) {
            (Script::Unknown, _) => false, // Keep empty/punctuation-only lines
            (s, m) if s == m => false,     // Same script as majority
            _ => true,                     // Foreign script
        };

        if dominated_by_foreign {
            removed += 1;
        } else {
            result.push(line.clone());
        }
    }

    if removed > 0 {
        tracing::info!(
            removed = removed,
            majority = ?majority,
            "removed foreign-script hallucination lines"
        );
    }

    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Script {
    Latin,
    Cjk,
    Other,
    Unknown,
}

/// Classify the dominant script of a text string.
/// Returns the script that comprises the majority of alphabetic characters.
fn classify_script(text: &str) -> Script {
    let mut latin = 0u32;
    let mut cjk = 0u32;
    let mut other_script = 0u32;

    for ch in text.chars() {
        if !ch.is_alphabetic() {
            continue;
        }
        if ch.is_ascii_alphabetic()
            || ('\u{00C0}'..='\u{024F}').contains(&ch) // Latin Extended
            || ('\u{1E00}'..='\u{1EFF}').contains(&ch)
        {
            latin += 1;
        } else if ('\u{4E00}'..='\u{9FFF}').contains(&ch)   // CJK Unified
            || ('\u{3400}'..='\u{4DBF}').contains(&ch)       // CJK Extension A
            || ('\u{3040}'..='\u{309F}').contains(&ch)       // Hiragana
            || ('\u{30A0}'..='\u{30FF}').contains(&ch)       // Katakana
            || ('\u{AC00}'..='\u{D7AF}').contains(&ch)
        // Hangul
        {
            cjk += 1;
        } else {
            other_script += 1;
        }
    }

    let total = latin + cjk + other_script;
    if total == 0 {
        return Script::Unknown;
    }

    if latin as f64 / total as f64 >= 0.5 {
        Script::Latin
    } else if cjk as f64 / total as f64 >= 0.5 {
        Script::Cjk
    } else {
        Script::Other
    }
}

/// Trim trailing non-speech noise from the end of a transcript.
///
/// Recordings often capture music, silence, or ambient noise after the conversation
/// ends. Long runs of `[music]`, `[BLANK_AUDIO]`, or very short filler at the end
/// add no value and make the transcript look broken.
pub fn trim_trailing_noise(lines: &[String]) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }

    /// Filler tokens that COULD be legitimate one-word closings ("Thanks.",
    /// "Yeah.", "Okay."). These need a higher floor to avoid trimming real
    /// terse content; only trim when there's a 5+ run of them.
    fn is_filler(text: &str) -> bool {
        let t = text.trim().to_lowercase();
        t == "you" || t == "okay." || t == "yeah."
        // Note: collapse markers ("[...] [repeated ...]") are NOT noise -
        // treating them as noise would make clean_transcript non-idempotent.
    }

    // Walk backward from the end, counting trailing noise/filler lines and
    // tracking how many were always-noise vs filler.
    let mut trim_from = lines.len();
    let mut always_noise_count = 0usize;
    for i in (0..lines.len()).rev() {
        let text = text_part(&lines[i]);
        if is_always_noise(text) {
            trim_from = i;
            always_noise_count += 1;
        } else if is_filler(text) {
            trim_from = i;
        } else {
            break;
        }
    }

    let trimmed_count = lines.len() - trim_from;

    // Trim if EITHER:
    //   - any always-noise marker is in the trailing block (those are never
    //     legitimate transcript content, regardless of count), OR
    //   - the trailing filler block is 5+ lines (protects "Thanks." closings)
    let should_trim = always_noise_count > 0 || trimmed_count >= 5;

    if should_trim {
        tracing::info!(
            trimmed = trimmed_count,
            always_noise = always_noise_count,
            "removed trailing noise from transcript"
        );
        let mut result: Vec<String> = lines[..trim_from].to_vec();
        result.push(format!(
            "[Recording ended - {} lines of trailing noise removed]",
            trimmed_count
        ));
        result
    } else {
        lines.to_vec()
    }
}

/// Strip trailing voice command phrases that get captured by the mic.
///
/// Users commonly say "stop recording" or "end recording" out loud to signal
/// they're done. The microphone captures these phrases and Whisper transcribes
/// them as part of the meeting. This function removes them from the last 1-2
/// lines of the transcript.
pub fn strip_trailing_commands(lines: &[String]) -> Vec<String> {
    const COMMANDS: &[&str] = &[
        "stop recording",
        "stop the recording",
        "end recording",
        "end the recording",
        "stop transcription",
        "end transcription",
        "stop transcribing",
        "hey minutes stop",
        "minutes stop",
        "okay stop",
        "ok stop",
    ];

    let mut result = lines.to_vec();
    // Check last 2 lines - the command might be split across whisper segments
    for _ in 0..2 {
        if let Some(last) = result.last() {
            let text = text_part(last).trim().to_lowercase();
            let text = text.trim_end_matches('.');
            if COMMANDS
                .iter()
                .any(|cmd| text == *cmd || text.ends_with(cmd))
            {
                tracing::debug!(
                    line = result.last().map(|l| l.as_str()).unwrap_or(""),
                    "stripping trailing voice command"
                );
                result.pop();
            } else {
                break;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── clean_transcript end-to-end ──

    #[test]
    fn clean_transcript_removes_repetition() {
        let input = "[0:00] Hello world\n[0:03] Hello world\n[0:06] Hello world\n[0:09] Hello world\n[0:12] Something different\n";
        let (cleaned, stats) = clean_transcript(input);
        assert!(stats.lines_removed > 0);
        assert!(cleaned.contains("Something different"));
        assert!(cleaned.contains("repeated audio removed"));
    }

    #[test]
    fn clean_transcript_preserves_normal_text() {
        let input = "[0:00] First line\n[0:05] Second line\n[0:10] Third line\n";
        let (cleaned, stats) = clean_transcript(input);
        assert_eq!(stats.lines_removed, 0);
        assert!(cleaned.contains("First line"));
        assert!(cleaned.contains("Third line"));
    }

    // ── dedup_segments ──

    #[test]
    fn dedup_no_repetition() {
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:03] How are you".into(),
            "[0:06] Fine thanks".into(),
        ];
        let result = dedup_segments(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn dedup_collapses_exact_repetition() {
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:03] Hello world".into(),
            "[0:06] Hello world".into(),
            "[0:09] Hello world".into(),
            "[0:12] Something different".into(),
        ];
        let result = dedup_segments(&lines);
        assert_eq!(result.len(), 3);
        assert!(result[0].contains("Hello world"));
        assert!(result[1].contains("repeated audio removed"));
        assert!(result[2].contains("Something different"));
    }

    #[test]
    fn dedup_collapses_near_identical() {
        let lines = vec![
            "[0:00] Ok bene le macedi diesel".into(),
            "[0:03] Ok, bene le macedi diesel".into(),
            "[0:06] Ok bene, le macedi diesel".into(),
            "[0:09] Good morning".into(),
        ];
        let result = dedup_segments(&lines);
        assert_eq!(result.len(), 3);
        assert!(result[1].contains("repeated audio removed"));
    }

    #[test]
    fn dedup_leaves_two_similar_alone() {
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:03] Hello world".into(),
            "[0:06] Something else".into(),
        ];
        let result = dedup_segments(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn dedup_handles_empty() {
        let result = dedup_segments(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn dedup_handles_single_line() {
        let lines = vec!["[0:00] Hello".into()];
        let result = dedup_segments(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn dedup_multiple_runs() {
        let lines = vec![
            "[0:00] First phrase".into(),
            "[0:03] First phrase".into(),
            "[0:06] First phrase".into(),
            "[0:09] Second phrase".into(),
            "[0:12] Second phrase".into(),
            "[0:15] Second phrase".into(),
            "[0:18] Second phrase".into(),
            "[0:21] Normal text".into(),
        ];
        let result = dedup_segments(&lines);
        assert_eq!(result.len(), 5);
        assert!(result[1].contains("2 identical"));
        assert!(result[3].contains("3 identical"));
    }

    // ── interleaved dedup ──

    #[test]
    fn interleaved_catches_alternating_pattern() {
        let mut lines: Vec<String> = Vec::new();
        for i in 0..20 {
            let ts = i * 2;
            if i % 2 == 0 {
                lines.push(format!(
                    "[{}:{:02}] So I'm going to pick his brain as well.",
                    ts / 60,
                    ts % 60
                ));
            } else {
                lines.push(format!("[{}:{:02}] Okay.", ts / 60, ts % 60));
            }
        }
        lines.push("[0:40] Something completely different".into());

        let result = dedup_interleaved(&lines);
        assert!(
            result.len() <= 4,
            "expected <=4 lines, got {}: {:?}",
            result.len(),
            result
        );
        assert!(result.iter().any(|l| l.contains("pick his brain")));
        assert!(result
            .iter()
            .any(|l| l.contains("hallucinated repetition removed")));
        assert!(result
            .last()
            .unwrap()
            .contains("Something completely different"));
    }

    #[test]
    fn interleaved_leaves_normal_conversation() {
        let lines = vec![
            "[0:00] Hello how are you".into(),
            "[0:05] I'm fine thanks".into(),
            "[0:10] Great to hear".into(),
            "[0:15] Let's talk about the project".into(),
            "[0:20] Sure what's the update".into(),
            "[0:25] We shipped the feature".into(),
        ];
        let result = dedup_interleaved(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn interleaved_ignores_short_repeats() {
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:02] Okay.".into(),
            "[0:04] Hello world".into(),
            "[0:06] Okay.".into(),
            "[0:08] Hello world".into(),
            "[0:10] Something else".into(),
        ];
        let result = dedup_interleaved(&lines);
        assert_eq!(result, lines);
    }

    // ── trailing noise ──

    #[test]
    fn trim_trailing_music() {
        let mut lines: Vec<String> = vec![
            "[0:00] Hello world".into(),
            "[0:05] Some real content".into(),
        ];
        for i in 0..20 {
            lines.push(format!("[{}:00] [music]", i + 1));
        }
        let result = trim_trailing_noise(&lines);
        assert_eq!(result.len(), 3);
        assert!(result[0].contains("Hello world"));
        assert!(result[1].contains("real content"));
        assert!(result[2].contains("trailing noise removed"));
    }

    #[test]
    fn trim_short_run_of_always_noise_now_trimmed() {
        // 0.2.0 behavior change: always-noise tokens (`[music]`, `[blank_audio]`,
        // `[silence]`, `music`) are NEVER legitimate transcript content, so they
        // get trimmed at any count. The 5-line floor still protects filler words
        // (`you`, `okay.`, `yeah.`) that COULD be legitimate one-word closings -
        // see `trim_keeps_short_trailing_filler` below.
        let lines: Vec<String> = vec![
            "[0:00] Hello world".into(),
            "[0:05] [music]".into(),
            "[0:10] [music]".into(),
            "[0:15] [music]".into(),
        ];
        let result = trim_trailing_noise(&lines);
        assert_eq!(result.len(), 2);
        assert!(result[0].contains("Hello world"));
        assert!(result[1].contains("trailing noise removed"));
    }

    #[test]
    fn trim_keeps_short_trailing_filler() {
        // Filler words at the end MUST survive - common legitimate closing.
        let lines: Vec<String> = vec!["[0:00] That wraps it".into(), "[0:05] yeah.".into()];
        let result = trim_trailing_noise(&lines);
        assert_eq!(result, lines, "single-filler closing must survive");
    }

    #[test]
    fn trim_long_run_of_filler_is_trimmed() {
        // 5+ filler in a row is suspicious enough to trim.
        let lines: Vec<String> = vec![
            "[0:00] Real content".into(),
            "[0:05] yeah.".into(),
            "[0:10] yeah.".into(),
            "[0:15] yeah.".into(),
            "[0:20] yeah.".into(),
            "[0:25] yeah.".into(),
        ];
        let result = trim_trailing_noise(&lines);
        assert_eq!(result.len(), 2);
        assert!(result[0].contains("Real content"));
        assert!(result[1].contains("trailing noise removed"));
    }

    #[test]
    fn trim_handles_empty() {
        assert!(trim_trailing_noise(&[]).is_empty());
    }

    #[test]
    fn trim_all_noise() {
        let lines: Vec<String> = (0..10).map(|i| format!("[{}:00] [music]", i)).collect();
        let result = trim_trailing_noise(&lines);
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("trailing noise removed"));
    }

    // ── foreign script detection ──

    #[test]
    fn script_removes_cjk_from_latin_transcript() {
        let lines = vec![
            "[0:00] Hello and welcome".into(),
            "[0:05] Let's discuss the project".into(),
            "[0:10] スパイシー".into(),
            "[0:15] We should wrap up now".into(),
        ];
        let result = strip_foreign_script(&lines);
        assert_eq!(result.len(), 3);
        assert!(!result.iter().any(|l| l.contains("スパイシー")));
    }

    #[test]
    fn script_preserves_pure_latin_transcript() {
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:05] How are you".into(),
            "[0:10] I'm doing fine".into(),
        ];
        let result = strip_foreign_script(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn script_preserves_pure_cjk_transcript() {
        let lines = vec![
            "[0:00] こんにちは".into(),
            "[0:05] お元気ですか".into(),
            "[0:10] 元気です".into(),
        ];
        let result = strip_foreign_script(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn script_no_action_on_mixed_transcript() {
        // No clear majority (50/50 split) - don't filter anything
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:05] こんにちは".into(),
            "[0:10] Good morning".into(),
            "[0:15] お元気ですか".into(),
        ];
        let result = strip_foreign_script(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn script_handles_single_line() {
        let lines = vec!["[0:00] スパイシー".into()];
        let result = strip_foreign_script(&lines);
        assert_eq!(result, lines); // Single line - no majority to compare against
    }

    #[test]
    fn script_all_hallucinated_in_latin_majority() {
        // Mostly Latin with a couple CJK hallucination lines (>70% Latin)
        let lines = vec![
            "[0:00] Today we need to discuss".into(),
            "[0:05] The quarterly results".into(),
            "[0:10] Are looking good".into(),
            "[0:15] Revenue is up".into(),
            "[0:20] Margins improved significantly".into(),
            "[0:25] 東京タワー".into(),
            "[0:30] 大阪城".into(),
        ];
        let result = strip_foreign_script(&lines);
        assert_eq!(result.len(), 5);
        assert!(result
            .iter()
            .all(|l| !l.contains('東') && !l.contains('大')));
    }

    #[test]
    fn script_two_cjk_lines_preserved() {
        // Exactly 2 CJK lines: majority is CJK, so both are kept (not hallucination).
        let lines = vec!["[0:00] スパイシー".into(), "[0:05] 東京タワー".into()];
        let result = strip_foreign_script(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn script_cyrillic_majority_strips_latin() {
        // Cyrillic majority with a Latin hallucination line.
        let lines = vec![
            "[0:00] Привет мир".into(),
            "[0:05] Как дела".into(),
            "[0:10] Всё хорошо".into(),
            "[0:15] Hello world".into(), // Hallucinated Latin
        ];
        let result = strip_foreign_script(&lines);
        assert_eq!(result.len(), 3);
        assert!(!result.iter().any(|l| l.contains("Hello")));
    }

    #[test]
    fn script_classify_basic() {
        assert_eq!(classify_script("Hello world"), Script::Latin);
        assert_eq!(classify_script("スパイシー"), Script::Cjk);
        assert_eq!(classify_script("Привет"), Script::Other);
        assert_eq!(classify_script(""), Script::Unknown);
        assert_eq!(classify_script("123 !@#"), Script::Unknown);
    }

    #[test]
    fn clean_transcript_includes_script_filter() {
        let input =
            "[0:00] Hello world\n[0:05] Testing one two\n[0:10] スパイシー\n[0:15] All done\n";
        let (cleaned, stats) = clean_transcript(input);
        assert!(!cleaned.contains("スパイシー"));
        assert!(stats.after_script_filter < stats.after_interleaved_dedup);
    }

    // ── noise marker collapse ──

    #[test]
    fn noise_markers_collapses_polish_laughter() {
        // Polish whisper hallucination: [Śmiech] = laughter
        let mut lines: Vec<String> = vec!["[0:00] Cześć, jak się masz?".into()];
        for i in 1..=10 {
            lines.push(format!("[0:{:02}] [Śmiech]", i * 3));
        }
        lines.push("[0:33] Dobrze, dziękuję".into());

        let result = collapse_noise_markers(&lines);
        assert!(
            result.len() <= 4,
            "got {} lines: {:?}",
            result.len(),
            result
        );
        assert!(result[0].contains("Cześć"));
        assert!(result
            .iter()
            .any(|l| l.contains("non-speech audio removed")));
        assert!(result.last().unwrap().contains("Dobrze"));
    }

    #[test]
    fn noise_markers_collapses_english_mixed() {
        let lines = vec![
            "[0:00] Good morning everyone".into(),
            "[0:05] [music]".into(),
            "[0:10] [laughter]".into(),
            "[0:15] [applause]".into(),
            "[0:20] [music]".into(),
            "[0:25] Thank you for coming".into(),
        ];
        let result = collapse_noise_markers(&lines);
        assert!(
            result.len() <= 4,
            "got {} lines: {:?}",
            result.len(),
            result
        );
        assert!(result[0].contains("Good morning"));
        assert!(result.last().unwrap().contains("Thank you"));
    }

    #[test]
    fn noise_markers_preserves_short_runs() {
        // 1-2 markers should be kept (legitimate non-speech annotations)
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:05] [laughter]".into(),
            "[0:10] That was funny".into(),
        ];
        let result = collapse_noise_markers(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn noise_markers_handles_empty() {
        assert!(collapse_noise_markers(&[]).is_empty());
    }

    #[test]
    fn noise_markers_handles_single_line() {
        let lines = vec!["[0:00] [music]".into()];
        let result = collapse_noise_markers(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn noise_markers_handles_two_lines() {
        let lines = vec!["[0:00] [music]".into(), "[0:03] [laughter]".into()];
        let result = collapse_noise_markers(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn noise_markers_ignores_timestamps() {
        // Timestamps like [0:00] are NOT noise markers
        let lines = vec![
            "[0:00] Hello".into(),
            "[0:05] World".into(),
            "[0:10] Test".into(),
        ];
        let result = collapse_noise_markers(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn noise_markers_ignores_collapse_markers() {
        // Prior dedup pass markers should not be treated as noise
        let lines = vec![
            "[0:00] Hello world".into(),
            "[...] [repeated audio removed - 5 identical segments collapsed]".into(),
            "[0:30] Something else".into(),
            "[...] [hallucinated repetition removed - 10 lines collapsed]".into(),
            "[1:00] Final line".into(),
        ];
        let result = collapse_noise_markers(&lines);
        assert_eq!(result, lines);
    }

    #[test]
    fn noise_markers_multilingual_markers() {
        // Various languages' non-speech markers
        let mut lines = Vec::new();
        lines.push("[0:00] Bonjour".into());
        // French: [rires] = laughter, [musique] = music
        for i in 1..=4 {
            lines.push(format!("[0:{:02}] [rires]", i * 3));
        }
        // German: [Musik], [Gelächter]
        for i in 5..=7 {
            lines.push(format!("[0:{:02}] [Musik]", i * 3));
        }
        lines.push("[0:30] Au revoir".into());

        let result = collapse_noise_markers(&lines);
        assert!(
            result.len() <= 5,
            "got {} lines: {:?}",
            result.len(),
            result
        );
        assert!(result[0].contains("Bonjour"));
        assert!(result.last().unwrap().contains("Au revoir"));
    }

    #[test]
    fn noise_markers_scattered_high_density() {
        // Pass 2 fires at ≥66% ratio with ≥8 remaining markers after pass 1.
        // Use pairs of markers (runs of 2, below pass 1's threshold of 3)
        // interleaved with single content lines: 5 content + 10 markers = 66.7%.
        let lines = vec![
            "[0:00] Real content one".into(),
            "[0:03] [Śmiech]".into(),
            "[0:06] [muzyka]".into(),
            "[0:09] Real content two".into(),
            "[0:12] [cisza]".into(),
            "[0:15] [oklaski]".into(),
            "[0:18] Real content three".into(),
            "[0:21] [Śmiech]".into(),
            "[0:24] [muzyka]".into(),
            "[0:27] Real content four".into(),
            "[0:30] [cisza]".into(),
            "[0:33] [oklaski]".into(),
            "[0:36] Real content five".into(),
            "[0:39] [Śmiech]".into(),
            "[0:42] [muzyka]".into(),
        ];
        let result = collapse_noise_markers(&lines);
        // All 5 content lines should survive
        let content_count = result.iter().filter(|l| l.contains("Real content")).count();
        assert_eq!(content_count, 5, "all content lines preserved");
        // Pass 2 should have stripped the scattered markers
        assert!(
            result
                .iter()
                .any(|l| l.contains("non-speech markers removed")),
            "expected pass 2 removal summary, got: {:?}",
            result
        );
    }

    #[test]
    fn noise_markers_below_threshold_kept() {
        // 50% markers (5 of 10) - below the 66% threshold, all kept
        let lines = vec![
            "[0:00] Real content one".into(),
            "[0:03] [laughter]".into(),
            "[0:06] Real content two".into(),
            "[0:09] [applause]".into(),
            "[0:12] Real content three".into(),
            "[0:15] [laughter]".into(),
            "[0:18] Real content four".into(),
            "[0:21] [music]".into(),
            "[0:24] Real content five".into(),
            "[0:27] [laughter]".into(),
        ];
        let result = collapse_noise_markers(&lines);
        // No markers stripped - density is too low for pass 2
        assert_eq!(result, lines);
    }

    #[test]
    fn noise_markers_handles_blank_audio() {
        let mut lines: Vec<String> = vec!["[0:00] Some content".into()];
        for i in 1..=6 {
            lines.push(format!("[0:{:02}] [BLANK_AUDIO]", i * 5));
        }
        lines.push("[0:35] More content".into());

        let result = collapse_noise_markers(&lines);
        assert!(result.len() <= 4);
        assert!(result
            .iter()
            .any(|l| l.contains("non-speech audio removed")));
    }

    #[test]
    fn clean_transcript_includes_noise_markers() {
        // Use varied markers so consecutive dedup doesn't catch them first.
        // This ensures the noise marker layer has work to do.
        let input = "[0:00] Hello world\n\
            [0:03] [Śmiech]\n\
            [0:06] [muzyka]\n\
            [0:09] [cisza]\n\
            [0:12] [oklaski]\n\
            [0:15] [Śmiech]\n\
            [0:18] [muzyka]\n\
            [0:21] [cisza]\n\
            [0:24] Goodbye\n";

        let (cleaned, stats) = clean_transcript(input);
        // Noise marker filter runs after script filter; should have removed some lines
        assert!(
            stats.after_noise_markers < stats.after_script_filter,
            "noise markers: {}, script filter: {}",
            stats.after_noise_markers,
            stats.after_script_filter
        );
        assert!(cleaned.contains("Hello world"));
        assert!(cleaned.contains("Goodbye"));
    }

    // ── strip_trailing_commands ──

    #[test]
    fn strip_command_removes_stop_recording() {
        let lines = vec![
            "[0:00] Great meeting everyone".into(),
            "[0:05] Let's wrap up".into(),
            "[0:10] Stop recording.".into(),
        ];
        let result = strip_trailing_commands(&lines);
        assert_eq!(result.len(), 2);
        assert!(result[1].contains("wrap up"));
    }

    #[test]
    fn strip_command_removes_with_timestamp() {
        let lines = vec!["[0:00] First point".into(), "[0:30] Stop recording".into()];
        let result = strip_trailing_commands(&lines);
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("First point"));
    }

    #[test]
    fn strip_command_removes_end_recording() {
        let lines = vec![
            "[0:00] Discussion content".into(),
            "[0:10] End recording".into(),
        ];
        let result = strip_trailing_commands(&lines);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn strip_command_removes_two_trailing_commands() {
        let lines = vec![
            "[0:00] Content".into(),
            "[0:10] Okay stop.".into(),
            "[0:12] Stop recording.".into(),
        ];
        let result = strip_trailing_commands(&lines);
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("Content"));
    }

    #[test]
    fn strip_command_preserves_non_command_lines() {
        let lines = vec![
            "[0:00] We need to stop recording expenses".into(),
            "[0:05] The stop recording policy is important".into(),
        ];
        let result = strip_trailing_commands(&lines);
        assert_eq!(result.len(), 2, "non-command lines should be preserved");
    }

    #[test]
    fn strip_command_handles_empty() {
        let result = strip_trailing_commands(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn strip_command_case_insensitive() {
        let lines = vec![
            "[0:00] Meeting notes".into(),
            "[0:05] STOP RECORDING".into(),
        ];
        let result = strip_trailing_commands(&lines);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn clean_transcript_strips_trailing_command() {
        let input =
            "[0:00] Important discussion\n[0:05] Action item for Bob\n[0:10] Stop recording.\n";
        let (cleaned, stats) = clean_transcript(input);
        assert!(!cleaned.contains("Stop recording"));
        assert!(cleaned.contains("Action item for Bob"));
        // Command-strip runs before trim now, so after_command_strip is the
        // count BEFORE the (no-op) trim runs. Trim is a no-op here, so the two
        // counts match.
        assert!(stats.after_command_strip <= stats.after_trailing_trim);
        assert_eq!(stats.lines_removed, 1);
    }

    // ---- clean_segments + CleanOptions ----

    #[test]
    fn clean_segments_handles_empty() {
        let (cleaned, stats) = clean_segments(&[]);
        assert!(cleaned.is_empty());
        assert_eq!(stats.original_lines, 0);
        assert_eq!(stats.lines_removed, 0);
    }

    #[test]
    fn clean_segments_passes_through_clean_input() {
        let input: Vec<String> = vec![
            "Welcome to the meeting.".into(),
            "Let's discuss Q3 numbers.".into(),
            "Revenue is up twelve percent.".into(),
        ];
        let (cleaned, stats) = clean_segments(&input);
        assert_eq!(cleaned, input, "clean input should be untouched");
        assert_eq!(stats.lines_removed, 0);
        assert_eq!(stats.after_command_strip, 3);
    }

    #[test]
    fn clean_segments_dedups_repeated_hallucination() {
        let input: Vec<String> = vec![
            "Thank you.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "What's the budget for Q3?".into(),
        ];
        let (cleaned, stats) = clean_segments(&input);
        // Real content survives; the hallucination loop collapses to
        // first occurrence + annotation line.
        assert!(cleaned.iter().any(|s| s.contains("budget")));
        assert!(stats.lines_removed >= 2);
        // Annotation line is inserted to mark what was collapsed.
        assert!(cleaned.iter().any(|s| s.contains("repeated audio removed")));
    }

    #[test]
    fn clean_segments_is_idempotent() {
        let input: Vec<String> = vec![
            "Real content.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "More real content.".into(),
        ];
        let (first, _) = clean_segments(&input);
        let (second, second_stats) = clean_segments(&first);
        assert_eq!(first, second, "second pass should be a no-op");
        assert_eq!(second_stats.lines_removed, 0);
    }

    #[test]
    fn clean_segments_with_options_respects_disabled_passes() {
        let input: Vec<String> = vec![
            "Hello.".into(),
            "Hello.".into(),
            "Hello.".into(),
            "Hello.".into(),
        ];
        // Disable consecutive dedup; everything else still runs.
        let opts = CleanOptions {
            dedup_consecutive: false,
            ..CleanOptions::default()
        };
        let (cleaned, _) = clean_segments_with_options(&input, &opts);
        assert_eq!(cleaned.len(), input.len(), "dedup disabled → no removal");
    }

    #[test]
    fn clean_options_none_runs_no_passes() {
        let input: Vec<String> = vec![
            "Thank you.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "Stop recording.".into(),
        ];
        let (cleaned, stats) = clean_segments_with_options(&input, &CleanOptions::none());
        assert_eq!(cleaned, input, "no passes → no changes");
        assert_eq!(stats.lines_removed, 0);
    }

    #[test]
    fn clean_options_all_matches_default() {
        // Same default config exercised two ways must produce the same output.
        let input: Vec<String> = vec![
            "Real meeting content.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "[music]".into(),
        ];
        let (default_out, default_stats) = clean_segments(&input);
        let (all_out, all_stats) = clean_segments_with_options(&input, &CleanOptions::all());
        assert_eq!(default_out, all_out);
        assert_eq!(default_stats, all_stats);
    }

    #[test]
    fn clean_segments_works_on_raw_segments_without_timestamps() {
        // The fork-user case: raw segments straight from whisper_state.get_segment(i).
        // No timestamp brackets. Cleaning should still work end-to-end.
        let raw_segments: Vec<String> = vec![
            " Thank you.".into(), // whisper segments often have leading space
            " Thank you.".into(),
            " Thank you.".into(),
            " Thank you.".into(),
            " So what's our action plan?".into(),
        ];
        let (cleaned, stats) = clean_segments(&raw_segments);
        assert!(stats.lines_removed >= 2);
        assert!(cleaned.iter().any(|s| s.contains("action plan")));
    }

    #[test]
    fn clean_transcript_delegates_to_clean_segments() {
        // Both entry points should produce the same logical output
        // for an input where formatting doesn't matter.
        let raw = "Thank you.\nThank you.\nThank you.\nReal content.";
        let segments: Vec<String> = raw.lines().map(String::from).collect();
        let (transcript_out, _t_stats) = clean_transcript(raw);
        let (segments_out, _s_stats) = clean_segments(&segments);
        assert_eq!(transcript_out, segments_out.join("\n"));
    }

    #[test]
    fn clean_stats_summary_is_human_readable() {
        let input: Vec<String> = vec![
            "Hello.".into(),
            "Hello.".into(),
            "Hello.".into(),
            "World.".into(),
        ];
        let (_, stats) = clean_segments(&input);
        let summary = stats.summary();
        assert!(summary.contains("whisper-guard:"));
        assert!(summary.contains("4")); // original count
    }

    #[test]
    fn clean_segments_with_huge_input_does_not_panic() {
        // Defensive: 10k segments, all identical, should not blow up.
        let input: Vec<String> = (0..10_000).map(|_| "Thank you.".to_string()).collect();
        let (cleaned, stats) = clean_segments(&input);
        assert_eq!(stats.original_lines, 10_000);
        assert!(cleaned.len() < 10);
    }

    #[test]
    fn clean_segments_handles_unicode_correctly() {
        // Mixed scripts within a single legitimate segment shouldn't trigger filtering.
        let input: Vec<String> = vec![
            "Café meeting at 9am with Søren and José".into(),
            "Discussed naïve Bayes models".into(),
        ];
        let (cleaned, _) = clean_segments(&input);
        assert_eq!(cleaned.len(), 2, "unicode-in-Latin should not be filtered");
    }

    #[test]
    fn keep_dedup_annotations_default_true_preserves_marker() {
        let input: Vec<String> = vec![
            "Thank you.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "Real content.".into(),
        ];
        let (cleaned, _) = clean_segments(&input);
        assert!(
            cleaned
                .iter()
                .any(|s| s.starts_with(DEDUP_ANNOTATION_PREFIX)),
            "default behavior should preserve the annotation line"
        );
    }

    #[test]
    fn keep_dedup_annotations_false_strips_marker() {
        let input: Vec<String> = vec![
            "Thank you.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "Real content.".into(),
        ];
        let opts = CleanOptions {
            keep_dedup_annotations: false,
            ..CleanOptions::default()
        };
        let (cleaned, stats) = clean_segments_with_options(&input, &opts);
        assert!(
            !cleaned
                .iter()
                .any(|s| s.starts_with(DEDUP_ANNOTATION_PREFIX)),
            "annotation should be removed"
        );
        // With annotation suppressed, output is just "Thank you." + "Real content."
        // Net removed: 5 - 2 = 3.
        assert_eq!(cleaned.len(), 2);
        assert_eq!(stats.lines_removed, 3);
    }

    #[test]
    fn keep_dedup_annotations_does_not_strip_other_bracket_content() {
        // A real segment that happens to start with a bracket should NOT be filtered.
        let input: Vec<String> = vec![
            "Thank you.".into(),
            "Thank you.".into(),
            "Thank you.".into(),
            "[NAME] said the deal closed.".into(),
        ];
        let opts = CleanOptions {
            keep_dedup_annotations: false,
            ..CleanOptions::default()
        };
        let (cleaned, _) = clean_segments_with_options(&input, &opts);
        assert!(
            cleaned.iter().any(|s| s.contains("[NAME]")),
            "non-annotation bracket content must survive"
        );
    }
}
