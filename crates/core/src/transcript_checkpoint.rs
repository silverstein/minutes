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
//!   (core version, transcription config, decode strategy, decode hints),
//! - it lies inside the file and does not overlap an earlier accepted span,
//! - its decode groups are exactly the final pass's groups for that audio, so
//!   no group crosses either edge of the span.
//!
//! Anything else is ignored and that range is transcribed as usual. With no
//! checkpoint, or no accepted span, processing is unchanged.
//!
//! Spans use the same decode plan ([`crate::decode_groups`]), decoder,
//! settings and per-source options as the final pass. Each group is decoded
//! independently, so a span's lines match what the final pass would produce
//! for those groups. The final pass decodes only the groups that no accepted
//! span covers. Lines are kept before the transcript cleanup pipeline, which
//! runs once over the whole source, as it does without a checkpoint.

use crate::config::Config;
use crate::decode_groups::{DecodeGroup, DecodePlan};
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

/// Load a checkpoint file. `None` when the file is missing, unreadable,
/// corrupt or of another version: the job then transcribes as if it had no
/// checkpoint.
pub fn load_checkpoint_file(path: &Path) -> Option<TranscriptCheckpoint> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(path = %path.display(), error = %error,
                "ignoring an unreadable transcript checkpoint; the job transcribes in full");
            return None;
        }
    };
    match serde_json::from_slice::<TranscriptCheckpoint>(&bytes) {
        Ok(checkpoint) if checkpoint.version == CHECKPOINT_VERSION => Some(checkpoint),
        Ok(checkpoint) => {
            tracing::warn!(path = %path.display(), version = checkpoint.version,
                "ignoring a transcript checkpoint of another version; the job transcribes in full");
            None
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), error = %error,
                "ignoring a corrupt transcript checkpoint; the job transcribes in full");
            None
        }
    }
}

/// Replace a checkpoint file with `checkpoint`. The new content goes to a
/// temporary file beside it, which is owner-only from the start (0600 on
/// Unix). The temporary file is written and synced, then renamed over the
/// checkpoint in one step. If any step fails, the previous file stays as it
/// was, and a reader never sees a partial file.
pub fn save_checkpoint_file(path: &Path, checkpoint: &TranscriptCheckpoint) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(checkpoint).map_err(std::io::Error::other)?;
    replace_file_with(path, &bytes, |temporary, path| {
        temporary
            .persist(path)
            .map(drop)
            .map_err(|error| error.error)
    })
}

/// [`save_checkpoint_file`] with the last step, the rename, given by the
/// caller, so tests can make it fail.
fn replace_file_with(
    path: &Path,
    bytes: &[u8],
    persist: impl FnOnce(tempfile::NamedTempFile, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    // tempfile creates the file with O_EXCL and mode 0600 on Unix. On
    // Windows, persist replaces the target with MoveFileExW and
    // MOVEFILE_REPLACE_EXISTING.
    let mut temporary = tempfile::Builder::new()
        .prefix(&temporary_prefix(path))
        .suffix(TEMPORARY_SUFFIX)
        .tempfile_in(dir)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    persist(temporary, path)
}

const TEMPORARY_SUFFIX: &str = ".tmp";

fn temporary_prefix(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    format!("{name}.")
}

/// Remove a checkpoint file and any temporary file that a crash left beside
/// it. The spans hold transcript text, so nothing of them should stay.
pub fn remove_checkpoint_file(path: &Path) {
    std::fs::remove_file(path).ok();
    let (Some(dir), Some(_)) = (path.parent(), path.file_name()) else {
        return;
    };
    let prefix = temporary_prefix(path);
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(TEMPORARY_SUFFIX) {
            std::fs::remove_file(entry.path()).ok();
        }
    }
}

/// A job's checkpoint file while the final pass adds the spans it finishes.
/// It keeps the spans in memory and replaces the file after each change, so a
/// failed save is written again with the next one.
pub(crate) struct CheckpointFile {
    path: std::path::PathBuf,
    checkpoint: TranscriptCheckpoint,
}

impl CheckpointFile {
    /// The file at `path` with the spans it holds now. A missing, corrupt or
    /// unsupported file starts empty, and the first save replaces it.
    pub(crate) fn open(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            checkpoint: load_checkpoint_file(path).unwrap_or_default(),
        }
    }

    pub(crate) fn checkpoint(&self) -> &TranscriptCheckpoint {
        &self.checkpoint
    }

    /// Add `span` and save. Spans of the same source that overlap it are
    /// dropped: the final pass writes a span only for audio that no accepted
    /// span covers, so what it overlaps was rejected. Spans of the other
    /// source are kept, because each source has its own clock.
    pub(crate) fn add(&mut self, span: CheckpointSpan) -> std::io::Result<()> {
        self.checkpoint.spans.retain(|old| {
            old.source != span.source
                || old.end_sample <= span.start_sample
                || old.start_sample >= span.end_sample
        });
        self.checkpoint.spans.push(span);
        save_checkpoint_file(&self.path, &self.checkpoint)
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

/// Decode hints using the same recording-date calendar lookup as final
/// processing. Run on the optional worker, never on the capture or Stop path.
/// If a title, context, vocabulary, or calendar changes later, the final pass
/// rejects the old spans and transcribes their audio again; never re-key text.
pub fn decode_hints_for_queued_meeting_at(
    title: Option<&str>,
    pre_context: Option<&str>,
    recorded_at: chrono::DateTime<chrono::Local>,
    config: &Config,
) -> DecodeHints {
    crate::pipeline::queued_meeting_decode_hints_at(title, pre_context, recorded_at, config)
}

/// Transcribe one span of a source during the recording.
///
/// `samples` are the source's canonical 16 kHz mono samples starting at
/// `start_sample` (a 16-bit WAV sample `s` is `s as f32 / 32768.0`). Start and
/// end a span only at a cut of the source's decode plan
/// ([`DecodePlan::cuts`] of [`DecodePlan::for_growing_source`]). The final
/// pass rejects a span whose groups differ from its own. `hints` are the job's
/// decode hints, normally from [`decode_hints_for_queued_meeting`].
pub fn transcribe_span(
    samples: &[f32],
    start_sample: u64,
    source: CheckpointSource,
    config: &Config,
    hints: &DecodeHints,
) -> Result<CheckpointSpan, TranscribeError> {
    SpanTranscriber::new(config, hints).transcribe_span(samples, start_sample, source)
}

/// A meeting-scoped decoder with frozen settings and hints. A single Whisper
/// context is loaded lazily and shared across all spans of both source stems.
/// Own this on the optional worker thread; dropping it releases the model.
pub struct SpanTranscriber {
    config: Config,
    hints: DecodeHints,
    decoder: transcribe::MeetingGroupDecoder,
}

impl SpanTranscriber {
    pub fn new(config: &Config, hints: &DecodeHints) -> Self {
        Self {
            config: config.clone(),
            hints: hints.clone(),
            decoder: Default::default(),
        }
    }

    pub fn transcribe_span(
        &mut self,
        samples: &[f32],
        start_sample: u64,
        source: CheckpointSource,
    ) -> Result<CheckpointSpan, TranscribeError> {
        if samples.is_empty() {
            return Err(TranscribeError::EmptyAudio);
        }
        let (config, hints) = source_inputs(source, &self.config, &self.hints);
        if transcribe::should_bypass_meeting_chunk_rotation(
            &config,
            crate::pipeline::private_audio_processing_available(),
        ) {
            return Err(TranscribeError::TranscriptionFailed(
                "checkpoint spans need meeting chunk rotation, which this engine cannot use here"
                    .into(),
            ));
        }
        let plan = DecodePlan::for_source(samples, transcribe::meeting_group_target(&config));
        let clock_offset = usize::try_from(start_sample).map_err(|_| {
            TranscribeError::UnsupportedFormat("span starts past the audio budget".into())
        })?;
        let had_model = self.decoder.model_loaded();
        let lines = self
            .decoder
            .transcribe(samples, plan.groups(), clock_offset, &config, &hints)?
            .lines;
        if !had_model && self.decoder.model_loaded() {
            tracing::info!("checkpoint whisper context loaded; reusing for remaining spans");
            crate::logging::log_step(
                "capture_checkpoint_model_load",
                "",
                0,
                serde_json::json!({"engine":"whisper"}),
            );
        }
        Ok(CheckpointSpan {
            source,
            start_sample,
            end_sample: start_sample + samples.len() as u64,
            settings: settings_fingerprint(&config, &hints),
            pcm_sha256: pcm_digest(samples),
            lines,
        })
    }
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

/// Save finished final-pass work about every this much source audio.
const SAVE_EVERY_SAMPLES: usize = 120 * SAMPLE_RATE as usize;

/// Final-pass meeting transcription of one source that reuses accepted
/// `spans` of it. `config` and `hints` are the ones the final pass would use
/// for the source (see [`source_inputs`]).
///
/// With `progress`, finished groups are saved there as spans of this source,
/// about every two minutes of audio, and the rest of the source when its last
/// group is done. A retry then reuses them like spans from the recording. An
/// interrupted group is not saved. A failed save is logged and does not stop
/// the transcription.
pub(crate) fn transcribe_meeting_with_spans(
    audio_path: &Path,
    source: CheckpointSource,
    config: &Config,
    hints: &DecodeHints,
    spans: &[&CheckpointSpan],
    progress: Option<&mut CheckpointFile>,
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
    let pass = SourcePass::new(
        &samples,
        source,
        settings_fingerprint(config, hints),
        transcribe::meeting_group_target(config),
    );
    let accepted = if bypass {
        Vec::new()
    } else {
        pass.accept(spans)
    };
    let covered_secs = accepted
        .iter()
        .map(|s| (s.end_sample - s.start_sample) as f64 / SAMPLE_RATE as f64)
        .sum::<f64>();
    let audio_secs = samples.len() as f64 / SAMPLE_RATE as f64;
    tracing::info!(
        ?source,
        offered = spans.len(),
        accepted = accepted.len(),
        covered_secs,
        audio_secs,
        "meeting transcription with checkpoint spans"
    );
    if !spans.is_empty() {
        crate::logging::log_step(
            "transcript_checkpoint_reuse",
            "",
            0,
            serde_json::json!({
                "source": source,
                "offered": spans.len(), "accepted": accepted.len(),
                "rejected": spans.len().saturating_sub(accepted.len()),
                "covered_secs": covered_secs, "audio_secs": audio_secs,
            }),
        );
    }
    // Spans are cut from decode groups. Where the meeting path does not
    // decode groups, there is nothing to save.
    let chunked = !bypass && pass.plan.chunk_count() >= transcribe::MIN_MEETING_CHUNKS;
    let progress = progress.filter(|_| chunked);
    if accepted.is_empty() && progress.is_none() {
        return transcribe::transcribe_meeting_samples(audio_path, &samples, config, hints);
    }

    let mut decoder = transcribe::MeetingGroupDecoder::default();
    let out = pass.assemble(&accepted, progress, |groups, finished| {
        decoder.transcribe_reporting(&samples, groups, 0, config, hints, finished)
    })?;

    match transcribe::finish_chunk_lines(out) {
        Some(result) => Ok(result),
        // No text anywhere, so decoding the groups again gives none either.
        // Let the whole-file path decide what that means, as the meeting
        // path does.
        None => transcribe::transcribe_dispatch(audio_path, config, hints),
    }
}

/// The final pass over one source after its audio is loaded.
struct SourcePass<'a> {
    samples: &'a [f32],
    source: CheckpointSource,
    /// The settings fingerprint of this pass, for spans it accepts and saves.
    settings: String,
    group_target: usize,
    /// The whole-source plan, made once.
    plan: DecodePlan,
}

impl<'a> SourcePass<'a> {
    fn new(
        samples: &'a [f32],
        source: CheckpointSource,
        settings: String,
        group_target: usize,
    ) -> Self {
        Self {
            plan: DecodePlan::for_source(samples, group_target),
            samples,
            source,
            settings,
            group_target,
        }
    }

    /// The spans of `spans` that this pass can reuse.
    fn accept<'s>(&self, spans: &[&'s CheckpointSpan]) -> Vec<&'s CheckpointSpan> {
        accepted_spans(
            self.samples,
            spans,
            &self.settings,
            &self.plan,
            self.group_target,
        )
    }

    /// The lines of the whole source, before cleanup. `accepted` comes from
    /// [`accept`](Self::accept). `decode` decodes groups of the source and
    /// reports each finished group. With `progress`, finished groups are saved
    /// as spans.
    fn assemble(
        &self,
        accepted: &[&CheckpointSpan],
        mut progress: Option<&mut CheckpointFile>,
        decode: impl FnMut(
            &[DecodeGroup],
            &mut dyn FnMut(usize, &transcribe::ChunkLines),
        ) -> Result<transcribe::ChunkLines, TranscribeError>,
    ) -> Result<transcribe::ChunkLines, TranscribeError> {
        let saving = progress.is_some();
        let (mut saved, mut failed, mut saved_samples) = (0usize, 0usize, 0usize);
        let out = assemble_source(
            self.samples,
            &self.plan,
            accepted,
            decode,
            |range, lines| {
                let Some(file) = progress.as_mut() else {
                    return;
                };
                let span = CheckpointSpan {
                    source: self.source,
                    start_sample: range.start as u64,
                    end_sample: range.end as u64,
                    settings: self.settings.clone(),
                    pcm_sha256: pcm_digest(&self.samples[range.clone()]),
                    lines,
                };
                match file.add(span) {
                    Ok(()) => {
                        saved += 1;
                        saved_samples += range.len();
                    }
                    Err(error) => {
                        failed += 1;
                        tracing::warn!(source = ?self.source, start_sample = range.start,
                        end_sample = range.end, error = %error,
                        "could not save finished transcription; a retry decodes it again");
                    }
                }
            },
        );
        if saving && (saved > 0 || failed > 0) {
            crate::logging::log_step(
                "transcript_checkpoint_saved",
                "",
                0,
                serde_json::json!({
                    "source": self.source, "saved": saved, "failed": failed,
                    "saved_secs": saved_samples as f64 / SAMPLE_RATE as f64,
                    "complete": out.is_ok(),
                }),
            );
        }
        out
    }
}

/// The lines of a whole source, before cleanup: the lines of each accepted
/// span, and `decode` for each run of groups between them. `accepted` must
/// come from [`accepted_spans`] with the same `plan`, so every group is
/// either inside one span or in a gap.
///
/// As groups finish, `finished(range, lines)` gets runs of them as spans:
/// about [`SAVE_EVERY_SAMPLES`] each, and the rest when the last group of a
/// gap is done. A span starts where the chunker resets before its first
/// group and ends where it resets after its last, so on a retry it has the
/// whole-source groups. It never reaches into an accepted span.
fn assemble_source(
    samples: &[f32],
    plan: &DecodePlan,
    accepted: &[&CheckpointSpan],
    mut decode: impl FnMut(
        &[DecodeGroup],
        &mut dyn FnMut(usize, &transcribe::ChunkLines),
    ) -> Result<transcribe::ChunkLines, TranscribeError>,
    mut finished: impl FnMut(std::ops::Range<usize>, Vec<String>),
) -> Result<transcribe::ChunkLines, TranscribeError> {
    let mut out = transcribe::ChunkLines {
        lines: Vec::new(),
        stats: transcribe::FilterStats {
            audio_duration_secs: samples.len() as f64 / SAMPLE_RATE as f64,
            ..Default::default()
        },
    };
    let groups = plan.groups();
    let cuts = plan.cuts();
    let mut next = 0usize;
    // The end of the last accepted span.
    let mut covered = 0usize;
    for span in accepted.iter().map(Some).chain([None]) {
        let gap_end = span.map_or(samples.len(), |s| s.start_sample as usize);
        let first = next;
        while next < groups.len() && groups[next].end() <= gap_end {
            next += 1;
        }
        if next > first {
            let last = next - 1;
            let mut from = if first == 0 { 0 } else { cuts[first - 1] }.max(covered);
            let mut lines = Vec::new();
            let gap = decode(&groups[first..next], &mut |index, group| {
                let g = first + index;
                lines.extend(group.lines.iter().cloned());
                let to = cuts[g].min(gap_end);
                if to.saturating_sub(from) >= SAVE_EVERY_SAMPLES || g == last {
                    finished(from..to, std::mem::take(&mut lines));
                    from = to;
                }
            })?;
            out.add_stats(&gap.stats);
            out.lines.extend(gap.lines);
        }
        if let Some(span) = span {
            out.lines.extend(span.lines.iter().cloned());
            covered = span.end_sample as usize;
            while next < groups.len() && groups[next].start() < span.end_sample as usize {
                next += 1;
            }
        }
    }
    Ok(out)
}

/// Spans that fit `samples` exactly, in order and without overlap, and whose
/// decode groups are the groups of `plan` for that audio.
fn accepted_spans<'a>(
    samples: &[f32],
    spans: &[&'a CheckpointSpan],
    settings: &str,
    plan: &DecodePlan,
    group_target: usize,
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
            Some("different transcription settings or decode hints (title/context/calendar/vocabulary)")
        } else if span.pcm_sha256
            != pcm_digest(&samples[span.start_sample as usize..span.end_sample as usize])
        {
            Some("audio differs")
        } else if !plan.matches_span(
            &samples[span.start_sample as usize..span.end_sample as usize],
            span.start_sample as usize,
            group_target,
        ) {
            Some("span edges are not decode-group boundaries")
        } else {
            None
        };
        match reason {
            Some(reason) => {
                tracing::warn!(source = ?span.source, start_sample = span.start_sample,
                    end_sample = span.end_sample, reason,
                    "ignoring checkpoint span; that range is transcribed again");
                crate::logging::log_step(
                    "transcript_checkpoint_rejected",
                    "",
                    0,
                    serde_json::json!({"source":span.source,"start_sample":span.start_sample,
                        "end_sample":span.end_sample,"reason":reason}),
                );
            }
            None => {
                covered_until = span.end_sample;
                accepted.push(span);
            }
        }
    }
    accepted
}

fn settings_fingerprint(config: &Config, hints: &DecodeHints) -> String {
    fingerprint_with_strategy(
        config,
        hints,
        &crate::decode_groups::strategy_id(transcribe::meeting_group_target(config)),
    )
}

/// `strategy` names how audio is cut into decoder calls and when a call is
/// retried. Spans from another strategy have other lines at the same
/// settings, so they must not match.
fn fingerprint_with_strategy(config: &Config, hints: &DecodeHints, strategy: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update([0]);
    hasher.update(transcribe::effective_batch_engine(config).as_bytes());
    hasher.update([0]);
    hasher.update(strategy.as_bytes());
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

    const GROUP: usize = crate::decode_groups::WHISPER_GROUP_TARGET_SAMPLES;
    const RATE: usize = SAMPLE_RATE as usize;

    /// Speech bursts of 1.2 to 9 s with pauses of 0.4 to 7 s, `secs` long.
    fn bursts(secs: f64) -> Vec<f32> {
        bursts_from(secs, 0)
    }

    /// [`bursts`] whose words are `w<first_id>` and on.
    fn bursts_from(secs: f64, first_id: u32) -> Vec<f32> {
        let mut bursts = Vec::new();
        let mut t = 0.7;
        let mut id = 0u32;
        while t < secs - 12.0 {
            let len = [1.2, 4.0, 9.0, 2.5, 6.3][id as usize % 5];
            bursts.push((t, len, (first_id + id) % 300));
            t += len + [0.4, 1.5, 7.0, 0.9, 2.2, 3.1][id as usize % 6];
            id += 1;
        }
        crate::decode_groups::fake::burst_audio(secs, &bursts)
    }

    /// The whole-source plan and its cuts, with 0 first.
    fn plan_and_cuts(samples: &[f32], target: usize) -> (DecodePlan, Vec<usize>) {
        let plan = DecodePlan::for_source(samples, target);
        let cuts = std::iter::once(0)
            .chain(plan.cuts().iter().copied())
            .collect();
        (plan, cuts)
    }

    fn voice(audio: &[f32], from: usize, to: usize, settings: &str) -> CheckpointSpan {
        span(
            CheckpointSource::Voice,
            from as u64,
            &audio[from..to],
            settings,
        )
    }

    #[test]
    fn matching_spans_are_accepted_in_order() {
        let audio = bursts(90.0);
        let (plan, c) = plan_and_cuts(&audio, GROUP);
        let a = voice(&audio, c[0], c[1], "s");
        let b = voice(&audio, c[1], c[3], "s");
        let accepted = accepted_spans(&audio, &[&b, &a], "s", &plan, GROUP);
        assert_eq!(accepted, vec![&a, &b]);
    }

    #[test]
    fn spans_that_do_not_fit_the_final_audio_are_ignored() {
        let audio = bursts(240.0);
        let (plan, c) = plan_and_cuts(&audio, GROUP);
        assert!(c.len() > 6, "{c:?}");
        let good = voice(&audio, c[0], c[1], "s");
        let other_settings = voice(&audio, c[1], c[2], "t");
        let mut changed = audio[c[2]..c[3]].to_vec();
        changed[5] += 0.25;
        let other_audio = span(CheckpointSource::Voice, c[2] as u64, &changed, "s");
        let past_end = span(
            CheckpointSource::Voice,
            (audio.len() - RATE) as u64,
            &ramp(2 * RATE),
            "s",
        );
        let overlap = voice(&audio, c[1] - RATE / 2, c[2], "s");
        // Ends inside a group that has more than one chunk.
        let g = (3..plan.groups().len())
            .find(|&g| plan.groups()[g].chunks.len() > 1)
            .unwrap();
        let inside = voice(&audio, c[g], plan.groups()[g].chunks[0].1 + RATE / 10, "s");
        let accepted = accepted_spans(
            &audio,
            &[
                &good,
                &other_settings,
                &other_audio,
                &past_end,
                &overlap,
                &inside,
            ],
            "s",
            &plan,
            GROUP,
        );
        assert_eq!(accepted, vec![&good]);
        // The same span is accepted when another plan has a group edge there.
        let per_chunk = DecodePlan::for_source(&audio, 0);
        let edge = per_chunk.cuts()[0];
        let one_chunk = voice(&audio, 0, edge, "s");
        assert_eq!(
            accepted_spans(&audio, &[&one_chunk], "s", &per_chunk, 0).len(),
            1
        );
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

    #[test]
    fn per_chunk_and_grouped_spans_never_match_each_other() {
        use crate::decode_groups::strategy_id;
        let config = Config::default();
        let hints = DecodeHints::from_candidates(&["Priya".into()], &[]);
        let grouped = fingerprint_with_strategy(&config, &hints, &strategy_id(GROUP));
        let per_chunk = fingerprint_with_strategy(&config, &hints, &strategy_id(0));
        let other_target = fingerprint_with_strategy(&config, &hints, &strategy_id(25 * RATE));
        assert_ne!(grouped, per_chunk);
        assert_ne!(grouped, other_target);
        // Spans saved before the strategy was part of the fingerprint.
        let mut hasher = Sha256::new();
        hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
        hasher.update([0]);
        hasher.update(transcribe::effective_batch_engine(&config).as_bytes());
        hasher.update([0]);
        hasher.update(format!("{:?}", config.transcription).as_bytes());
        hasher.update([0]);
        hasher.update(format!("{:?}", hints.without_audio_format_extension()).as_bytes());
        let unversioned = hex(&hasher.finalize());
        for current in [&grouped, &per_chunk, &settings_fingerprint(&config, &hints)] {
            assert_ne!(*current, unversioned);
        }
        assert!([grouped, per_chunk].contains(&settings_fingerprint(&config, &hints)));
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
        let pcm: Vec<i16> = bursts(60.0).iter().map(|&s| (s * 32768.0) as i16).collect();
        let samples = write_wav(&path, &pcm);
        let Some(config) = speechless_config() else {
            return;
        };
        let hints = DecodeHints::from_candidates(&["Priya".into()], &[]);
        let (config, hints) = source_inputs(CheckpointSource::Voice, &config, &hints);
        let settings = settings_fingerprint(&config, &hints);
        let (_, c) = plan_and_cuts(&samples, transcribe::meeting_group_target(&config));
        assert!(c.len() > 2 && c[1] < samples.len(), "{c:?}");
        let mut first = voice(&samples, 0, c[1], &settings);
        first.lines = vec!["[0:00] first piece".into()];
        let mut second = voice(&samples, c[1], samples.len(), &settings);
        second.lines = vec!["[0:01] second piece".into()];

        let result = transcribe_meeting_with_spans(
            &path,
            CheckpointSource::Voice,
            &config,
            &hints,
            &[&second, &first],
            None,
        )
        .unwrap();

        assert_eq!(result.text, "[0:00] first piece\n[0:01] second piece\n");
        assert_eq!(result.stats.audio_duration_secs, 60.0);
        assert_eq!(result.stats.final_words, 6);
    }

    #[test]
    fn checkpoint_lines_go_through_the_same_cleanup_as_the_batch_pass() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meeting.wav");
        let pcm: Vec<i16> = bursts(60.0).iter().map(|&s| (s * 32768.0) as i16).collect();
        let samples = write_wav(&path, &pcm);
        let Some(config) = speechless_config() else {
            return;
        };
        let hints = DecodeHints::default();
        let settings = settings_fingerprint(&config, &hints);
        let (_, c) = plan_and_cuts(&samples, transcribe::meeting_group_target(&config));
        assert!(c.len() > 2 && c[1] < samples.len(), "{c:?}");
        // A repetition loop split across two spans is collapsed as one run.
        let mut first = span(CheckpointSource::Recording, 0, &samples[..c[1]], &settings);
        first.lines = vec![
            "[0:00] thank you".into(),
            "[0:00] thank you".into(),
            "[0:00] thank you".into(),
        ];
        let mut second = span(
            CheckpointSource::Recording,
            c[1] as u64,
            &samples[c[1]..],
            &settings,
        );
        second.lines = vec!["[0:01] thank you".into(), "[0:01] next topic".into()];
        let spans = [&first, &second];

        let result = transcribe_meeting_with_spans(
            &path,
            CheckpointSource::Recording,
            &config,
            &hints,
            &spans,
            None,
        )
        .unwrap();

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
    fn final_title_context_and_calendar_changes_reject_old_spans() {
        let config = Config::default();
        let event: crate::calendar::CalendarEvent = serde_json::from_value(serde_json::json!({
            "title": "Priya product planning", "start": "2026-11-06 10:00", "minutes_until": 0,
            "attendees": ["Priya Chen"],
        }))
        .unwrap();
        let hints_for = |title, context, event| {
            crate::pipeline::queued_meeting_decode_hints_for_event(
                title, context, event, &config, None,
            )
        };
        let title = Some("Priya product planning");
        let context = Some("Discussing Project Tesseract");
        let initial = hints_for(title, context, None);
        let (source_config, initial) = source_inputs(CheckpointSource::Voice, &config, &initial);
        let audio = ramp(16_000);
        let plan = DecodePlan::for_source(&audio, GROUP);
        let old = span(
            CheckpointSource::Voice,
            0,
            &audio,
            &settings_fingerprint(&source_config, &initial),
        );
        for hints in [
            hints_for(Some("Amara finance review"), context, None),
            hints_for(title, Some("Discussing Meridian Holdings"), None),
            hints_for(title, context, Some(&event)),
        ] {
            let (_, final_hints) = source_inputs(CheckpointSource::Voice, &config, &hints);
            assert!(accepted_spans(
                &audio,
                &[&old],
                &settings_fingerprint(&source_config, &final_hints),
                &plan,
                GROUP,
            )
            .is_empty());
        }
        // A calendar match present at capture and final processing uses exactly
        // the same shared hint builder and can reuse the completed span.
        let calendar_hints = hints_for(title, context, Some(&event));
        let (_, calendar_hints) = source_inputs(CheckpointSource::Voice, &config, &calendar_hints);
        let settings = settings_fingerprint(&source_config, &calendar_hints);
        let matching = span(CheckpointSource::Voice, 0, &audio, &settings);
        assert_eq!(
            accepted_spans(&audio, &[&matching], &settings, &plan, GROUP).len(),
            1
        );
    }

    /// What `SpanTranscriber` does for one span, with the fake decoder.
    fn fake_span(
        audio: &[f32],
        (from, to): (usize, usize),
        decoder: &mut crate::decode_groups::fake::FakeWhisper,
    ) -> CheckpointSpan {
        let samples = &audio[from..to];
        let plan = DecodePlan::for_source(samples, GROUP);
        let lines = crate::decode_groups::decode_groups(decoder, samples, plan.groups(), from)
            .unwrap()
            .lines;
        CheckpointSpan {
            lines,
            ..span(CheckpointSource::System, from as u64, samples, "s")
        }
    }

    /// What `transcribe_meeting_with_spans` does after loading the audio, with
    /// the fake decoder. Returns the text and the final pass's decoder calls.
    fn fake_final_pass(
        audio: &[f32],
        spans: &[&CheckpointSpan],
        decoder: &mut crate::decode_groups::fake::FakeWhisper,
    ) -> (String, usize) {
        let accepted = SourcePass::new(audio, CheckpointSource::System, "s".into(), GROUP)
            .accept(spans)
            .len();
        assert_eq!(accepted, spans.len(), "a span was rejected");
        let before = decoder.calls;
        let text = run_pass(audio, CheckpointSource::System, spans, None, decoder).unwrap();
        (text, decoder.calls - before)
    }

    /// The final pass over one source, as `transcribe_meeting_with_spans`
    /// runs it after loading the audio, with any decoder and settings "s".
    /// Returns the text after cleanup.
    fn run_pass(
        audio: &[f32],
        source: CheckpointSource,
        spans: &[&CheckpointSpan],
        progress: Option<&mut CheckpointFile>,
        decoder: &mut impl crate::decode_groups::AudioDecoder,
    ) -> Result<String, TranscribeError> {
        let pass = SourcePass::new(audio, source, "s".into(), GROUP);
        let accepted = pass.accept(spans);
        let lines = pass.assemble(&accepted, progress, |groups, finished| {
            crate::decode_groups::decode_groups_reporting(decoder, audio, groups, 0, finished)
        })?;
        Ok(transcribe::finish_chunk_lines(lines).unwrap().text)
    }

    fn assert_checkpoints_do_not_change_the_transcript(loop_ids: &[u32]) {
        use crate::decode_groups::fake::FakeWhisper;
        let audio = bursts(420.0);
        let plan = DecodePlan::for_source(&audio, GROUP);
        let mut decoder = FakeWhisper::looping(loop_ids);
        let (whole, calls) = fake_final_pass(&audio, &[], &mut decoder);
        if loop_ids.is_empty() {
            assert_eq!(calls, plan.groups().len());
        } else {
            assert!(calls > plan.groups().len(), "no group was retried");
        }
        assert!(whole
            .lines()
            .all(|line| whisper_guard::find_repetition_loop(line).is_none()));
        assert!(whole.lines().count() > 40, "{}", whole.lines().count());

        // The recording-time producer's spans. The audio after the last cut
        // was not finished at Stop and is left to the final pass.
        let cuts = crate::capture_checkpoint::cut_like_feed(&audio, 120 * RATE, GROUP);
        assert!(cuts.len() >= 2, "{cuts:?}");
        let mut producer = FakeWhisper::looping(loop_ids);
        let spans: Vec<CheckpointSpan> = cuts
            .iter()
            .map(|&cut| fake_span(&audio, cut, &mut producer))
            .collect();
        let all: Vec<&CheckpointSpan> = spans.iter().collect();
        let (full, tail_calls) = fake_final_pass(&audio, &all, &mut decoder);
        assert_eq!(full, whole);
        let tail_groups = plan
            .groups()
            .iter()
            .filter(|g| g.start() >= cuts.last().unwrap().1)
            .count();
        assert!(tail_groups > 0);
        assert!(tail_calls >= tail_groups && tail_calls < calls);

        // Only some spans: the first one missing, or only the first one.
        for partial in [&all[1..], &all[..1]] {
            assert_eq!(fake_final_pass(&audio, partial, &mut decoder).0, whole);
        }
        // A fully covered source makes no decoder call.
        let covered = fake_span(&audio, (0, audio.len()), &mut producer);
        assert_eq!(
            fake_final_pass(&audio, &[&covered], &mut decoder),
            (whole, 0)
        );
    }

    #[test]
    fn no_full_and_partial_checkpoints_give_the_same_transcript() {
        assert_checkpoints_do_not_change_the_transcript(&[]);
    }

    #[test]
    fn the_loop_retry_is_the_same_in_spans_gaps_and_the_whole_file() {
        // Bursts early, in the middle and late, so that loops land in spans
        // and in the tail that the final pass decodes.
        let ids: Vec<u32> = (0..300).step_by(7).collect();
        assert_checkpoints_do_not_change_the_transcript(&ids);
    }

    fn saved_spans(path: &Path) -> Vec<(CheckpointSource, u64, u64)> {
        let mut spans: Vec<_> = load_checkpoint_file(path)
            .unwrap()
            .spans
            .iter()
            .map(|s| (s.source, s.start_sample, s.end_sample))
            .collect();
        spans.sort_by_key(|&(source, start, _)| (source as u8, start));
        spans
    }

    fn temporaries(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn a_saved_checkpoint_is_owner_only_and_loads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job-1.checkpoint.json");
        let audio = ramp(32_000);
        let checkpoint = TranscriptCheckpoint {
            spans: vec![span(CheckpointSource::Voice, 0, &audio, "s")],
            ..Default::default()
        };
        save_checkpoint_file(&path, &checkpoint).unwrap();
        assert_eq!(load_checkpoint_file(&path), Some(checkpoint));
        assert!(temporaries(dir.path()).is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn missing_corrupt_and_unsupported_files_load_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job-1.checkpoint.json");
        assert_eq!(load_checkpoint_file(&path), None);
        // A file cut off in the middle, as a plain write could leave it.
        let audio = ramp(16_000);
        let full = serde_json::to_vec(&TranscriptCheckpoint {
            spans: vec![span(CheckpointSource::Voice, 0, &audio, "s")],
            ..Default::default()
        })
        .unwrap();
        std::fs::write(&path, &full[..full.len() / 2]).unwrap();
        assert_eq!(load_checkpoint_file(&path), None);
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(load_checkpoint_file(&path), None);
        let future = TranscriptCheckpoint {
            version: CHECKPOINT_VERSION + 1,
            spans: vec![span(CheckpointSource::Voice, 0, &audio, "s")],
        };
        std::fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
        assert_eq!(load_checkpoint_file(&path), None);
        // The final pass then starts empty, and its first save replaces the file.
        let mut file = CheckpointFile::open(&path);
        assert!(file.checkpoint().is_empty());
        file.add(span(CheckpointSource::System, 0, &audio, "s"))
            .unwrap();
        assert_eq!(
            saved_spans(&path),
            vec![(CheckpointSource::System, 0, 16_000)]
        );
    }

    #[test]
    fn a_failed_replacement_keeps_the_previous_checkpoint_usable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job-1.checkpoint.json");
        let audio = ramp(32_000);
        let before = TranscriptCheckpoint {
            spans: vec![span(CheckpointSource::Voice, 0, &audio[..16_000], "s")],
            ..Default::default()
        };
        save_checkpoint_file(&path, &before).unwrap();
        let after = TranscriptCheckpoint {
            spans: vec![span(CheckpointSource::Voice, 0, &audio, "s")],
            ..Default::default()
        };
        let bytes = serde_json::to_vec(&after).unwrap();

        // The rename fails. The temporary file was complete and owner-only.
        let error = replace_file_with(&path, &bytes, |temporary, _| {
            assert_eq!(std::fs::read(temporary.path()).unwrap(), bytes);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = temporary.as_file().metadata().unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600);
            }
            Err(std::io::Error::other("rename failed"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "rename failed");
        assert_eq!(load_checkpoint_file(&path), Some(before.clone()));
        assert!(temporaries(dir.path()).is_empty());

        // The process dies before the rename: the temporary file stays.
        let _ = replace_file_with(&path, &bytes, |temporary, _| {
            temporary.into_temp_path().keep().unwrap();
            Err(std::io::Error::other("killed"))
        });
        assert_eq!(temporaries(dir.path()).len(), 1);
        assert_eq!(load_checkpoint_file(&path), Some(before));

        // Removal takes the checkpoint and what a crash left beside it, and
        // only the files of this job.
        let other = dir.path().join("job-2.checkpoint.json");
        save_checkpoint_file(&other, &after).unwrap();
        remove_checkpoint_file(&path);
        assert!(!path.exists());
        assert!(temporaries(dir.path()).is_empty());
        assert!(other.exists());
    }

    #[test]
    fn added_spans_replace_overlapping_spans_of_their_own_source_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job-1.checkpoint.json");
        let audio = ramp(10 * RATE);
        let old = TranscriptCheckpoint {
            spans: vec![
                voice(&audio, 0, 2 * RATE, "old"),
                voice(&audio, 2 * RATE, 5 * RATE, "old"),
                voice(&audio, 5 * RATE, 7 * RATE, "old"),
                span(CheckpointSource::System, 0, &audio, "old"),
            ],
            ..Default::default()
        };
        save_checkpoint_file(&path, &old).unwrap();
        let mut file = CheckpointFile::open(&path);
        assert_eq!(file.checkpoint(), &old);
        file.add(voice(&audio, RATE, 6 * RATE, "new")).unwrap();
        use CheckpointSource::*;
        assert_eq!(
            saved_spans(&path),
            vec![
                (Voice, RATE as u64, 6 * RATE as u64),
                (System, 0, 10 * RATE as u64),
            ]
        );
        // Adjacent spans stay.
        file.add(voice(&audio, 6 * RATE, 8 * RATE, "new")).unwrap();
        file.add(voice(&audio, 0, RATE, "new")).unwrap();
        assert_eq!(saved_spans(&path).len(), 4);
    }

    /// The fake decoder in a worker that dies after `left` more calls. The
    /// call that fails is the interrupted group.
    struct DiesAfter {
        decoder: crate::decode_groups::fake::FakeWhisper,
        left: usize,
    }

    impl DiesAfter {
        fn new(left: usize) -> Self {
            Self {
                decoder: crate::decode_groups::fake::FakeWhisper::new(),
                left,
            }
        }
    }

    impl crate::decode_groups::AudioDecoder for DiesAfter {
        fn decode(
            &mut self,
            audio: &[f32],
        ) -> Result<Option<crate::decode_groups::DecodedAudio>, TranscribeError> {
            if self.left == 0 {
                return Err(TranscribeError::TranscriptionFailed("worker died".into()));
            }
            self.left -= 1;
            self.decoder.decode(audio)
        }
    }

    /// One attempt of the final pass over both stems of a call, as the
    /// pipeline runs it for a job: microphone first, then system, each with
    /// the spans of the job's checkpoint file, saving to it as groups finish.
    fn attempt(
        path: &Path,
        sources: &[(CheckpointSource, &[f32])],
        settings: &str,
        decoder: &mut impl crate::decode_groups::AudioDecoder,
    ) -> Result<Vec<String>, TranscribeError> {
        let mut file = CheckpointFile::open(path);
        let offered = file.checkpoint().clone();
        let mut texts = Vec::new();
        for &(source, audio) in sources {
            let pass = SourcePass::new(audio, source, settings.into(), GROUP);
            let spans = offered.spans_for(source);
            let accepted = pass.accept(&spans);
            let lines = pass.assemble(&accepted, Some(&mut file), |groups, finished| {
                crate::decode_groups::decode_groups_reporting(decoder, audio, groups, 0, finished)
            })?;
            texts.push(transcribe::finish_chunk_lines(lines).unwrap().text);
        }
        Ok(texts)
    }

    /// The saved spans of `source` as ranges, in order.
    fn saved_ranges(path: &Path, source: CheckpointSource) -> Vec<(usize, usize)> {
        load_checkpoint_file(path)
            .unwrap()
            .spans_for(source)
            .iter()
            .map(|s| (s.start_sample as usize, s.end_sample as usize))
            .collect()
    }

    fn assert_contiguous_from_zero(ranges: &[(usize, usize)]) -> usize {
        let mut end = 0;
        for &(from, to) in ranges {
            assert_eq!(from, end, "{ranges:?}");
            end = to;
        }
        end
    }

    #[test]
    fn interruption_in_the_second_stem_decodes_only_the_unfinished_groups() {
        use crate::decode_groups::fake::FakeWhisper;
        use CheckpointSource::{System, Voice};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job-1.checkpoint.json");
        let voice = bursts(420.0);
        let system = bursts_from(420.0, 150);
        let sources: [(CheckpointSource, &[f32]); 2] = [(Voice, &voice), (System, &system)];
        let voice_groups = DecodePlan::for_source(&voice, GROUP).groups().len();
        let system_plan = DecodePlan::for_source(&system, GROUP);
        let system_groups = system_plan.groups().len();

        // An uninterrupted run, without a checkpoint file.
        let mut decoder = FakeWhisper::new();
        let expected: Vec<String> = sources
            .iter()
            .map(|&(source, audio)| run_pass(audio, source, &[], None, &mut decoder).unwrap())
            .collect();
        assert_eq!(decoder.calls, voice_groups + system_groups);

        // The worker dies halfway through the system stem.
        let finished = voice_groups + system_groups / 2;
        let mut dying = DiesAfter::new(finished);
        assert!(attempt(&path, &sources, "s", &mut dying).is_err());
        // The microphone stem was saved whole before the system stem started.
        let saved_voice = saved_ranges(&path, Voice);
        assert_eq!(assert_contiguous_from_zero(&saved_voice), voice.len());
        // About every two minutes of audio, after a whole group.
        for &(from, to) in &saved_voice[..saved_voice.len() - 1] {
            assert!(
                (120 * RATE..180 * RATE).contains(&(to - from)),
                "{saved_voice:?}"
            );
        }
        let saved_system = saved_ranges(&path, System);
        let resume_at = assert_contiguous_from_zero(&saved_system);
        assert!(
            resume_at > 0 && resume_at < system.len(),
            "{saved_system:?}"
        );
        // The interrupted group, and those after it, are not saved.
        let interrupted = &system_plan.groups()[system_groups / 2];
        assert!(resume_at <= interrupted.start(), "{resume_at}");

        // The retry decodes only the system groups after the saved spans and
        // gives the same transcript as the uninterrupted run.
        let mut retry = FakeWhisper::new();
        assert_eq!(attempt(&path, &sources, "s", &mut retry).unwrap(), expected);
        let unfinished = system_plan
            .groups()
            .iter()
            .filter(|group| group.start() >= resume_at)
            .count();
        assert_eq!(retry.calls, unfinished);
        assert!(unfinished < system_groups - system_groups / 2 + 4);
        assert_eq!(
            assert_contiguous_from_zero(&saved_ranges(&path, System)),
            system.len()
        );

        // A retry after diarization started finds both stems saved whole.
        let mut after_diarization = FakeWhisper::new();
        assert_eq!(
            attempt(&path, &sources, "s", &mut after_diarization).unwrap(),
            expected
        );
        assert_eq!(after_diarization.calls, 0);
    }

    #[test]
    fn repeated_retries_finish_without_duplicate_or_lost_words() {
        use crate::decode_groups::fake::FakeWhisper;
        use CheckpointSource::Voice;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job-1.checkpoint.json");
        let voice = bursts(600.0);
        let sources: [(CheckpointSource, &[f32]); 1] = [(Voice, &voice)];
        let groups = DecodePlan::for_source(&voice, GROUP).groups().len();
        let expected = vec![run_pass(&voice, Voice, &[], None, &mut FakeWhisper::new()).unwrap()];

        // The host saved recording-time spans for the first part of the call.
        let cuts = crate::capture_checkpoint::cut_like_feed(&voice, 120 * RATE, GROUP);
        let mut producer = FakeWhisper::new();
        let mut host = TranscriptCheckpoint::default();
        for &cut in &cuts[..2] {
            let mut span = fake_span(&voice, cut, &mut producer);
            span.source = Voice;
            host.spans.push(span);
        }
        save_checkpoint_file(&path, &host).unwrap();
        let host_end = cuts[1].1;

        // Each attempt dies after a few calls, until one finishes.
        let mut attempts = 0;
        let mut calls = 0;
        let texts = loop {
            attempts += 1;
            assert!(attempts < 30, "no progress");
            let mut dying = DiesAfter::new(4);
            let result = attempt(&path, &sources, "s", &mut dying);
            calls += dying.decoder.calls;
            if let Ok(texts) = result {
                break texts;
            }
            let saved = saved_ranges(&path, Voice);
            assert_contiguous_from_zero(&saved);
            // The host's spans stay, and nothing overlaps them.
            assert_eq!(&saved[..2], &cuts[..2]);
        };
        assert!(attempts > 2, "{attempts}");
        assert_eq!(texts, expected);
        let words = |text: &str| -> Vec<String> {
            let mut words: Vec<String> = text
                .lines()
                .map(|line| line.split_once("] ").unwrap().1.to_string())
                .collect();
            words.sort();
            words
        };
        let mut unique = words(&texts[0]);
        unique.dedup();
        assert_eq!(words(&texts[0]), unique, "a word appears twice");
        // No attempt decodes the host's spans. Work lost to an interruption
        // is the groups since the last save, a few at most, never the call.
        let after_host = DecodePlan::for_source(&voice, GROUP)
            .groups()
            .iter()
            .filter(|g| g.start() >= host_end)
            .count();
        assert!(after_host < groups);
        assert!(calls <= after_host + 4 * (attempts - 1), "{calls}");
        assert_eq!(
            assert_contiguous_from_zero(&saved_ranges(&path, Voice)),
            voice.len()
        );
    }

    #[test]
    fn changed_settings_audio_or_version_decode_again_and_never_rekey_old_text() {
        use crate::decode_groups::fake::FakeWhisper;
        use CheckpointSource::Voice;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job-1.checkpoint.json");
        let voice = bursts(420.0);
        let plan = DecodePlan::for_source(&voice, GROUP);
        let groups = plan.groups().len();
        let sources: [(CheckpointSource, &[f32]); 1] = [(Voice, &voice)];
        let mut first = FakeWhisper::new();
        let expected = attempt(&path, &sources, "s", &mut first).unwrap();
        assert_eq!(first.calls, groups);
        let spans_before = saved_ranges(&path, Voice);
        assert!(spans_before.len() > 2);

        // Other settings or hints: every span is rejected and decoded again.
        // The new spans replace the old ones, and only they are saved.
        let mut other = FakeWhisper::new();
        assert_eq!(attempt(&path, &sources, "t", &mut other).unwrap(), expected);
        assert_eq!(other.calls, groups);
        let saved = load_checkpoint_file(&path).unwrap();
        assert!(saved.spans.iter().all(|span| span.settings == "t"));
        assert_eq!(saved_ranges(&path, Voice), spans_before);
        let mut again = FakeWhisper::new();
        attempt(&path, &sources, "t", &mut again).unwrap();
        assert_eq!(again.calls, 0);

        // Changed audio in the second span: only that span is decoded again.
        let mut changed = voice.clone();
        let (from, to) = spans_before[1];
        let burst = (from..to).find(|&i| changed[i] != 0.0).unwrap();
        changed[burst] += 0.0001;
        let changed_sources: [(CheckpointSource, &[f32]); 1] = [(Voice, &changed)];
        let mut decoder = FakeWhisper::new();
        attempt(&path, &changed_sources, "t", &mut decoder).unwrap();
        let in_span = plan
            .groups()
            .iter()
            .filter(|g| g.start() >= from && g.end() <= to)
            .count();
        assert_eq!(decoder.calls, in_span);

        // A checkpoint of another version is not read. Everything is decoded
        // again, and the file is replaced with the current version.
        let mut future = load_checkpoint_file(&path).unwrap();
        future.version = CHECKPOINT_VERSION + 1;
        std::fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
        let mut decoder = FakeWhisper::new();
        assert_eq!(
            attempt(&path, &sources, "t", &mut decoder).unwrap(),
            expected
        );
        assert_eq!(decoder.calls, groups);
        assert_eq!(
            load_checkpoint_file(&path).unwrap().version,
            CHECKPOINT_VERSION
        );
    }

    #[test]
    fn a_failed_save_does_not_stop_the_transcription() {
        use crate::decode_groups::fake::FakeWhisper;
        let dir = tempfile::tempdir().unwrap();
        // The checkpoint path is a directory, so every replacement fails.
        let path = dir.path().join("job-1.checkpoint.json");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep"), b"x").unwrap();
        let voice = bursts(300.0);
        let expected = run_pass(
            &voice,
            CheckpointSource::Voice,
            &[],
            None,
            &mut FakeWhisper::new(),
        )
        .unwrap();
        let mut file = CheckpointFile::open(&path);
        let text = run_pass(
            &voice,
            CheckpointSource::Voice,
            &[],
            Some(&mut file),
            &mut FakeWhisper::new(),
        )
        .unwrap();
        assert_eq!(text, expected);
        assert!(temporaries(dir.path()).is_empty());
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
