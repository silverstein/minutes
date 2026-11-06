//! Transcript checkpoints: meeting transcription done while the meeting is
//! still recording.
//!
//! A host that has the growing capture can transcribe finished spans of each
//! source during the recording with [`transcribe_span`], and queue the job with
//! the spans it has (`jobs::queue_live_capture_with_transcript_checkpoint`).
//! Final processing then reuses those spans and transcribes only the audio
//! that no span covers, usually the last minute or two.
//!
//! A span is reused only when it provably describes the final file:
//!
//! - its PCM digest matches the same samples of the final source file,
//! - its settings fingerprint matches the settings of the final pass
//!   (core version, transcription config, decode hints),
//! - it lies inside the file and does not overlap an earlier accepted span.
//!
//! Anything else is ignored and that range is transcribed as usual. With no
//! checkpoint, or no accepted span, processing is unchanged.
//!
//! Spans use the same VAD chunking, decoder settings and per-source options as
//! the final pass. Each chunk is decoded independently, so a span's lines
//! match what the final pass would produce for those chunks. Lines are kept
//! before the transcript cleanup pipeline, which runs once over the whole
//! source, as it does without a checkpoint.

use crate::config::Config;
use crate::error::TranscribeError;
use crate::transcribe::{self, DecodeHints, TranscribeResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Samples per second of the canonical audio that spans refer to.
pub const SAMPLE_RATE: u64 = 16_000;

const CHECKPOINT_VERSION: u32 = 1;

/// The audio a span was taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointSource {
    /// A single-source meeting recording (the job's own WAV).
    Recording,
    /// The microphone stem (`.voice.wav`) of a dual-source recording.
    Voice,
    /// The call/system stem (`.system.wav`) of a dual-source recording.
    System,
}

/// Transcript lines for samples `[start_sample, end_sample)` of one source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointSpan {
    pub source: CheckpointSource,
    pub start_sample: u64,
    pub end_sample: u64,
    /// Fingerprint of the settings that produced `lines`.
    pub settings: String,
    /// SHA-256 of the span's canonical samples (little-endian `f32`).
    pub pcm_sha256: String,
    /// `[m:ss] text` lines on the source clock, before transcript cleanup.
    pub lines: Vec<String>,
}

/// The spans a host transcribed while recording. Order does not matter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptCheckpoint {
    pub version: u32,
    pub spans: Vec<CheckpointSpan>,
}

impl Default for TranscriptCheckpoint {
    fn default() -> Self {
        Self {
            version: CHECKPOINT_VERSION,
            spans: Vec::new(),
        }
    }
}

impl TranscriptCheckpoint {
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Spans for `source`, sorted by start.
    pub fn spans_for(&self, source: CheckpointSource) -> Vec<&CheckpointSpan> {
        if self.version != CHECKPOINT_VERSION {
            return Vec::new();
        }
        let mut spans: Vec<_> = self.spans.iter().filter(|s| s.source == source).collect();
        spans.sort_by_key(|s| (s.start_sample, s.end_sample));
        spans
    }
}

/// The decode hints a queued meeting job uses when no calendar event matches
/// it. Pass them to [`transcribe_span`] so spans fit the final pass. If the
/// job's hints turn out different (another title, a calendar match), its
/// spans are ignored, not misused.
pub fn decode_hints_for_queued_meeting(
    title: Option<&str>,
    pre_context: Option<&str>,
    config: &Config,
) -> DecodeHints {
    crate::pipeline::queued_meeting_decode_hints(title, pre_context, config)
}

/// Transcribe one span of a source during the recording.
///
/// `samples` are the source's canonical 16 kHz mono samples starting at
/// `start_sample` (a 16-bit WAV sample `s` is `s as f32 / 32768.0`). Cut a span
/// in a pause, so no word is split between two spans. `hints` are the job's
/// decode hints, normally from [`decode_hints_for_queued_meeting`].
pub fn transcribe_span(
    samples: &[f32],
    start_sample: u64,
    source: CheckpointSource,
    config: &Config,
    hints: &DecodeHints,
) -> Result<CheckpointSpan, TranscribeError> {
    if samples.is_empty() {
        return Err(TranscribeError::EmptyAudio);
    }
    let (config, hints) = source_inputs(source, config, hints);
    if transcribe::should_bypass_meeting_chunk_rotation(
        &config,
        crate::pipeline::private_audio_processing_available(),
    ) {
        return Err(TranscribeError::TranscriptionFailed(
            "checkpoint spans need meeting chunk rotation, which this engine cannot use here"
                .into(),
        ));
    }
    let chunks = transcribe::detect_meeting_vad_chunks(samples);
    let clock_offset = usize::try_from(start_sample).map_err(|_| {
        TranscribeError::UnsupportedFormat("span starts past the audio budget".into())
    })?;
    let lines =
        transcribe::transcribe_chunk_lines(samples, &chunks, clock_offset, &config, &hints)?.lines;
    Ok(CheckpointSpan {
        source,
        start_sample,
        end_sample: start_sample + samples.len() as u64,
        settings: settings_fingerprint(&config, &hints),
        pcm_sha256: pcm_digest(samples),
        lines,
    })
}

/// The config and hints the final pass uses for `source`. Independent stems
/// keep the shared clock and have no per-source word minimum.
pub(crate) fn source_inputs(
    source: CheckpointSource,
    config: &Config,
    hints: &DecodeHints,
) -> (Config, DecodeHints) {
    match source {
        CheckpointSource::Recording => (config.clone(), hints.clone()),
        CheckpointSource::Voice | CheckpointSource::System => {
            let mut config = config.clone();
            config.transcription.min_words = 0;
            let hints = hints
                .clone()
                .with_audio_format_extension("wav")
                .with_source_timeline();
            (config, hints)
        }
    }
}

/// Final-pass meeting transcription that reuses accepted `spans` of this
/// source. `config` and `hints` are the ones the final pass would use for the
/// source (see [`source_inputs`]).
pub(crate) fn transcribe_meeting_with_spans(
    audio_path: &Path,
    config: &Config,
    hints: &DecodeHints,
    spans: &[&CheckpointSpan],
) -> Result<TranscribeResult, TranscribeError> {
    transcribe::ensure_ambient_audio_path(audio_path)?;
    let samples = transcribe::load_audio_samples_with_format(
        audio_path,
        hints.audio_format_extension(),
        config,
    )?;
    let bypass = transcribe::should_bypass_meeting_chunk_rotation(
        config,
        crate::pipeline::private_audio_processing_available(),
    );
    let accepted = if bypass {
        Vec::new()
    } else {
        accepted_spans(&samples, spans, &settings_fingerprint(config, hints))
    };
    tracing::info!(
        offered = spans.len(),
        accepted = accepted.len(),
        covered_secs = accepted
            .iter()
            .map(|s| (s.end_sample - s.start_sample) as f64 / SAMPLE_RATE as f64)
            .sum::<f64>(),
        audio_secs = samples.len() as f64 / SAMPLE_RATE as f64,
        "meeting transcription with checkpoint spans"
    );
    if accepted.is_empty() {
        return transcribe::transcribe_meeting_samples(audio_path, &samples, config, hints);
    }

    let mut out = transcribe::ChunkLines {
        lines: Vec::new(),
        stats: transcribe::FilterStats {
            audio_duration_secs: samples.len() as f64 / SAMPLE_RATE as f64,
            ..Default::default()
        },
    };
    let mut cursor = 0usize;
    for span in accepted.iter().map(Some).chain([None]) {
        let gap_end = span.map_or(samples.len(), |s| s.start_sample as usize);
        if gap_end > cursor {
            let chunks: Vec<(usize, usize)> =
                transcribe::detect_meeting_vad_chunks(&samples[cursor..gap_end])
                    .into_iter()
                    .map(|(start, end)| (start + cursor, end + cursor))
                    .collect();
            let gap = transcribe::transcribe_chunk_lines(&samples, &chunks, 0, config, hints)?;
            out.add_stats(&gap.stats);
            out.lines.extend(gap.lines);
        }
        if let Some(span) = span {
            out.lines.extend(span.lines.iter().cloned());
            cursor = span.end_sample as usize;
        }
    }

    match transcribe::finish_chunk_lines(out) {
        Some(result) => Ok(result),
        // No text anywhere: let the ordinary path decide what that means.
        None => transcribe::transcribe_meeting_samples(audio_path, &samples, config, hints),
    }
}

/// Spans that fit `samples` exactly, in order and without overlap.
fn accepted_spans<'a>(
    samples: &[f32],
    spans: &[&'a CheckpointSpan],
    settings: &str,
) -> Vec<&'a CheckpointSpan> {
    let mut sorted = spans.to_vec();
    sorted.sort_by_key(|s| (s.start_sample, s.end_sample));
    let mut accepted = Vec::new();
    let mut covered_until = 0u64;
    for span in sorted {
        let reason = if span.start_sample >= span.end_sample {
            Some("empty range")
        } else if span.start_sample < covered_until {
            Some("overlaps an earlier span")
        } else if span.end_sample > samples.len() as u64 {
            Some("ends after the audio")
        } else if span.settings != settings {
            Some("different transcription settings")
        } else if span.pcm_sha256
            != pcm_digest(&samples[span.start_sample as usize..span.end_sample as usize])
        {
            Some("audio differs")
        } else {
            None
        };
        match reason {
            Some(reason) => tracing::warn!(
                source = ?span.source,
                start_sample = span.start_sample,
                end_sample = span.end_sample,
                reason,
                "ignoring checkpoint span; that range is transcribed again"
            ),
            None => {
                covered_until = span.end_sample;
                accepted.push(span);
            }
        }
    }
    accepted
}

fn settings_fingerprint(config: &Config, hints: &DecodeHints) -> String {
    let mut hasher = Sha256::new();
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update([0]);
    hasher.update(transcribe::effective_batch_engine(config).as_bytes());
    hasher.update([0]);
    hasher.update(format!("{:?}", config.transcription).as_bytes());
    hasher.update([0]);
    hasher.update(format!("{:?}", hints.without_audio_format_extension()).as_bytes());
    hex(&hasher.finalize())
}

fn pcm_digest(samples: &[f32]) -> String {
    let mut hasher = Sha256::new();
    for chunk in samples.chunks(8192) {
        let bytes: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
        hasher.update(&bytes);
    }
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(
        source: CheckpointSource,
        start: u64,
        samples: &[f32],
        settings: &str,
    ) -> CheckpointSpan {
        CheckpointSpan {
            source,
            start_sample: start,
            end_sample: start + samples.len() as u64,
            settings: settings.into(),
            pcm_sha256: pcm_digest(samples),
            lines: vec![format!("[0:{:02}] span at {start}", start / SAMPLE_RATE)],
        }
    }

    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|i| (i % 97) as f32 / 97.0 - 0.5).collect()
    }

    #[test]
    fn matching_spans_are_accepted_in_order() {
        let audio = ramp(48_000);
        let a = span(CheckpointSource::Voice, 0, &audio[..16_000], "s");
        let b = span(CheckpointSource::Voice, 16_000, &audio[16_000..32_000], "s");
        let accepted = accepted_spans(&audio, &[&b, &a], "s");
        assert_eq!(accepted, vec![&a, &b]);
    }

    #[test]
    fn spans_that_do_not_fit_the_final_audio_are_ignored() {
        let audio = ramp(48_000);
        let good = span(CheckpointSource::Voice, 0, &audio[..16_000], "s");
        let other_settings = span(CheckpointSource::Voice, 16_000, &audio[16_000..32_000], "t");
        let mut changed = audio[32_000..48_000].to_vec();
        changed[5] += 0.25;
        let other_audio = span(CheckpointSource::Voice, 32_000, &changed, "s");
        let past_end = span(CheckpointSource::Voice, 40_000, &ramp(16_000), "s");
        let overlap = span(CheckpointSource::Voice, 8_000, &audio[8_000..24_000], "s");
        let accepted = accepted_spans(
            &audio,
            &[&good, &other_settings, &other_audio, &past_end, &overlap],
            "s",
        );
        assert_eq!(accepted, vec![&good]);
    }

    #[test]
    fn spans_for_filters_by_source_and_sorts() {
        let audio = ramp(32_000);
        let checkpoint = TranscriptCheckpoint {
            spans: vec![
                span(CheckpointSource::System, 16_000, &audio[16_000..], "s"),
                span(CheckpointSource::Voice, 0, &audio[..16_000], "s"),
                span(CheckpointSource::System, 0, &audio[..16_000], "s"),
            ],
            ..Default::default()
        };
        let system = checkpoint.spans_for(CheckpointSource::System);
        assert_eq!(system.len(), 2);
        assert_eq!(system[0].start_sample, 0);
        assert_eq!(checkpoint.spans_for(CheckpointSource::Recording).len(), 0);
    }

    #[test]
    fn unknown_checkpoint_versions_offer_no_spans() {
        let audio = ramp(16_000);
        let checkpoint = TranscriptCheckpoint {
            version: CHECKPOINT_VERSION + 1,
            spans: vec![span(CheckpointSource::Voice, 0, &audio, "s")],
        };
        assert!(checkpoint.spans_for(CheckpointSource::Voice).is_empty());
    }

    #[test]
    fn checkpoint_round_trips_through_json() {
        let audio = ramp(16_000);
        let checkpoint = TranscriptCheckpoint {
            spans: vec![span(CheckpointSource::Recording, 0, &audio, "s")],
            ..Default::default()
        };
        let json = serde_json::to_string(&checkpoint).unwrap();
        assert!(json.contains("\"source\":\"recording\""));
        let back: TranscriptCheckpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(back, checkpoint);
    }

    #[test]
    fn stems_use_the_shared_clock_and_no_word_minimum() {
        let mut config = Config::default();
        config.transcription.min_words = 3;
        let hints = DecodeHints::default();
        let (voice_config, voice_hints) = source_inputs(CheckpointSource::Voice, &config, &hints);
        assert_eq!(voice_config.transcription.min_words, 0);
        assert_eq!(
            voice_hints,
            hints
                .clone()
                .with_audio_format_extension("wav")
                .with_source_timeline()
        );
        let (recording_config, recording_hints) =
            source_inputs(CheckpointSource::Recording, &config, &hints);
        assert_eq!(recording_config.transcription.min_words, 3);
        assert_eq!(recording_hints, hints);
    }

    #[test]
    fn settings_fingerprint_follows_model_and_hints_but_not_format() {
        let config = Config::default();
        let hints = DecodeHints::from_candidates(&["Priya".into()], &[]);
        let base = settings_fingerprint(&config, &hints);
        assert_eq!(
            base,
            settings_fingerprint(&config, &hints.clone().with_audio_format_extension("wav"))
        );
        let mut other_model = config.clone();
        other_model.transcription.model = "tiny".into();
        assert_ne!(base, settings_fingerprint(&other_model, &hints));
        assert_ne!(base, settings_fingerprint(&config, &DecodeHints::default()));
    }

    fn write_wav(path: &Path, samples: &[i16]) -> Vec<f32> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for &sample in samples {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        samples.iter().map(|&s| s as f32 / 32768.0).collect()
    }

    /// A config whose fully covered sources never reach a decoder, so no model
    /// is needed. `None` where chunk rotation is bypassed (no private audio),
    /// because spans are then never used.
    fn speechless_config() -> Option<Config> {
        let mut config = Config::default();
        config.transcription.engine = "whisper".into();
        let bypass = transcribe::should_bypass_meeting_chunk_rotation(
            &config,
            crate::pipeline::private_audio_processing_available(),
        );
        (!bypass).then_some(config)
    }

    #[test]
    fn a_fully_covered_source_reuses_its_spans_without_decoding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meeting.voice.wav");
        let pcm: Vec<i16> = (0..48_000).map(|i| ((i % 400) as i16 - 200) * 40).collect();
        let samples = write_wav(&path, &pcm);
        let Some(config) = speechless_config() else {
            return;
        };
        let hints = DecodeHints::from_candidates(&["Priya".into()], &[]);
        let (config, hints) = source_inputs(CheckpointSource::Voice, &config, &hints);
        let settings = settings_fingerprint(&config, &hints);
        let mut first = span(CheckpointSource::Voice, 0, &samples[..24_000], &settings);
        first.lines = vec!["[0:00] first piece".into()];
        let mut second = span(
            CheckpointSource::Voice,
            24_000,
            &samples[24_000..],
            &settings,
        );
        second.lines = vec!["[0:01] second piece".into()];

        let result =
            transcribe_meeting_with_spans(&path, &config, &hints, &[&second, &first]).unwrap();

        assert_eq!(result.text, "[0:00] first piece\n[0:01] second piece\n");
        assert_eq!(result.stats.audio_duration_secs, 3.0);
        assert_eq!(result.stats.final_words, 6);
    }

    #[test]
    fn checkpoint_lines_go_through_the_same_cleanup_as_the_batch_pass() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meeting.wav");
        let pcm: Vec<i16> = (0..32_000).map(|i| ((i % 300) as i16 - 150) * 50).collect();
        let samples = write_wav(&path, &pcm);
        let Some(config) = speechless_config() else {
            return;
        };
        let hints = DecodeHints::default();
        let settings = settings_fingerprint(&config, &hints);
        // A repetition loop split across two spans is collapsed as one run.
        let mut first = span(
            CheckpointSource::Recording,
            0,
            &samples[..16_000],
            &settings,
        );
        first.lines = vec![
            "[0:00] thank you".into(),
            "[0:00] thank you".into(),
            "[0:00] thank you".into(),
        ];
        let mut second = span(
            CheckpointSource::Recording,
            16_000,
            &samples[16_000..],
            &settings,
        );
        second.lines = vec!["[0:01] thank you".into(), "[0:01] next topic".into()];
        let spans = [&first, &second];

        let result = transcribe_meeting_with_spans(&path, &config, &hints, &spans).unwrap();

        let all: Vec<String> = first.lines.iter().chain(&second.lines).cloned().collect();
        let expected = transcribe::finish_chunk_lines(transcribe::ChunkLines {
            lines: all,
            stats: Default::default(),
        })
        .unwrap();
        assert_eq!(result.text, expected.text);
        assert!(
            result.text.matches("thank you").count() < 4,
            "{}",
            result.text
        );
    }

    #[test]
    fn empty_span_audio_is_an_error() {
        let config = Config::default();
        let error = transcribe_span(
            &[],
            0,
            CheckpointSource::Voice,
            &config,
            &DecodeHints::default(),
        )
        .unwrap_err();
        assert!(matches!(error, TranscribeError::EmptyAudio));
    }
}
