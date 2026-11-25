//! Silero speech gating for whole-file whisper, done by Minutes (#1417).
//!
//! whisper.cpp applies its integrated VAD only in `whisper_full` and
//! `whisper_full_parallel`. whisper-rs `WhisperState::full` calls
//! `whisper_full_with_state`, which ignores `params.vad`, so setting
//! `enable_vad` never gated anything. Minutes runs Silero itself, and
//! [`gate`] keeps only the speech: each Silero stretch, padded, with
//! stretches less than [`JOIN_PAUSE_SAMPLES`] apart joined with the audio
//! between them, and [`JOIN_PAUSE_SAMPLES`] of digital silence between the
//! resulting runs.
//!
//! This is not whisper.cpp's own VAD. That inserts 0.1 s between stretches
//! and interpolates times back across each removed pause. Whisper starts a
//! segment where the one before it ended, often before the speech, so across
//! a 0.1 s join an error of a few hundredths becomes tens of seconds on the
//! original clock: measured, a line after a 60 s pause was labelled a minute
//! early. Here every run keeps its exact offset, the pause between runs is
//! long enough for whisper to end a segment in it, and
//! [`SpeechGate::place_segment`] starts a segment in the run that holds its
//! speech, the rule the meeting decode groups use
//! (`decode_groups::place_segment`).

const RATE: usize = 16_000;

/// Silence between runs, and the shortest pause that is cut. The pause the
/// meeting decode groups put between their stretches
/// (`decode_groups::GROUP_PAUSE_SAMPLES`).
pub(crate) const JOIN_PAUSE_SAMPLES: usize = RATE;

/// Audio kept on each side of a Silero stretch, so a soft onset or a
/// trailing consonant is not cut.
const RUN_PAD_SAMPLES: usize = RATE / 5;

/// A segment must hold this much of a run to start in it
/// (`decode_groups::SPEECH_START_MIN_SAMPLES`).
const START_MIN_SAMPLES: usize = 3 * RATE / 10;

/// A run of kept input audio and where it is in the gated audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Run {
    input_start: usize,
    output_start: usize,
    len: usize,
    /// Where Silero heard the speech of the run start, before padding.
    speech_start: usize,
}

/// The speech-only audio for whisper and the way back to the input clock.
#[derive(Debug)]
pub(crate) struct SpeechGate {
    pub(crate) samples: Vec<f32>,
    runs: Vec<Run>,
}

/// Keep the speech of `samples`. `speech_cs` are Silero's stretches in
/// centiseconds. `None` when no stretch holds any audio: no speech.
pub(crate) fn gate(samples: &[f32], speech_cs: &[(i64, i64)]) -> Option<SpeechGate> {
    let to_sample = |cs: i64| ((cs.max(0) as usize) * RATE / 100).min(samples.len());
    let mut ranges: Vec<(usize, usize, usize)> = speech_cs
        .iter()
        .map(|&(start, end)| (to_sample(start), to_sample(end)))
        // Pad only a stretch with audio in it.
        .filter(|(start, end)| end > start)
        .map(|(start, end)| {
            (
                start.saturating_sub(RUN_PAD_SAMPLES),
                (end + RUN_PAD_SAMPLES).min(samples.len()),
                start,
            )
        })
        .collect();
    ranges.sort_unstable();
    // (padded start, padded end, speech start)
    let mut joined: Vec<(usize, usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end, speech) in ranges {
        match joined.last_mut() {
            Some(last) if start < last.1 + JOIN_PAUSE_SAMPLES => {
                last.1 = last.1.max(end);
                last.2 = last.2.min(speech);
            }
            _ => joined.push((start, end, speech)),
        }
    }
    if joined.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    let mut runs = Vec::with_capacity(joined.len());
    for (index, (start, end, speech_start)) in joined.into_iter().enumerate() {
        if index > 0 {
            out.resize(out.len() + JOIN_PAUSE_SAMPLES, 0.0);
        }
        runs.push(Run {
            input_start: start,
            output_start: out.len(),
            len: end - start,
            speech_start,
        });
        out.extend_from_slice(&samples[start..end]);
    }
    // Whisper needs a second of audio to decode; a short clip with a brief
    // utterance gets trailing silence, which no run claims.
    if out.len() < RATE {
        out.resize(RATE, 0.0);
    }
    Some(SpeechGate { samples: out, runs })
}

/// Which side of a cut pause a time belongs to.
#[derive(Debug, Clone, Copy)]
enum Edge {
    /// Something starting in a pause starts with the next run.
    Start,
    /// Something ending in a pause ended with the run before.
    End,
}

impl SpeechGate {
    /// Seconds of speech kept.
    pub(crate) fn speech_secs(&self) -> f64 {
        self.runs.iter().map(|run| run.len).sum::<usize>() as f64 / RATE as f64
    }

    /// The input time, in samples, of the gated time `output` (samples).
    /// Monotonic for each edge. A start on a run's last sample or in the
    /// pause after it belongs to the next run; an end on a run's first
    /// sample or in the pause before it belongs to the run before.
    fn input_at(&self, output: f64, edge: Edge) -> f64 {
        let after = match edge {
            Edge::Start => self
                .runs
                .partition_point(|run| run.output_start as f64 <= output),
            Edge::End => self
                .runs
                .partition_point(|run| (run.output_start as f64) < output),
        };
        let Some(run) = after.checked_sub(1).map(|index| self.runs[index]) else {
            return self.runs[0].input_start as f64;
        };
        let within = output - run.output_start as f64;
        let inside = match edge {
            Edge::Start => within < run.len as f64,
            Edge::End => within <= run.len as f64,
        };
        if inside {
            return run.input_start as f64 + within;
        }
        match (edge, self.runs.get(after)) {
            (Edge::Start, Some(next)) => next.input_start as f64,
            _ => (run.input_start + run.len) as f64,
        }
    }

    /// Input-clock `(start, end)` in seconds of a segment whisper placed at
    /// `start..end` seconds of the gated audio. The segment starts in the
    /// first run it holds at least [`START_MIN_SAMPLES`] of, else in the run
    /// it holds most of, and not before Silero heard that run's speech start:
    /// whisper often starts a segment at the start of its audio or where the
    /// segment before it ended, both before the speech.
    pub(crate) fn place_segment(&self, start: f64, end: f64) -> (f64, f64) {
        let from = start * RATE as f64;
        let to = end.max(start) * RATE as f64;
        let mut home: Option<(f64, Run)> = None;
        for run in &self.runs {
            let run_from = run.output_start as f64;
            let held = to.min(run_from + run.len as f64) - from.max(run_from);
            if held >= START_MIN_SAMPLES as f64 {
                home = Some((held, *run));
                break;
            }
            if held > 0.0 && home.is_none_or(|(most, _)| held > most) {
                home = Some((held, *run));
            }
        }
        let input_start = match home {
            Some((_, run)) => self
                .input_at(from.max(run.output_start as f64), Edge::Start)
                .max(run.speech_start as f64),
            None => self.input_at(from, Edge::Start),
        };
        let input_end = self.input_at(to, Edge::End).max(input_start);
        (input_start / RATE as f64, input_end / RATE as f64)
    }

    /// Input-clock `(start, end)` in seconds of a word whisper placed at
    /// `start..end` seconds of the gated audio, kept inside its segment's
    /// placed span `segment`.
    pub(crate) fn place_word(&self, start: f64, end: f64, segment: (f64, f64)) -> (f64, f64) {
        let start = (self.input_at(start * RATE as f64, Edge::Start) / RATE as f64)
            .clamp(segment.0, segment.1);
        let end =
            (self.input_at(end * RATE as f64, Edge::End) / RATE as f64).clamp(start, segment.1);
        (start, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(secs: usize) -> Vec<f32> {
        (0..secs * RATE).map(|i| i as f32).collect()
    }

    /// Speech at 2-5 s and 30-33 s of a 40 s file.
    fn two_stretches() -> SpeechGate {
        gate(&ramp(40), &[(200, 500), (3000, 3300)]).unwrap()
    }

    #[test]
    fn runs_are_padded_and_joined_by_one_second_of_silence() {
        let g = two_stretches();
        let pad = RUN_PAD_SAMPLES;
        let first = 3 * RATE + 2 * pad;
        assert_eq!(g.samples.len(), 2 * first + JOIN_PAUSE_SAMPLES);
        assert_eq!(g.samples[0], (2 * RATE - pad) as f32);
        assert!(g.samples[first..first + JOIN_PAUSE_SAMPLES]
            .iter()
            .all(|s| *s == 0.0));
        assert_eq!(
            g.samples[first + JOIN_PAUSE_SAMPLES],
            (30 * RATE - pad) as f32
        );
        assert!((g.speech_secs() - 6.8).abs() < 1e-9);
    }

    #[test]
    fn close_stretches_are_one_run_with_the_audio_between() {
        let g = gate(&ramp(10), &[(100, 200), (250, 400)]).unwrap();
        assert_eq!(g.runs.len(), 1);
        assert_eq!(g.samples.len(), 3 * RATE + 2 * RUN_PAD_SAMPLES);
    }

    #[test]
    fn times_inside_a_run_keep_their_exact_offset() {
        let g = two_stretches();
        // 1 s into the gated audio is 2.8 s of input (2 s minus 0.2 s pad).
        let (start, end) = g.place_segment(1.0, 2.0);
        assert!((start - 2.8).abs() < 1e-9, "{start}");
        assert!((end - 3.8).abs() < 1e-9, "{end}");
        // A segment from the start of the audio starts with the speech.
        let (start, _) = g.place_segment(0.0, 2.0);
        assert!((start - 2.0).abs() < 1e-9, "{start}");
    }

    #[test]
    fn a_segment_that_starts_early_starts_with_its_speech() {
        let g = two_stretches();
        // Run 1 is 0-3.4 s gated, the pause 3.4-4.4 s, run 2 from 4.4 s
        // (input 29.8 s, speech from 30 s). Whisper ends the first segment
        // and starts the next at 3.3 s, in run 1's tail.
        let (start, end) = g.place_segment(3.3, 6.0);
        assert!((start - 30.0).abs() < 1e-9, "{start}");
        assert!((end - 31.4).abs() < 1e-9, "{end}");
        // Starting in the pause is the same.
        let (start, _) = g.place_segment(3.9, 6.0);
        assert!((start - 30.0).abs() < 1e-9, "{start}");
    }

    #[test]
    fn a_segment_that_ends_in_the_pause_ends_with_its_run() {
        let g = two_stretches();
        let (start, end) = g.place_segment(2.0, 4.0);
        assert!((start - 3.8).abs() < 1e-9, "{start}");
        assert!((end - 5.2).abs() < 1e-9, "{end}");
    }

    #[test]
    fn words_snap_to_their_side_of_a_pause_inside_the_segment() {
        let g = two_stretches();
        let segment = g.place_segment(3.3, 6.0);
        // A word whisper put in the pause starts with run 2.
        let (start, end) = g.place_word(3.5, 4.8, segment);
        assert!((start - 30.0).abs() < 1e-9, "{start}");
        assert!((end - 30.2).abs() < 1e-9, "{end}");
        // A word before the segment's start is clamped to it.
        let (start, end) = g.place_word(3.3, 3.35, segment);
        assert_eq!((start, end), (segment.0, segment.0));
    }

    #[test]
    fn pause_boundaries_belong_to_the_right_run() {
        let g = two_stretches();
        // Run 1 ends at 3.4 s gated (input 5.2 s); run 2 starts at 4.4 s.
        let end = g.input_at(4.4 * RATE as f64, Edge::End) / RATE as f64;
        assert!((end - 5.2).abs() < 1e-9, "{end}");
        let start = g.input_at(3.4 * RATE as f64, Edge::Start) / RATE as f64;
        assert!((start - 29.8).abs() < 1e-9, "{start}");
    }

    #[test]
    fn words_across_a_pause_keep_their_order() {
        let g = two_stretches();
        let segment = g.place_segment(3.3, 6.0);
        let mut last = f64::MIN;
        for at in [3.39, 3.4, 4.0, 4.39, 4.4, 4.41, 5.0] {
            let (start, end) = g.place_word(at, at + 0.05, segment);
            assert!(start >= last, "{at}: {start} < {last}");
            assert!(end >= start);
            last = start;
        }
    }

    #[test]
    fn a_merged_run_keeps_its_earliest_onset() {
        let g = gate(&ramp(5), &[(10, 80), (0, 100)]).unwrap();
        assert_eq!(g.runs.len(), 1);
        assert_eq!(g.runs[0].speech_start, 0);
    }

    #[test]
    fn a_short_clip_still_gives_whisper_a_second() {
        let g = gate(&ramp(10), &[(400, 440)]).unwrap();
        assert_eq!(g.samples.len(), RATE);
        assert!((g.speech_secs() - 0.8).abs() < 1e-9);
        let (start, end) = g.place_segment(0.0, 0.9);
        assert!((start - 4.0).abs() < 1e-9, "{start}");
        assert!((end - 4.6).abs() < 1e-9, "{end}");
    }

    #[test]
    fn no_speech_means_no_audio() {
        assert!(gate(&ramp(1), &[]).is_none());
        assert!(gate(&[], &[(0, 100)]).is_none());
        assert!(gate(&ramp(1), &[(500, 600)]).is_none());
    }

    #[test]
    fn times_past_the_audio_end_with_the_last_run() {
        let g = two_stretches();
        let (start, end) = g.place_segment(100.0, 120.0);
        assert!((start - 33.2).abs() < 1e-9, "{start}");
        assert!((end - 33.2).abs() < 1e-9, "{end}");
    }
}
