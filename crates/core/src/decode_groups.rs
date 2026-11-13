//! Decode groups: how meeting audio is cut into decoder calls.
//!
//! Whisper encodes a full 30 s window for every call and pads shorter audio to
//! 30 s. With one call per VAD chunk, most of that work is padding, because
//! most meeting chunks are short (issue #1340). The Whisper meeting path packs
//! adjacent VAD chunks into groups and decodes each group in one call.
//!
//! The decoder gets the chunks of a group with the pauses between them. A
//! pause of up to [`GROUP_PAUSE_SAMPLES`] is kept whole. A longer pause is cut
//! to that length: half of it stays after the chunk before, half before the
//! chunk after. Whisper needs the pauses to keep short answers apart and to
//! time them. Without them it joins the end of one chunk to the start of the
//! next. The kept pauses count toward the group target. The decoder audio is
//! made of [runs](Run) of source audio, and each output time is mapped back to
//! the source clock through the run that holds it, sample for sample.
//!
//! One plan serves all three paths: the whole-file pass, the checkpoint spans
//! that a host transcribes while recording, and the gaps between accepted
//! spans. Groups are packed greedily from the start of each source. Each
//! finished group has a [cut](DecodePlan::cuts): the point after it where the
//! VAD chunker resets. A span that starts at one cut and ends at another gets
//! the same chunks and the same groups as the whole-source plan. The
//! recording-time producer cuts spans only there. While a source is still
//! recording, a group whose end is not known yet has no cut, and is left to
//! the final pass. The final pass accepts a span only when its own plan is
//! exactly the whole-source plan for that audio ([`DecodePlan::matches_span`]).
//!
//! The loop guard: when a line of a group with more than one chunk is a
//! repetition loop ([`whisper_guard::find_repetition_loop`]), the group is
//! decoded again once, one call per chunk. The per-chunk output is kept even
//! if it loops too. That is what the per-chunk pass produced before grouping,
//! and it bounds the extra work to one call per chunk of a flagged group.
//!
//! Other engines keep one call per chunk: their plan has one chunk per group.

use crate::error::TranscribeError;
use crate::transcribe::{ChunkLines, FilterStats};
use whisper_guard::segments::{find_repetition_loop, LOOP_MIN_REPEATS, LOOP_MIN_WORDS};

const RATE: usize = 16_000;

/// The packing target for one Whisper call: the audio the decoder gets for a
/// group, which is its chunks and the kept pauses between them. It is a
/// target, not a maximum. A VAD chunk longer than the target (the chunker
/// splits speech only at 45 s) is a group of its own.
pub const WHISPER_GROUP_TARGET_SAMPLES: usize = 30 * RATE;

/// The longest pause that the decoder gets between two chunks of a group.
/// The chunker closes a chunk after 0.8 s of silence, so most pauses inside a
/// conversation are kept whole. A stem of a call is quiet while the other
/// side speaks. Those long pauses are cut to this length, so that they do not
/// use up the 30 s window.
pub const GROUP_PAUSE_SAMPLES: usize = RATE;

/// Change when grouping or the loop retry changes in a way that the values in
/// [`strategy_id`] do not show. Checkpoint spans made with another strategy
/// are then rejected and decoded again.
const GROUPED_STRATEGY_VERSION: u32 = 2;

/// The pause that a group keeps between two chunks that are `gap` samples
/// apart.
fn kept_pause(gap: usize) -> usize {
    gap.min(GROUP_PAUSE_SAMPLES)
}

/// The group target that the Whisper meeting path uses.
pub(crate) fn whisper_group_target_samples() -> usize {
    WHISPER_GROUP_TARGET_SAMPLES
}

/// The decode strategy for a group target, as recorded in the checkpoint
/// settings fingerprint. A target of 0 means one call per chunk.
pub(crate) fn strategy_id(group_target: usize) -> String {
    if group_target == 0 {
        return "per-chunk".into();
    }
    format!(
        "groups-v{GROUPED_STRATEGY_VERSION};target={group_target};\
         pause={GROUP_PAUSE_SAMPLES};loop={LOOP_MIN_REPEATS}x{LOOP_MIN_WORDS}w;\
         retry=per-chunk-once"
    )
}

/// Adjacent VAD chunks that are decoded in one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeGroup {
    /// VAD chunks `[start, end)`, in samples of the planned audio, in order.
    pub chunks: Vec<(usize, usize)>,
}

impl DecodeGroup {
    pub fn start(&self) -> usize {
        self.chunks[0].0
    }

    pub fn end(&self) -> usize {
        self.chunks[self.chunks.len() - 1].1
    }

    /// The samples the decoder gets for this group: its chunks and the kept
    /// pauses between them.
    pub fn audio_samples(&self) -> usize {
        let chunks: usize = self.chunks.iter().map(|&(start, end)| end - start).sum();
        let pauses: usize = self
            .chunks
            .windows(2)
            .map(|pair| kept_pause(pair[1].0 - pair[0].1))
            .sum();
        chunks + pauses
    }

    /// [`audio_samples`](Self::audio_samples) of this group with `chunk`
    /// added after its last chunk.
    fn audio_samples_with(&self, (start, end): (usize, usize)) -> usize {
        self.audio_samples() + kept_pause(start - self.end()) + (end - start)
    }

    fn offset(&self, by: usize) -> Self {
        Self {
            chunks: self
                .chunks
                .iter()
                .map(|&(start, end)| (start + by, end + by))
                .collect(),
        }
    }
}

/// Pack chunks greedily, in order. A chunk joins the open group while the
/// group's audio, with the kept pause before this chunk and this chunk, is at
/// most `target` samples. Otherwise it starts a new group. So a chunk longer
/// than `target` is a group of its own, and a target of 0 gives one group per
/// chunk.
pub fn pack_groups(chunks: &[(usize, usize)], target: usize) -> Vec<DecodeGroup> {
    let mut groups: Vec<DecodeGroup> = Vec::new();
    for &chunk in chunks {
        match groups.last_mut() {
            Some(group) if group.audio_samples_with(chunk) <= target => group.chunks.push(chunk),
            _ => groups.push(DecodeGroup {
                chunks: vec![chunk],
            }),
        }
    }
    groups
}

/// The decode groups of one source, or of a span of it, and the points where
/// a span may start or end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodePlan {
    chunk_count: usize,
    groups: Vec<DecodeGroup>,
    /// Groups whose end is known. All of them for complete audio.
    finished: usize,
    /// `cuts[k]` is where a span that ends with group `k` ends.
    cuts: Vec<usize>,
}

impl DecodePlan {
    /// The plan for complete audio: a whole source in the final pass, or one
    /// checkpoint span. Its last group ends at the end of `samples`.
    pub fn for_source(samples: &[f32], group_target: usize) -> Self {
        let chunks = crate::transcribe::detect_meeting_vad_chunks(samples);
        Self::from_chunks(&chunks, samples.len(), group_target, true)
    }

    /// The plan for the audio received so far of a source that is still
    /// recording. `samples` must start at the source start or at a cut of an
    /// earlier plan. Only groups whose end is known get a cut.
    pub fn for_growing_source(samples: &[f32], group_target: usize) -> Self {
        let chunks = crate::transcribe::detect_meeting_vad_chunks(samples);
        Self::from_chunks(&chunks, samples.len(), group_target, false)
    }

    fn from_chunks(chunks: &[(usize, usize)], len: usize, target: usize, complete: bool) -> Self {
        let groups = pack_groups(chunks, target);
        let mut cuts = Vec::with_capacity(groups.len());
        if complete && groups.is_empty() {
            cuts.push(len);
        }
        for (index, group) in groups.iter().enumerate() {
            let Some(next) = groups.get(index + 1) else {
                if complete {
                    cuts.push(len);
                }
                break;
            };
            // Only the last chunk of growing audio can still change, and it
            // only grows. Its group is the last one. The group before it is
            // finished when even its current length does not fit.
            let next_is_open = !complete && index + 2 == groups.len() && next.chunks.len() == 1;
            if next_is_open && group.audio_samples_with(next.chunks[0]) <= target {
                break;
            }
            cuts.push(chunker_reset_after(group.chunks[group.chunks.len() - 1]));
        }
        Self {
            chunk_count: chunks.len(),
            finished: cuts.len().min(groups.len()),
            groups,
            cuts,
        }
    }

    /// VAD chunks in the planned audio.
    pub fn chunk_count(&self) -> usize {
        self.chunk_count
    }

    /// All groups, in order. For growing audio, the groups after
    /// [`finished_groups`](Self::finished_groups) can still change.
    pub fn groups(&self) -> &[DecodeGroup] {
        &self.groups
    }

    /// The groups whose end is known.
    pub fn finished_groups(&self) -> &[DecodeGroup] {
        &self.groups[..self.finished]
    }

    /// Where a span may end, one point after each finished group, in order.
    /// It is the point where the VAD chunker resets after that group, so the
    /// next span starts in the state that the whole-source pass has there.
    /// For complete audio, the last cut is the end of the audio.
    pub fn cuts(&self) -> &[usize] {
        &self.cuts
    }

    /// Whether `span_samples`, taken from `start` of the audio this plan
    /// covers, have exactly this plan's groups. Then decoding the span gives
    /// the lines that the whole-source pass gives for that audio. A group
    /// that crosses either edge of the span fails the check.
    pub fn matches_span(&self, span_samples: &[f32], start: usize, group_target: usize) -> bool {
        let end = start + span_samples.len();
        let inside: Vec<&DecodeGroup> = self
            .groups
            .iter()
            .filter(|group| group.end() > start && group.start() < end)
            .collect();
        let span = Self::for_source(span_samples, group_target);
        span.groups.len() == inside.len()
            && span
                .groups
                .iter()
                .zip(inside)
                .all(|(own, whole)| own.offset(start) == *whole)
    }
}

/// The sample where the VAD chunker is back in its start state after it
/// closes `chunk`. It closes a chunk at a pause after 800 ms of silence, or
/// splits speech without a pause at 45 s.
fn chunker_reset_after((start, end): (usize, usize)) -> usize {
    if end - start >= crate::transcribe::VAD_MAX_CHUNK_SAMPLES {
        end
    } else {
        end + crate::transcribe::VAD_CLOSE_PAUSE_SAMPLES
    }
}

/// One line of decoder output. Times are seconds from the start of the audio
/// that the decoder got.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TimedText {
    pub(crate) start: f64,
    pub(crate) end: f64,
    pub(crate) text: String,
}

/// What a decoder returned for one call.
pub(crate) struct DecodedAudio {
    pub(crate) segments: Vec<TimedText>,
    pub(crate) stats: FilterStats,
}

/// One decoder call on 16 kHz mono audio. `Ok(None)` when the audio has no
/// speech.
pub(crate) trait AudioDecoder {
    fn decode(&mut self, audio: &[f32]) -> Result<Option<DecodedAudio>, TranscribeError>;
}

impl<F> AudioDecoder for F
where
    F: FnMut(&[f32]) -> Result<Option<DecodedAudio>, TranscribeError>,
{
    fn decode(&mut self, audio: &[f32]) -> Result<Option<DecodedAudio>, TranscribeError> {
        self(audio)
    }
}

/// Decode `groups` of `samples`, one call per group, with the loop guard.
/// Lines are `[m:ss] text` on the source clock: `clock_offset` is the source
/// sample at which `samples` starts. Lines are not cleaned up here. Cleanup
/// runs once over the whole assembled source.
#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
pub(crate) fn decode_groups(
    decoder: &mut impl AudioDecoder,
    samples: &[f32],
    groups: &[DecodeGroup],
    clock_offset: usize,
) -> Result<ChunkLines, TranscribeError> {
    let mut out = ChunkLines {
        lines: Vec::new(),
        stats: FilterStats::default(),
    };
    let mut calls = 0usize;
    let mut retried = 0usize;
    let mut still_looping = 0usize;
    for group in groups {
        calls += 1;
        let mut lines = decode_chunks(decoder, samples, &group.chunks, clock_offset)?;
        if group.chunks.len() > 1 {
            if let Some(found) = lines
                .lines
                .iter()
                .find_map(|line| find_repetition_loop(line))
            {
                retried += 1;
                tracing::warn!(
                    start_sample = clock_offset + group.start(),
                    end_sample = clock_offset + group.end(),
                    chunks = group.chunks.len(),
                    phrase_words = found.phrase_words,
                    repeats = found.repeats,
                    "repetition loop in a decode group; decoding its chunks one by one"
                );
                lines = ChunkLines {
                    lines: Vec::new(),
                    stats: FilterStats::default(),
                };
                for chunk in &group.chunks {
                    calls += 1;
                    let one =
                        decode_chunks(decoder, samples, std::slice::from_ref(chunk), clock_offset)?;
                    lines.add_stats(&one.stats);
                    lines.lines.extend(one.lines);
                }
                if lines
                    .lines
                    .iter()
                    .any(|line| find_repetition_loop(line).is_some())
                {
                    still_looping += 1;
                }
            }
        }
        out.add_stats(&lines.stats);
        out.lines.extend(lines.lines);
    }
    tracing::info!(
        groups = groups.len(),
        calls,
        retried,
        still_looping,
        "decoded meeting audio in groups"
    );
    if retried > 0 {
        crate::logging::log_step(
            "decode_group_loop_retry",
            "",
            0,
            serde_json::json!({
                "groups": groups.len(), "calls": calls,
                "retried": retried, "still_looping": still_looping,
            }),
        );
    }
    Ok(out)
}

/// A run of source audio in the audio of one decoder call: `len` samples
/// from `source` of the planned audio are at `joined` of the decoder audio.
/// The decoder clock and the source clock run together inside a run. Between
/// two runs, a part of a pause is left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Run {
    joined: usize,
    source: usize,
    len: usize,
}

/// The runs of one decoder call on `chunks`: the chunks and the kept pauses
/// between them. Two chunks with a pause of at most [`GROUP_PAUSE_SAMPLES`]
/// between them are in one run.
fn group_runs(chunks: &[(usize, usize)]) -> Vec<Run> {
    let keep = GROUP_PAUSE_SAMPLES;
    let mut runs: Vec<Run> = Vec::new();
    for &(start, end) in chunks {
        let Some(run) = runs.last_mut() else {
            runs.push(Run {
                joined: 0,
                source: start,
                len: end - start,
            });
            continue;
        };
        let gap = start - (run.source + run.len);
        if gap <= keep {
            run.len += gap + (end - start);
            continue;
        }
        let after = keep / 2;
        let before = keep - after;
        run.len += after;
        let joined = run.joined + run.len;
        runs.push(Run {
            joined,
            source: start - before,
            len: before + (end - start),
        });
    }
    runs
}

/// One decoder call on `chunks` with the kept pauses between them.
fn decode_chunks(
    decoder: &mut impl AudioDecoder,
    samples: &[f32],
    chunks: &[(usize, usize)],
    clock_offset: usize,
) -> Result<ChunkLines, TranscribeError> {
    let mut out = ChunkLines {
        lines: Vec::new(),
        stats: FilterStats::default(),
    };
    let runs = group_runs(chunks);
    let audio: Vec<f32> = runs
        .iter()
        .flat_map(|run| samples[run.source..run.source + run.len].iter().copied())
        .collect();
    if audio.is_empty() {
        return Ok(out);
    }
    let Some(decoded) = decoder.decode(&audio)? else {
        return Ok(out);
    };
    out.stats = decoded.stats;
    let speech = speech_stretches(samples, chunks);
    out.lines = decoded
        .segments
        .iter()
        .map(|segment| {
            let secs = source_secs(&speech, &runs, clock_offset, segment);
            let secs = secs.max(0.0).floor() as u64;
            format!("[{}:{:02}] {}", secs / 60, secs % 60, segment.text)
        })
        .collect();
    Ok(out)
}

/// A segment must have this much of a stretch of speech to start in it. The
/// end of the segment before it is where Whisper starts a segment, and that
/// time can be a little before the speech really ends.
const SPEECH_START_MIN_SAMPLES: usize = 3 * RATE / 10;

/// The source time of a segment from a call on `chunks`, whose audio is
/// `runs`. `speech` is [`speech_stretches`] of the chunks.
///
/// Whisper starts a segment where the segment before it ended. So a start is
/// often early: in the quiet end of the speech before, or anywhere in a
/// pause. The end of a segment is close to the end of its speech. So the
/// segment starts in the first stretch of speech that it holds at least
/// [`SPEECH_START_MIN_SAMPLES`] of, or else in the stretch it holds most of.
/// A segment that starts before that stretch gets the start of the stretch.
/// For the first speech of a chunk, that is the time the per-chunk pass gave
/// it. A segment that starts inside the stretch keeps its exact time.
fn source_secs(
    speech: &[(usize, usize)],
    runs: &[Run],
    clock_offset: usize,
    segment: &TimedText,
) -> f64 {
    let start = segment.start * RATE as f64;
    let end = segment.end.max(segment.start) * RATE as f64;
    let mut most: Option<(f64, usize, usize)> = None; // (held, joined start, source start)
    for &(from_source, to_source) in speech {
        let joined = joined_position(runs, from_source);
        let from = joined as f64;
        let to = (joined + to_source - from_source) as f64;
        let held = end.min(to) - start.max(from);
        if held >= SPEECH_START_MIN_SAMPLES as f64 {
            most = Some((held, joined, from_source));
            break;
        }
        if held > 0.0 && most.is_none_or(|(most, _, _)| held > most) {
            most = Some((held, joined, from_source));
        }
    }
    let source = match most {
        // The start of a stretch is known to one window. A segment that
        // starts less than that before it keeps its exact time.
        Some((_, joined, source)) => {
            let before = joined as f64 - start;
            let window = crate::transcribe::VAD_WINDOW_SAMPLES as f64;
            source as f64 - if before < window { before } else { 0.0 }
        }
        // No speech at all: the segment is in a pause or after the audio. It
        // keeps the exact source time of its start.
        None => {
            let run = runs
                .iter()
                .rev()
                .find(|run| run.joined as f64 <= start)
                .unwrap_or(&runs[0]);
            run.source as f64 + (start - run.joined as f64).clamp(0.0, run.len as f64)
        }
    };
    clock_offset as f64 / RATE as f64 + source / RATE as f64
}

/// Where the source sample `source` is in the decoder audio. `source` must be
/// inside a run.
fn joined_position(runs: &[Run], source: usize) -> usize {
    let run = runs
        .iter()
        .rev()
        .find(|run| run.source <= source)
        .unwrap_or(&runs[0]);
    run.joined + (source - run.source)
}

/// The stretches of speech inside `chunks`, `[start, end)` in samples of
/// `samples`, in order.
///
/// A VAD chunk is not one stretch of speech. It ends with the quiet hangover
/// of the chunker. And the chunker does not close a chunk shorter than 1 s,
/// so a one-word answer and the sentence after a long silence can be one
/// chunk. A window of 100 ms is speech when its level is within 20 dB of the
/// loud level of its chunk. Speech windows less than the chunker hangover
/// apart are one stretch. The result depends only on the audio of each chunk,
/// so a checkpoint span and the whole-source pass get the same stretches.
fn speech_stretches(samples: &[f32], chunks: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let window = crate::transcribe::VAD_WINDOW_SAMPLES;
    let hangover = crate::transcribe::VAD_HANGOVER_SAMPLES;
    let mut stretches: Vec<(usize, usize)> = Vec::new();
    for &(chunk_start, chunk_end) in chunks {
        let levels: Vec<f32> = samples[chunk_start..chunk_end]
            .chunks(window)
            .map(|w| (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).sqrt())
            .collect();
        let mut sorted = levels.clone();
        sorted.sort_by(f32::total_cmp);
        let loud = sorted[(sorted.len() * 95 / 100).min(sorted.len() - 1)];
        let first = stretches.len();
        for (index, level) in levels.iter().enumerate() {
            if *level <= loud / 10.0 {
                continue;
            }
            let from = chunk_start + index * window;
            let to = (from + window).min(chunk_end);
            let in_this_chunk = stretches.len() > first;
            match stretches.last_mut() {
                Some(open) if in_this_chunk && from - open.1 < hangover => open.1 = to,
                _ => stretches.push((from, to)),
            }
        }
        // A chunk of one level, as in a test fixture, is one stretch.
        if stretches.len() == first {
            stretches.push((chunk_start, chunk_end));
        }
    }
    stretches
}

/// A deterministic stand-in for Whisper, for tests that must not need a model.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    /// The constant level of the speech burst `id`.
    pub(crate) fn level(id: u32) -> f32 {
        0.1 + id as f32 * 0.001
    }

    /// Silence with constant-level bursts `(start secs, length secs, id)`.
    pub(crate) fn burst_audio(total_secs: f64, bursts: &[(f64, f64, u32)]) -> Vec<f32> {
        let mut samples = vec![0.0; (total_secs * RATE as f64) as usize];
        for &(start, len, id) in bursts {
            let from = (start * RATE as f64) as usize;
            let to = ((start + len) * RATE as f64) as usize;
            samples[from..to].fill(level(id));
        }
        samples
    }

    /// Decode a burst fixture: one segment `w<id>` where each burst starts in
    /// `audio`. A call that holds three or more bursts and a burst in
    /// `loop_ids` returns a looping segment for it. In the test fixtures a
    /// chunk holds at most two bursts, so only a group call loops. With
    /// `loop_always`, a call with fewer bursts loops too. Counts calls.
    pub(crate) struct FakeWhisper {
        pub(crate) calls: usize,
        pub(crate) loop_ids: Vec<u32>,
        pub(crate) loop_always: bool,
    }

    impl FakeWhisper {
        pub(crate) fn new() -> Self {
            Self {
                calls: 0,
                loop_ids: Vec::new(),
                loop_always: false,
            }
        }

        pub(crate) fn looping(ids: &[u32]) -> Self {
            Self {
                loop_ids: ids.to_vec(),
                ..Self::new()
            }
        }
    }

    pub(crate) const LOOP_TEXT: &str = "we will send the deck by friday";

    impl AudioDecoder for FakeWhisper {
        fn decode(&mut self, audio: &[f32]) -> Result<Option<DecodedAudio>, TranscribeError> {
            self.calls += 1;
            let mut segments = Vec::new();
            for (index, &value) in audio.iter().enumerate() {
                let previous = if index == 0 { 0.0 } else { audio[index - 1] };
                if value == 0.0 || value == previous {
                    continue;
                }
                let end = audio[index..]
                    .iter()
                    .position(|&v| v != value)
                    .map_or(audio.len(), |n| index + n);
                let id = ((value - 0.1) / 0.001).round() as u32;
                segments.push(TimedText {
                    start: index as f64 / RATE as f64,
                    end: end as f64 / RATE as f64,
                    text: format!("w{id}"),
                });
            }
            if segments.is_empty() {
                return Ok(None);
            }
            let several = segments.len() > 2;
            for segment in &mut segments {
                let id: u32 = segment.text[1..].parse().unwrap();
                if self.loop_ids.contains(&id) && (several || self.loop_always) {
                    segment.text = format!("{} {}", segment.text, [LOOP_TEXT; 7].join(" "));
                }
            }
            Ok(Some(DecodedAudio {
                segments,
                stats: FilterStats::default(),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    const S: usize = RATE;

    fn chunks_of(lens_secs: &[(f64, f64)]) -> Vec<(usize, usize)> {
        lens_secs
            .iter()
            .map(|&(start, end)| ((start * S as f64) as usize, (end * S as f64) as usize))
            .collect()
    }

    fn group_spans(groups: &[DecodeGroup]) -> Vec<Vec<(usize, usize)>> {
        groups.iter().map(|group| group.chunks.clone()).collect()
    }

    #[test]
    fn packing_counts_chunk_audio_and_kept_pauses() {
        // 10 s of speech, a 60 s pause, 10 s, a 60 s pause, 9 s. A long
        // pause counts as the 1 s that the decoder gets of it: 31 s is too
        // much for one call, 21 s is not.
        let chunks = chunks_of(&[(0.0, 10.0), (70.0, 80.0), (140.0, 149.0)]);
        let groups = pack_groups(&chunks, 30 * S);
        assert_eq!(
            group_spans(&groups),
            vec![chunks[..2].to_vec(), chunks[2..].to_vec()]
        );
        assert_eq!(groups[0].audio_samples(), 21 * S);
        assert_eq!((groups[0].start(), groups[0].end()), (0, 80 * S));
        // With 8 s at the end, the three chunks and two pauses are 30 s.
        let chunks = chunks_of(&[(0.0, 10.0), (70.0, 80.0), (140.0, 148.0)]);
        let groups = pack_groups(&chunks, 30 * S);
        assert_eq!(group_spans(&groups), vec![chunks.clone()]);
        assert_eq!(groups[0].audio_samples(), 30 * S);
        // A pause shorter than 1 s counts with its own length.
        let chunks = chunks_of(&[(0.0, 10.0), (10.9, 20.0)]);
        assert_eq!(pack_groups(&chunks, 30 * S)[0].audio_samples(), 20 * S);
    }

    #[test]
    fn the_decoder_gets_short_pauses_whole_and_long_pauses_cut() {
        // Chunks at 2 s, 5.9 s and 70 s: a 0.9 s pause, then a 60 s pause.
        let chunks = chunks_of(&[(2.0, 5.0), (5.9, 10.0), (70.0, 74.0)]);
        let runs = group_runs(&chunks);
        assert_eq!(
            runs,
            vec![
                // The first two chunks, the pause between them, and the
                // first 0.5 s of the long pause.
                Run {
                    joined: 0,
                    source: 2 * S,
                    len: 8 * S + S / 2
                },
                // The last 0.5 s of the long pause, and the third chunk.
                Run {
                    joined: 8 * S + S / 2,
                    source: 70 * S - S / 2,
                    len: 4 * S + S / 2
                },
            ]
        );
        let audio: usize = runs.iter().map(|run| run.len).sum();
        assert_eq!(audio, pack_groups(&chunks, 30 * S)[0].audio_samples());
        // The decoder audio is those runs of the source, sample for sample.
        let samples: Vec<f32> = (0..75 * S).map(|n| n as f32).collect();
        let mut heard = Vec::new();
        let mut decoder = |audio: &[f32]| -> Result<Option<DecodedAudio>, TranscribeError> {
            heard = audio.to_vec();
            Ok(None)
        };
        decode_groups(&mut decoder, &samples, &pack_groups(&chunks, 30 * S), 0).unwrap();
        assert_eq!(heard.len(), audio);
        assert_eq!(heard[0], (2 * S) as f32);
        assert_eq!(heard[8 * S + S / 2 - 1], (10 * S + S / 2 - 1) as f32);
        assert_eq!(heard[8 * S + S / 2], (70 * S - S / 2) as f32);
        assert_eq!(heard[audio - 1], (74 * S - 1) as f32);
    }

    #[test]
    fn a_group_that_reaches_the_target_exactly_is_full() {
        let exact = chunks_of(&[(0.0, 20.0), (21.0, 30.0), (31.0, 31.5)]);
        let groups = pack_groups(&exact, 30 * S);
        assert_eq!(
            group_spans(&groups),
            vec![exact[..2].to_vec(), exact[2..].to_vec()]
        );
        // One sample over the target starts a new group.
        let over = vec![(0, 20 * S), (21 * S, 30 * S + 1)];
        assert_eq!(pack_groups(&over, 30 * S).len(), 2);
    }

    #[test]
    fn a_chunk_longer_than_the_target_is_a_group_of_its_own() {
        // The chunker splits speech without a pause at 45 s.
        let chunks = chunks_of(&[(0.0, 5.0), (6.0, 51.0), (52.0, 55.0), (56.0, 58.0)]);
        let groups = pack_groups(&chunks, 30 * S);
        assert_eq!(
            group_spans(&groups),
            vec![vec![chunks[0]], vec![chunks[1]], vec![chunks[2], chunks[3]]]
        );
        assert!(groups[1].audio_samples() > 30 * S);
    }

    #[test]
    fn target_zero_is_one_call_per_chunk() {
        let chunks = chunks_of(&[(0.0, 2.0), (3.0, 4.0), (5.0, 9.0)]);
        let groups = pack_groups(&chunks, 0);
        assert_eq!(groups.len(), 3);
        assert_eq!(strategy_id(0), "per-chunk");
        assert_ne!(strategy_id(30 * S), strategy_id(25 * S));
        assert_ne!(strategy_id(30 * S), strategy_id(0));
    }

    #[test]
    fn groups_cover_every_chunk_once_in_order() {
        let mut chunks = Vec::new();
        let mut t = 0usize;
        for n in 0..200usize {
            let len = [S / 2, 3 * S, 11 * S, 29 * S, 30 * S, 31 * S, 45 * S][n % 7] + n;
            chunks.push((t, t + len));
            t += len + S + n % 5 * S;
        }
        for target in [0, 25 * S, 30 * S] {
            let groups = pack_groups(&chunks, target);
            let flat: Vec<_> = groups.iter().flat_map(|g| g.chunks.clone()).collect();
            assert_eq!(flat, chunks);
            for group in &groups {
                assert!(group.chunks.len() == 1 || group.audio_samples() <= target);
            }
        }
    }

    fn fixture() -> (Vec<f32>, Vec<(f64, f64, u32)>) {
        // Bursts of 1.2 to 9 s with pauses of 0.4 to 7 s, one 50 s monologue
        // that the chunker splits at 45 s, and a short tail at the end.
        let mut bursts = Vec::new();
        let mut t = 0.7;
        for id in 0..60u32 {
            let len = [1.2, 4.0, 9.0, 2.5, 6.3][id as usize % 5];
            let len = if id == 23 { 50.0 } else { len };
            bursts.push((t, len, id));
            t += len + [0.4, 1.5, 7.0, 0.9, 2.2, 3.1][id as usize % 6];
        }
        (burst_audio(t + 0.5, &bursts), bursts)
    }

    fn expected_lines(bursts: &[(f64, f64, u32)]) -> Vec<String> {
        bursts
            .iter()
            .map(|&(start, _, id)| {
                let secs = start.floor() as u64;
                format!("[{}:{:02}] w{id}", secs / 60, secs % 60)
            })
            .collect()
    }

    #[test]
    fn grouped_lines_keep_source_timestamps_across_pauses() {
        let (samples, bursts) = fixture();
        let plan = DecodePlan::for_source(&samples, 30 * S);
        let mut fake = FakeWhisper::new();
        let out = decode_groups(&mut fake, &samples, plan.groups(), 0).unwrap();
        assert_eq!(fake.calls, plan.groups().len());
        assert!(plan.groups().len() < plan.chunk_count() / 2, "{plan:?}");
        // The 50 s burst is decoded in two calls: one per 45 s chunk side.
        let mut expected = expected_lines(&bursts);
        let split = expected.iter().position(|l| l.ends_with(" w23")).unwrap();
        let rest = out.lines[split + 1].clone();
        assert!(rest.ends_with(" w23"), "{rest}");
        expected.insert(split + 1, rest);
        assert_eq!(out.lines, expected);
        // The per-chunk plan gives the same lines with more calls.
        let per_chunk = DecodePlan::for_source(&samples, 0);
        let mut fake = FakeWhisper::new();
        let lines = decode_groups(&mut fake, &samples, per_chunk.groups(), 0).unwrap();
        assert_eq!(lines.lines, out.lines);
        assert_eq!(fake.calls, plan.chunk_count());
    }

    #[test]
    fn clock_offset_moves_lines_to_the_source_clock() {
        let (samples, bursts) = fixture();
        let plan = DecodePlan::for_source(&samples, 30 * S);
        let out = decode_groups(
            &mut FakeWhisper::new(),
            &samples,
            &plan.groups()[..1],
            3_600 * S,
        )
        .unwrap();
        // The first burst starts at 0.7 s, so at 3600.7 s on the source clock.
        assert_eq!(bursts[0].0, 0.7);
        assert_eq!(out.lines[0], "[60:00] w0");
    }

    /// The source time of a segment `start..end` of a call on `chunks`,
    /// where each chunk is speech up to the 0.5 s hangover at its end.
    fn secs_of(chunks: &[(usize, usize)], start: f64, end: f64) -> f64 {
        let speech: Vec<(usize, usize)> = chunks.iter().map(|c| (c.0, c.1 - S / 2)).collect();
        secs_with_speech(chunks, &speech, start, end)
    }

    fn secs_with_speech(
        chunks: &[(usize, usize)],
        speech: &[(usize, usize)],
        start: f64,
        end: f64,
    ) -> f64 {
        let segment = TimedText {
            start,
            end,
            text: "x".into(),
        };
        source_secs(speech, &group_runs(chunks), 0, &segment)
    }

    #[test]
    fn a_segment_that_starts_early_keeps_its_own_speech_time() {
        // Chunks at 100 s and 160 s. The decoder gets 10 s, 0.5 s of the
        // pause, 0.5 s of the pause, 5 s: the second chunk is at 11 s.
        let chunks = [(100 * S, 110 * S), (160 * S, 165 * S)];
        // Whisper starts the second segment where the first ended: in the
        // quiet end of the first chunk, 9.6 s.
        assert_eq!(secs_of(&chunks, 9.6, 15.4), 160.0);
        // Or in the kept pause.
        assert_eq!(secs_of(&chunks, 10.2, 15.4), 160.0);
        // A segment that starts inside speech keeps its exact time.
        assert_eq!(secs_of(&chunks, 2.5, 4.0), 102.5);
        assert_eq!(secs_of(&chunks, 12.0, 15.0), 161.0);
        // Also a segment that goes on over the pause into the next chunk.
        assert_eq!(secs_of(&chunks, 8.0, 15.0), 108.0);
        // Without speech, a time is mapped through its run.
        assert_eq!(secs_of(&chunks, 2.5, 2.5), 102.5);
        assert_eq!(secs_of(&chunks, 10.2, 10.4), 110.2);
        assert_eq!(secs_of(&chunks, 10.6, 10.8), 159.6);
        assert_eq!(secs_of(&chunks, 20.0, 20.0), 165.0);
    }

    #[test]
    fn a_short_answer_is_not_given_to_the_speech_before_it() {
        // "Okay, then we ship on Friday." and, 1.2 s later, "Agreed.", as
        // the VAD chunker cuts them. Whisper ended the first segment 0.27 s
        // before the speech of the first chunk ends, and started "Agreed."
        // there. That is less than a segment needs to start in a stretch.
        let chunks = chunks_of(&[(112.4, 115.1), (116.3, 117.4)]);
        let first_speech_ends = 2.7 - 0.5;
        let agreed_at = 2.7 + 1.0;
        assert_eq!(
            secs_of(&chunks, first_speech_ends - 0.27, agreed_at + 0.6).floor(),
            116.0
        );
        // The same after a loop, with one call per chunk.
        let own = [chunks[1]];
        assert_eq!(secs_of(&own, 0.0, 0.6).floor(), 116.0);
    }

    #[test]
    fn a_sentence_after_a_one_word_answer_in_the_same_chunk_gets_its_own_time() {
        // "Right." is shorter than 1 s, so the chunker cannot close its
        // chunk. The chunk goes on over 13 s of silence into the next
        // sentence. Whisper starts that sentence where "Right." ended.
        let chunk = chunks_of(&[(520.6, 539.9)]);
        let speech = chunks_of(&[(520.6, 521.0), (534.4, 539.4)]);
        assert_eq!(secs_with_speech(&chunk, &speech, 0.0, 0.5), 520.6);
        let sentence = secs_with_speech(&chunk, &speech, 0.5, 18.7);
        assert!((sentence - 534.4).abs() < 0.001, "{sentence}");
    }

    #[test]
    fn speech_stretches_leave_out_the_quiet_parts_of_a_chunk() {
        // One chunk: 0.4 s of speech, 13 s of room noise, 5 s of speech,
        // 0.5 s of room noise. A second chunk of one level.
        let mut samples = vec![0.001f32; 30 * S];
        samples[S..S + 4 * S / 10].fill(0.2);
        samples[14 * S..19 * S].fill(0.3);
        // A dip of 0.3 s inside the speech does not split it.
        samples[16 * S..16 * S + 3 * S / 10].fill(0.001);
        samples[25 * S..28 * S].fill(0.2);
        let chunks = [(S, 19 * S + S / 2), (25 * S, 28 * S)];
        assert_eq!(
            speech_stretches(&samples, &chunks),
            vec![(S, S + 4 * S / 10), (14 * S, 19 * S), (25 * S, 28 * S)]
        );
        // A louder or quieter recording has the same stretches.
        let quiet: Vec<f32> = samples.iter().map(|s| s / 50.0).collect();
        assert_eq!(
            speech_stretches(&quiet, &chunks),
            speech_stretches(&samples, &chunks)
        );
    }

    #[test]
    fn a_looping_group_is_decoded_again_chunk_by_chunk() {
        let (samples, bursts) = fixture();
        let plan = DecodePlan::for_source(&samples, 30 * S);
        let looped = plan
            .groups()
            .iter()
            .position(|group| group.chunks.len() > 2)
            .expect("a group with several chunks");
        let group = &plan.groups()[looped];
        let id = bursts
            .iter()
            .find(|b| (b.0 * S as f64) as usize >= group.start())
            .unwrap()
            .2;
        let mut fake = FakeWhisper::looping(&[id]);
        let out = decode_groups(&mut fake, &samples, plan.groups(), 0).unwrap();
        assert_eq!(fake.calls, plan.groups().len() + group.chunks.len());
        // The result is the per-chunk result: no loop, and the same lines.
        let mut clean = FakeWhisper::new();
        let expected = decode_groups(&mut clean, &samples, plan.groups(), 0).unwrap();
        assert_eq!(out.lines, expected.lines);
        assert!(out.lines.iter().all(|l| find_repetition_loop(l).is_none()));
    }

    #[test]
    fn a_retry_that_loops_again_is_kept_and_not_retried_further() {
        let (samples, bursts) = fixture();
        let plan = DecodePlan::for_source(&samples, 30 * S);
        let group = plan
            .groups()
            .iter()
            .find(|group| group.chunks.len() > 2)
            .unwrap();
        let id = bursts
            .iter()
            .find(|b| (b.0 * S as f64) as usize >= group.start())
            .unwrap()
            .2;
        let mut fake = FakeWhisper::looping(&[id]);
        fake.loop_always = true;
        let out = decode_groups(&mut fake, &samples, plan.groups(), 0).unwrap();
        assert_eq!(fake.calls, plan.groups().len() + group.chunks.len());
        let looping: Vec<_> = out
            .lines
            .iter()
            .filter(|l| find_repetition_loop(l).is_some())
            .collect();
        assert_eq!(looping.len(), 1, "{looping:?}");
        assert!(looping[0].contains(&format!("w{id} ")));
    }

    #[test]
    fn legitimate_repetition_and_changed_numbers_are_not_retried() {
        let chunks = [(0, 2 * S), (3 * S, 5 * S)];
        let samples = vec![0.2; 6 * S];
        let texts = [
            "No, no, no, no. That is not what I meant.",
            "We sold 5 units. We sold 6 units. We sold 7 units. We sold 8 units.",
            "It works now. It doesn't work now. It works now. It does not work now.",
        ];
        let mut calls = 0;
        let mut decoder = |_: &[f32]| -> Result<Option<DecodedAudio>, TranscribeError> {
            calls += 1;
            Ok(Some(DecodedAudio {
                segments: texts
                    .iter()
                    .enumerate()
                    .map(|(n, text)| TimedText {
                        start: n as f64,
                        end: n as f64 + 1.0,
                        text: text.to_string(),
                    })
                    .collect(),
                stats: FilterStats::default(),
            }))
        };
        let groups = pack_groups(&chunks, 30 * S);
        let out = decode_groups(&mut decoder, &samples, &groups, 0).unwrap();
        assert_eq!(calls, 1);
        let text: Vec<_> = out.lines.iter().map(|l| l[7..].to_string()).collect();
        assert_eq!(text, texts);
    }

    #[test]
    fn growing_audio_leaves_a_group_without_a_known_end_to_the_final_pass() {
        // 10 s, pause, 5 s, pause, then speech still going at the end. All
        // three fit in one group so far.
        let samples = burst_audio(30.0, &[(0.0, 10.0, 1), (12.0, 5.0, 2), (20.0, 10.0, 3)]);
        let growing = DecodePlan::for_growing_source(&samples, 30 * S);
        // The open chunk may still grow past the target: no cut yet.
        assert_eq!(growing.groups().len(), 1);
        assert!(growing.finished_groups().is_empty());
        assert!(growing.cuts().is_empty());
        // Once the open chunk is too long to fit, the group before it ends.
        let samples = burst_audio(50.0, &[(0.0, 10.0, 1), (12.0, 5.0, 2), (20.0, 30.0, 3)]);
        let growing = DecodePlan::for_growing_source(&samples, 30 * S);
        assert_eq!(growing.finished_groups().len(), 1);
        let closed = growing.groups()[0].end();
        assert_eq!(
            growing.cuts(),
            &[closed + crate::transcribe::VAD_CLOSE_PAUSE_SAMPLES]
        );
        // The complete plan of the same audio has a cut at its end.
        let complete = DecodePlan::for_source(&samples, 30 * S);
        assert_eq!(complete.cuts().last(), Some(&samples.len()));
        assert_eq!(complete.finished_groups().len(), complete.groups().len());
    }

    #[test]
    fn a_span_between_cuts_matches_the_whole_source_plan() {
        let (samples, _) = fixture();
        let plan = DecodePlan::for_source(&samples, 30 * S);
        let cuts: Vec<usize> = std::iter::once(0)
            .chain(plan.cuts().iter().copied())
            .collect();
        assert!(cuts.len() > 4);
        for pair in cuts.windows(2) {
            assert!(
                plan.matches_span(&samples[pair[0]..pair[1]], pair[0], 30 * S),
                "span {pair:?}"
            );
        }
        // A span over several groups matches as well.
        assert!(plan.matches_span(&samples[cuts[1]..cuts[4]], cuts[1], 30 * S));
        // Edges inside a group do not.
        let g = (1..plan.groups().len())
            .find(|&g| plan.groups()[g].chunks.len() > 1)
            .unwrap();
        let inside = plan.groups()[g].chunks[0].1 + S / 10;
        assert!(!plan.matches_span(&samples[cuts[g]..inside], cuts[g], 30 * S));
        assert!(!plan.matches_span(&samples[inside..cuts[g + 1]], inside, 30 * S));
        // Another target gives other groups.
        assert!(!plan.matches_span(&samples[cuts[0]..cuts[4]], 0, 25 * S));
    }

    #[test]
    fn silence_has_no_groups_and_matches_any_silent_span() {
        let samples = vec![0.0; 90 * S];
        let plan = DecodePlan::for_source(&samples, 30 * S);
        assert!(plan.groups().is_empty());
        assert_eq!(plan.cuts(), &[samples.len()]);
        assert!(plan.matches_span(&samples[S..40 * S], S, 30 * S));
    }
}
