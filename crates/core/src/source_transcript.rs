//! Final ASR for independently captured sources. Conservatively suppress long,
//! near-verbatim microphone echoes contained in contemporaneous system speech.
//! Short or uncertain matches stay. Source provenance is not a person's identity.

use crate::diarize::{CaptureSource, DiarizationResult};
use crate::transcribe::{FilterStats, TranscribeResult};

#[derive(Debug, Clone)]
struct SourceLine {
    source: CaptureSource,
    timestamp: Option<String>,
    start: f64,
    end: f64,
    text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceTranscript {
    lines: Vec<SourceLine>,
}

fn source_label(source: CaptureSource) -> &'static str {
    match source {
        CaptureSource::Voice => "MICROPHONE",
        CaptureSource::System => "SYSTEM",
        _ => unreachable!("independent ASR accepts microphone and system only"),
    }
}

fn timestamp_seconds(timestamp: &str) -> Option<f64> {
    let mut total = 0.0;
    let mut count = 0;
    for component in timestamp.split(':') {
        let value = component.parse::<f64>().ok()?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        total = total * 60.0 + value;
        count += 1;
    }
    (count == 2 || count == 3).then_some(total)
}

fn source_lines(source: CaptureSource, result: &TranscribeResult) -> Vec<SourceLine> {
    let mut last_start = 0.0_f64;
    let mut lines: Vec<_> = result
        .text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let timed = line.strip_prefix('[').and_then(|rest| rest.split_once(']'));
            let parsed = timed.and_then(|(ts, text)| {
                timestamp_seconds(ts).map(|start| (ts.to_string(), start, text.trim().to_string()))
            });
            let (timestamp, start, text) = match parsed {
                Some((ts, start, text)) => (Some(ts), start, text),
                None => (None, last_start, line.to_string()),
            };
            last_start = start;
            SourceLine {
                source,
                timestamp,
                start,
                end: start,
                text,
            }
        })
        .collect();
    // Some engines/chunked paths expose only timestamped text. Infer an end
    // from the next line in this source, never the competing source's onset.
    for index in 0..lines.len() {
        let next = lines
            .get(index + 1)
            .map(|line| line.start)
            .unwrap_or(result.stats.audio_duration_secs);
        lines[index].end = next.max(lines[index].start);
    }
    // Retain precise timings when the engine supplied the same cleaned text.
    // Match occurrences in order so repeated utterances are not conflated.
    let mut segments = result.segments.iter();
    for line in &mut lines {
        if let Some(segment) = segments.find(|segment| {
            segment.text.trim() == line.text
                && segment.start.is_finite()
                && segment.end.is_finite()
                && segment.start >= 0.0
                && segment.end >= segment.start
                && (segment.start - line.start).abs() < 1.0
        }) {
            line.start = segment.start;
            line.end = segment.end;
        }
    }
    lines
}

fn normalized_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

#[derive(Clone, Copy, Default)]
struct EchoMatch {
    words: usize,
    first_window_index: usize,
    last_window_index: usize,
    start: f64,
    end: f64,
}

/// Word LCS with the span of the system lines that actually contributed words.
/// Keep only two rows: long utterances must not allocate a quadratic matrix.
fn echo_match(microphone: &[String], system: &[(&str, &SourceLine)]) -> EchoMatch {
    let mut previous = vec![EchoMatch::default(); system.len() + 1];
    let mut current = previous.clone();
    for word in microphone {
        for (index, (system_word, line)) in system.iter().enumerate() {
            current[index + 1] = if word == system_word {
                let diagonal = previous[index];
                EchoMatch {
                    words: diagonal.words + 1,
                    first_window_index: if diagonal.words == 0 {
                        index
                    } else {
                        diagonal.first_window_index
                    },
                    last_window_index: index,
                    start: if diagonal.words == 0 {
                        line.start
                    } else {
                        diagonal.start.min(line.start)
                    },
                    end: diagonal.end.max(line.end),
                }
            } else if previous[index + 1].words >= current[index].words {
                previous[index + 1]
            } else {
                current[index]
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[system.len()]
}

fn suppress_cross_source_echo(lines: &mut Vec<SourceLine>) -> usize {
    const SLACK_SECS: f64 = 1.5;
    // The input is time-sorted. Untimed/invalid spans cannot establish echo.
    let system: Vec<_> = lines
        .iter()
        .filter(|line| {
            line.source == CaptureSource::System
                && line.timestamp.is_some()
                && line.start.is_finite()
                && line.end.is_finite()
                && line.end > line.start
        })
        .map(|line| (line, normalized_words(&line.text)))
        .collect();
    let mut remove = vec![false; lines.len()];
    for (index, line) in lines.iter().enumerate() {
        if line.source != CaptureSource::Voice
            || line.timestamp.is_none()
            || !line.start.is_finite()
            || !line.end.is_finite()
            || line.end <= line.start
        {
            continue;
        }
        let microphone = normalized_words(&line.text);
        if microphone.len() < 4 {
            continue;
        }
        let first = system.partition_point(|(system, _)| system.start < line.start - SLACK_SECS);
        let window: Vec<_> = system[first..]
            .iter()
            .take_while(|(system, _)| system.start <= line.end + SLACK_SECS)
            .flat_map(|(system, words)| words.iter().map(|word| (word.as_str(), *system)))
            .collect();
        let matched = echo_match(&microphone, &window);
        let matched_window_words = matched.last_window_index - matched.first_window_index + 1;
        // Use only matched lines for containment; unrelated nearby system
        // speech must not extend the span to swallow a later read-back.
        remove[index] = matched.words as f64 / microphone.len() as f64 >= 0.85
            && microphone.len() - matched.words <= 2
            // An LCS scattered through a longer sentence is not near-verbatim
            // echo. The matched system span can be at most two words longer
            // than the microphone line.
            && matched_window_words <= microphone.len() + 2
            && line.start >= matched.start - SLACK_SECS
            && line.end <= matched.end + SLACK_SECS;
    }
    let suppressed = remove.iter().filter(|&&remove| remove).count();
    let mut remove = remove.into_iter();
    lines.retain(|_| !remove.next().unwrap());
    suppressed
}

pub(crate) fn merge_sources(
    voice: TranscribeResult,
    system: TranscribeResult,
    cross_source_echo_suppression: bool,
) -> (TranscribeResult, SourceTranscript) {
    let mut lines = source_lines(CaptureSource::Voice, &voice);
    lines.extend(source_lines(CaptureSource::System, &system));
    // Stable sort retains both utterances at equal starts, and subsecond
    // timings where available. Never flatten overlapping source utterances.
    lines.sort_by(|a, b| a.start.total_cmp(&b.start));
    let cross_source_echo_suppressed = if cross_source_echo_suppression {
        suppress_cross_source_echo(&mut lines)
    } else {
        0
    };
    let final_words = if cross_source_echo_suppressed == 0 {
        voice.stats.final_words + system.stats.final_words
    } else {
        lines
            .iter()
            .map(|line| line.text.split_whitespace().count())
            .sum()
    };
    let sources = SourceTranscript { lines };
    let text = sources.render();
    let a = voice.stats;
    let b = system.stats;
    let stats = FilterStats {
        audio_duration_secs: a.audio_duration_secs.max(b.audio_duration_secs),
        samples_after_silence_strip: a.samples_after_silence_strip + b.samples_after_silence_strip,
        raw_segments: a.raw_segments + b.raw_segments,
        skipped_no_speech: a.skipped_no_speech + b.skipped_no_speech,
        after_no_speech_filter: a.after_no_speech_filter + b.after_no_speech_filter,
        after_dedup: a.after_dedup + b.after_dedup,
        after_interleaved: a.after_interleaved + b.after_interleaved,
        after_script_filter: a.after_script_filter + b.after_script_filter,
        after_noise_markers: a.after_noise_markers + b.after_noise_markers,
        after_trailing_trim: a.after_trailing_trim + b.after_trailing_trim,
        rescued_no_speech: a.rescued_no_speech + b.rescued_no_speech,
        final_words,
        cross_source_echo_suppressed,
    };
    let detected_language = match (voice.detected_language, system.detected_language) {
        (Some(a), Some(b)) if a == b => Some(a),
        (Some(language), None) | (None, Some(language)) => Some(language),
        _ => None,
    };
    (
        TranscribeResult {
            text,
            stats,
            segments: vec![],
            detected_language,
        },
        sources,
    )
}

impl SourceTranscript {
    fn render(&self) -> String {
        self.lines
            .iter()
            .map(|line| {
                let label = source_label(line.source);
                match &line.timestamp {
                    Some(ts) => format!("[{label} {ts}] {}\n", line.text),
                    None => format!("[{label}] {}\n", line.text),
                }
            })
            .collect()
    }

    /// Only source-aware diarization has the reserved local SPEAKER_0 vs
    /// system-label contract. An ordinary mixed-audio cluster number cannot
    /// establish source membership. Even source-aware labels must cover the
    /// whole utterance unambiguously; otherwise keep the capture-source label.
    pub(crate) fn apply_speakers(&self, transcript: &str, result: &DiarizationResult) -> String {
        let rendered: Vec<_> = transcript.lines().collect();
        if rendered.len() != self.lines.len() || !result.source_aware {
            return transcript.to_string();
        }
        self.lines
            .iter()
            .zip(rendered)
            .map(|(line, rendered)| {
                let Some((_, text)) = rendered.split_once(']') else {
                    return format!("{rendered}\n");
                };
                let Some(ts) = &line.timestamp else {
                    return format!("{rendered}\n");
                };
                let overlapping: Vec<_> = result
                    .segments
                    .iter()
                    .filter(|segment| {
                        segment.start.is_finite()
                            && segment.end.is_finite()
                            && segment.start < line.end
                            && segment.end > line.start
                    })
                    .collect();
                if overlapping
                    .iter()
                    .any(|segment| segment.speaker == "UNKNOWN")
                {
                    return format!("{rendered}\n");
                }
                let mut candidates: Vec<_> = overlapping
                    .into_iter()
                    .filter(|segment| match line.source {
                        CaptureSource::Voice => segment.speaker == "SPEAKER_0",
                        CaptureSource::System => segment.speaker != "SPEAKER_0",
                        _ => false,
                    })
                    .collect();
                candidates.sort_by(|a, b| a.start.total_cmp(&b.start));
                let Some(first) = candidates.first() else {
                    return format!("{rendered}\n");
                };
                if first.start > line.start + 0.5 {
                    return format!("{rendered}\n");
                }
                let mut end = first.end;
                for candidate in &candidates[1..] {
                    if candidate.speaker != first.speaker || candidate.start > end + 0.5 {
                        return format!("{rendered}\n");
                    }
                    end = end.max(candidate.end);
                }
                if end < line.end - 0.5 {
                    return format!("{rendered}\n");
                }
                format!(
                    "[{} {ts}] [{}] {}\n",
                    first.speaker,
                    source_label(line.source),
                    text.trim()
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diarize::SpeakerSegment;
    use crate::transcribe::TranscriptSegment;

    // Exercise the compiled default while leaving the original merge regressions
    // (including the simultaneous short "yes" case) unchanged.
    fn merge_sources(
        voice: TranscribeResult,
        system: TranscribeResult,
    ) -> (TranscribeResult, SourceTranscript) {
        super::merge_sources(
            voice,
            system,
            crate::config::TranscriptionConfig::default().cross_source_echo_suppression,
        )
    }

    fn result(text: &str, duration: f64) -> TranscribeResult {
        TranscribeResult {
            text: text.into(),
            stats: FilterStats {
                audio_duration_secs: duration,
                final_words: text.split_whitespace().count(),
                ..Default::default()
            },
            segments: vec![],
            detected_language: None,
        }
    }

    #[test]
    fn keeps_overlapping_words_and_repeated_responses_from_both_sources() {
        let (merged, _) = merge_sources(
            result("[0:02] keep annual billing\n[0:04] yes\n", 8.0),
            result(
                "[0:01] proposal\n[0:02] send it Tuesday\n[0:04] yes\n",
                10.0,
            ),
        );
        assert_eq!(merged.text, "[SYSTEM 0:01] proposal\n[MICROPHONE 0:02] keep annual billing\n[SYSTEM 0:02] send it Tuesday\n[MICROPHONE 0:04] yes\n[SYSTEM 0:04] yes\n");
        assert_eq!(
            merged.stats.audio_duration_secs, 10.0,
            "wall time is not the sum of the tracks"
        );
    }

    #[test]
    fn orders_subsecond_segments_without_requiring_structured_timing_from_every_engine() {
        let mut voice = result("[0:02] local\n", 4.0);
        voice.segments.push(TranscriptSegment {
            start: 2.8,
            end: 3.0,
            text: "local".into(),
        });
        let mut system = result("[0:02] remote\n", 4.0);
        system.segments.push(TranscriptSegment {
            start: 2.1,
            end: 2.5,
            text: "remote".into(),
        });
        let (merged, _) = merge_sources(voice, system);
        assert_eq!(
            merged.text,
            "[SYSTEM 0:02] remote\n[MICROPHONE 0:02] local\n"
        );
    }

    #[test]
    fn preserves_long_timestamps_untimed_text_and_one_empty_source() {
        let (merged, _) = merge_sources(
            result("", 5.0),
            result("[1:02:03.5] later\nuntimed suffix\n", 4000.0),
        );
        assert!(merged.text.contains("[SYSTEM 1:02:03.5] later\n"));
        assert!(merged.text.contains("[SYSTEM] untimed suffix\n"));
        assert!(!merged.text.contains("MICROPHONE"));
    }

    fn source_diarization() -> DiarizationResult {
        DiarizationResult {
            source_aware: true,
            segments: vec![
                SpeakerSegment {
                    speaker: "SPEAKER_0".into(),
                    start: 0.0,
                    end: 5.0,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_2".into(),
                    start: 0.0,
                    end: 5.0,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn attributes_simultaneous_source_utterances_without_collapsing_them() {
        let (merged, sources) = merge_sources(
            result("[0:01] my words\n", 4.0),
            result("[0:01] their words\n", 4.0),
        );
        let text = sources.apply_speakers(&merged.text, &source_diarization());
        assert_eq!(
            text,
            "[SPEAKER_0 0:01] [MICROPHONE] my words\n[SPEAKER_2 0:01] [SYSTEM] their words\n"
        );
    }

    #[test]
    fn never_assigns_a_source_from_an_ordinary_cluster_number() {
        let (merged, sources) = merge_sources(
            result("[0:01] local\n", 4.0),
            result("[0:01] remote\n", 4.0),
        );
        let mut diarization = source_diarization();
        diarization.source_aware = false;
        assert_eq!(
            sources.apply_speakers(&merged.text, &diarization),
            merged.text
        );
    }

    #[test]
    fn abstains_when_multiple_system_speakers_overlap_or_an_utterance_crosses_a_turn() {
        let (merged, sources) =
            merge_sources(result("", 4.0), result("[0:01] several people\n", 4.0));
        let mut diarization = source_diarization();
        diarization.segments.push(SpeakerSegment {
            speaker: "SPEAKER_3".into(),
            start: 2.0,
            end: 4.0,
        });
        assert_eq!(
            sources.apply_speakers(&merged.text, &diarization),
            merged.text
        );
        diarization
            .segments
            .retain(|segment| segment.speaker == "SPEAKER_0");
        assert_eq!(
            sources.apply_speakers(&merged.text, &diarization),
            merged.text,
            "system audio is never assigned to the microphone speaker"
        );
    }

    #[test]
    fn retains_normalized_text_when_source_attribution_is_added() {
        let (merged, sources) = merge_sources(result("[0:01] matt\n", 4.0), result("", 4.0));
        let normalized = merged.text.replace("matt", "Mat");
        assert!(sources
            .apply_speakers(&normalized, &source_diarization())
            .contains("Mat"));
    }

    #[test]
    fn conflicting_detected_languages_do_not_establish_one_meeting_language() {
        let mut voice = result("[0:00] hello", 4.0);
        voice.detected_language = Some("en".into());
        let mut system = result("[0:00] bonjour", 4.0);
        system.detected_language = Some("fr".into());
        assert_eq!(merge_sources(voice, system).0.detected_language, None);
    }

    fn timed(utterances: &[(f64, f64, &str)]) -> TranscribeResult {
        let text: String = utterances
            .iter()
            .map(|(start, _, text)| format!("[0:{start:04.1}] {text}\n"))
            .collect();
        let mut result = result(
            &text,
            utterances
                .iter()
                .map(|(_, end, _)| *end)
                .fold(0.0, f64::max),
        );
        result.stats.final_words = utterances
            .iter()
            .map(|(_, _, text)| text.split_whitespace().count())
            .sum();
        result.segments = utterances
            .iter()
            .map(|(start, end, text)| TranscriptSegment {
                start: *start,
                end: *end,
                text: (*text).into(),
            })
            .collect();
        result
    }

    #[test]
    fn drops_exact_microphone_echo_and_reports_removal() {
        let (merged, sources) = merge_sources(
            timed(&[(1.5, 4.0, "please send the revised proposal")]),
            timed(&[(1.0, 4.0, "please send the revised proposal")]),
        );
        assert_eq!(
            merged.text,
            "[SYSTEM 0:01.0] please send the revised proposal\n"
        );
        assert_eq!(sources.lines.len(), 1);
        assert_eq!(merged.stats.cross_source_echo_suppressed, 1);
        assert_eq!(merged.stats.final_words, 5);
        assert!(merged
            .stats
            .diagnosis()
            .contains("cross_source_echo_suppressed: 1"));
    }

    #[test]
    fn keeps_short_identical_replies() {
        for reply in ["yes", "yes please", "yes please do"] {
            let (merged, _) =
                merge_sources(timed(&[(1.0, 2.0, reply)]), timed(&[(1.0, 2.0, reply)]));
            assert_eq!(merged.stats.cross_source_echo_suppressed, 0);
            assert_eq!(merged.text.lines().count(), 2);
        }
    }

    #[test]
    fn keeps_read_back_five_seconds_later() {
        let (merged, _) = merge_sources(
            timed(&[(6.0, 8.0, "please send the revised proposal")]),
            timed(&[(1.0, 3.0, "please send the revised proposal")]),
        );
        assert_eq!(merged.stats.cross_source_echo_suppressed, 0);
        assert_eq!(merged.text.lines().count(), 2);
    }

    #[test]
    fn keeps_talk_over_with_three_extra_words_even_above_ratio_threshold() {
        let remote = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty";
        let local = format!("{remote} but I disagree");
        let (merged, _) = merge_sources(timed(&[(1.0, 4.0, &local)]), timed(&[(1.0, 4.0, remote)]));
        assert_eq!(merged.stats.cross_source_echo_suppressed, 0);
        assert!(merged.text.contains(&local));
    }

    #[test]
    fn matches_split_system_lines_case_and_surrounding_punctuation() {
        let (merged, _) = merge_sources(
            timed(&[(1.0, 5.0, "Please, SEND the revised proposal!")]),
            timed(&[
                (1.0, 3.0, "please send"),
                (3.0, 5.0, "the revised proposal."),
            ]),
        );
        assert_eq!(merged.stats.cross_source_echo_suppressed, 1);
        assert_eq!(merged.text.lines().count(), 2);
        assert!(merged.text.lines().all(|line| line.starts_with("[SYSTEM ")));
    }

    #[test]
    fn never_drops_system_lines_even_when_microphone_is_the_longer_source() {
        let system = timed(&[(1.0, 3.0, "please send the revised proposal")]);
        let expected = source_lines(CaptureSource::System, &system);
        let (merged, sources) = merge_sources(
            timed(&[(1.0, 8.0, "please send the revised proposal")]),
            system,
        );
        let retained: Vec<_> = sources
            .lines
            .iter()
            .filter(|line| line.source == CaptureSource::System)
            .collect();
        assert_eq!(retained.len(), expected.len());
        assert_eq!(retained[0].text, expected[0].text);
        assert_eq!(retained[0].end, expected[0].end);
        assert_eq!(merged.stats.cross_source_echo_suppressed, 0);
    }

    #[test]
    fn speaker_alignment_survives_microphone_echo_removal() {
        let (merged, sources) = merge_sources(
            timed(&[
                (1.0, 2.0, "please send the revised proposal"),
                (3.0, 4.0, "my distinct answer"),
            ]),
            timed(&[(1.0, 2.0, "please send the revised proposal")]),
        );
        assert_eq!(merged.stats.cross_source_echo_suppressed, 1);
        assert_eq!(sources.apply_speakers(&merged.text, &source_diarization()),
            "[SPEAKER_2 0:01.0] [SYSTEM] please send the revised proposal\n[SPEAKER_0 0:03.0] [MICROPHONE] my distinct answer\n");
    }

    #[test]
    fn switch_off_retains_identical_source_output() {
        let (merged, sources) = super::merge_sources(
            timed(&[(1.0, 3.0, "please send the revised proposal")]),
            timed(&[(1.0, 3.0, "please send the revised proposal")]),
            false,
        );
        assert_eq!(merged.text,
            "[MICROPHONE 0:01.0] please send the revised proposal\n[SYSTEM 0:01.0] please send the revised proposal\n");
        assert_eq!(sources.lines.len(), 2);
        assert_eq!(merged.stats.cross_source_echo_suppressed, 0);
        assert_eq!(merged.stats.final_words, 10);
    }

    #[test]
    fn tolerates_one_asr_word_difference_but_keeps_three() {
        let remote = "please send the revised proposal to me";
        for (local, suppressed) in [
            ("please send a revised proposal to me", 1),
            ("please send a different document to me", 0),
        ] {
            let (merged, _) =
                merge_sources(timed(&[(1.0, 3.0, local)]), timed(&[(1.0, 3.0, remote)]));
            assert_eq!(
                merged.stats.cross_source_echo_suppressed, suppressed,
                "{local}"
            );
        }
    }

    #[test]
    fn keeps_talk_over_when_matching_system_words_are_scattered() {
        let (merged, _) = merge_sources(
            timed(&[(1.0, 3.0, "I think we should")]),
            timed(&[(1.0, 3.0, "I think that maybe we really should wait")]),
        );
        assert_eq!(merged.stats.cross_source_echo_suppressed, 0);
        assert_eq!(merged.text.lines().count(), 2);
        assert!(merged
            .text
            .contains("[MICROPHONE 0:01.0] I think we should\n"));
    }

    #[test]
    fn drops_echo_with_one_inserted_system_asr_filler_word() {
        let (merged, _) = merge_sources(
            timed(&[(1.0, 3.0, "please send the revised proposal")]),
            timed(&[(1.0, 3.0, "please send um the revised proposal")]),
        );
        assert_eq!(merged.stats.cross_source_echo_suppressed, 1);
        assert_eq!(
            merged.text,
            "[SYSTEM 0:01.0] please send um the revised proposal\n"
        );
    }

    #[test]
    fn keeps_read_back_when_only_unrelated_system_speech_extends_the_window() {
        let (merged, _) = merge_sources(
            timed(&[(2.5, 5.0, "please send the revised proposal")]),
            timed(&[
                (1.0, 2.0, "please send the revised proposal"),
                (4.0, 10.0, "an unrelated follow up"),
            ]),
        );
        assert_eq!(merged.stats.cross_source_echo_suppressed, 0);
    }

    #[test]
    fn requires_timing_and_respects_start_slack_and_lcs_ratio() {
        for (system_start, suppressed) in [(8.5, 1), (8.49, 0)] {
            let (merged, _) = merge_sources(
                timed(&[(10.0, 12.0, "please send the revised proposal")]),
                timed(&[(system_start, 12.0, "please send the revised proposal")]),
            );
            assert_eq!(merged.stats.cross_source_echo_suppressed, suppressed);
        }
        let (merged, _) = merge_sources(
            result("please send the revised proposal", 3.0),
            timed(&[(0.0, 3.0, "please send the revised proposal")]),
        );
        assert_eq!(merged.stats.cross_source_echo_suppressed, 0);
        for (local, suppressed) in [
            (
                "one two three four five six seven eight nine ten eleven twelve extra words",
                1,
            ),
            (
                "one two three four five six seven eight nine ten eleven extra words",
                0,
            ),
        ] {
            let (merged, _) = merge_sources(
                timed(&[(1.0, 3.0, local)]),
                timed(&[(
                    1.0,
                    3.0,
                    "one two three four five six seven eight nine ten eleven twelve",
                )]),
            );
            assert_eq!(
                merged.stats.cross_source_echo_suppressed, suppressed,
                "{local}"
            );
        }
    }
}
