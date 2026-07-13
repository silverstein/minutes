use crate::config::Config;
use crate::error::TranscribeError;
#[cfg(test)]
use crate::transcription_coordinator::{
    collapse_noise_markers, dedup_interleaved, dedup_segments, strip_foreign_script,
    trim_trailing_noise,
};
use crate::transcription_coordinator::{run_transcript_cleanup_pipeline, TranscriptCleanupStage};
#[cfg(feature = "parakeet")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "parakeet")]
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
#[cfg(feature = "parakeet")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "parakeet")]
use std::sync::{Mutex, OnceLock};
#[cfg(feature = "parakeet")]
use std::time::Instant;

// Re-export from whisper-guard for public API compatibility
pub use whisper_guard::audio::{normalize_audio, resample, strip_silence};
#[cfg(feature = "whisper")]
pub use whisper_guard::params::{default_whisper_params, streaming_whisper_params};
pub use whisper_guard::segments::{clean_transcript, CleanStats};

/// Diagnostics from the transcription filtering pipeline.
/// Tracks how many segments survived each anti-hallucination layer,
/// so blank transcripts can be diagnosed.
#[derive(Debug, Clone, Default)]
pub struct FilterStats {
    /// Total audio duration in seconds (after loading)
    pub audio_duration_secs: f64,
    /// Samples after silence stripping (0 = all silence)
    pub samples_after_silence_strip: usize,
    /// Raw segments from whisper/parakeet before any filtering
    pub raw_segments: usize,
    /// Segments skipped by whisper's no_speech_prob > 0.8
    pub skipped_no_speech: usize,
    /// Segments with non-empty text after no_speech filter
    pub after_no_speech_filter: usize,
    /// After consecutive dedup
    pub after_dedup: usize,
    /// After interleaved dedup
    pub after_interleaved: usize,
    /// After foreign-script filter
    pub after_script_filter: usize,
    /// After noise marker collapse
    pub after_noise_markers: usize,
    /// After trailing noise trim
    pub after_trailing_trim: usize,
    /// Segments rescued from the no_speech filter (would have been blank otherwise)
    pub rescued_no_speech: usize,
    /// Final word count
    pub final_words: usize,
}

impl FilterStats {
    /// Human-readable summary of what each layer removed.
    pub fn diagnosis(&self) -> String {
        let mut parts = Vec::new();
        parts.push(format!("audio: {:.1}s", self.audio_duration_secs));
        if self.samples_after_silence_strip == 0 {
            parts.push("silence strip removed ALL audio".into());
            return parts.join(", ");
        }
        parts.push(format!("whisper produced {} segments", self.raw_segments));
        if self.raw_segments == 0 {
            return parts.join(", ");
        }
        if self.rescued_no_speech > 0 {
            parts.push(format!(
                "no_speech rescue: {} segments saved (would have been blank)",
                self.rescued_no_speech
            ));
        } else if self.skipped_no_speech > 0 {
            parts.push(format!(
                "no_speech filter: -{} → {}",
                self.skipped_no_speech, self.after_no_speech_filter
            ));
        }
        if self.after_dedup < self.after_no_speech_filter {
            parts.push(format!(
                "dedup: -{} → {}",
                self.after_no_speech_filter - self.after_dedup,
                self.after_dedup
            ));
        }
        if self.after_interleaved < self.after_dedup {
            parts.push(format!(
                "interleaved: -{} → {}",
                self.after_dedup - self.after_interleaved,
                self.after_interleaved
            ));
        }
        if self.after_script_filter < self.after_interleaved {
            parts.push(format!(
                "script filter: -{} → {}",
                self.after_interleaved - self.after_script_filter,
                self.after_script_filter
            ));
        }
        if self.after_noise_markers < self.after_script_filter {
            parts.push(format!(
                "noise markers: -{} → {}",
                self.after_script_filter - self.after_noise_markers,
                self.after_noise_markers
            ));
        }
        if self.after_trailing_trim < self.after_noise_markers {
            parts.push(format!(
                "trailing trim: -{} → {}",
                self.after_noise_markers - self.after_trailing_trim,
                self.after_trailing_trim
            ));
        }
        parts.push(format!("final: {} words", self.final_words));
        parts.join(", ")
    }
}

/// A single timestamped segment from the transcription pipeline.
///
/// `start` and `end` are in seconds with sub-second (centisecond) precision
/// for the whisper engine. The sherpa engine provides start only; end is
/// inferred from the next segment's start (or total audio duration for the
/// final segment).
#[derive(Debug, Clone)]
pub struct TranscriptSegment {
    /// Segment start time in seconds.
    pub start: f64,
    /// Segment end time in seconds.
    pub end: f64,
    /// Segment text, without any `[m:ss]` timestamp prefix.
    pub text: String,
}

/// Result from the transcription pipeline, including filter diagnostics.
#[derive(Debug, Clone)]
pub struct TranscribeResult {
    pub text: String,
    pub stats: FilterStats,
    /// Per-segment timing and clean text. Populated by the whisper and sherpa
    /// engines; empty for parakeet (which uses a subprocess-based format) and
    /// for chunked whisper paths (where segments are aggregated into lines).
    pub segments: Vec<TranscriptSegment>,
    /// Language code detected by the ASR engine (e.g. `"en"`, `"es"`).
    /// `None` when the engine did not report a language or when the whisper
    /// feature is disabled.
    pub detected_language: Option<String>,
}

/// Meeting-local lexical hints that can safely inform batch decoding.
///
/// These are intentionally narrower than the global graph-derived phrase set:
/// the goal is to bias decoding toward names and terms that are plausible in
/// this specific recording, without dragging in the user's full history.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecodeHints {
    priority_phrases: Vec<String>,
    contextual_phrases: Vec<String>,
}

impl DecodeHints {
    pub fn from_candidates(priority: &[String], contextual: &[String]) -> Self {
        let mut seen = std::collections::HashSet::new();
        let mut priority_phrases = Vec::new();
        let mut contextual_phrases = Vec::new();

        for candidate in priority {
            if let Some(normalized) = normalize_decode_hint_candidate(candidate, true) {
                let key = normalized.to_ascii_lowercase();
                if seen.insert(key) {
                    priority_phrases.push(normalized);
                }
            }
            if priority_phrases.len() >= 8 {
                break;
            }
        }

        for candidate in contextual {
            if let Some(normalized) = normalize_decode_hint_candidate(candidate, false) {
                let key = normalized.to_ascii_lowercase();
                if seen.insert(key) {
                    contextual_phrases.push(normalized);
                }
            }
            if contextual_phrases.len() >= 6 {
                break;
            }
        }

        Self {
            priority_phrases,
            contextual_phrases,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.priority_phrases.is_empty() && self.contextual_phrases.is_empty()
    }

    pub fn with_additional_candidates(&self, priority: &[String], contextual: &[String]) -> Self {
        let mut merged_priority = self.priority_phrases.clone();
        merged_priority.extend(priority.iter().cloned());

        let mut merged_contextual = self.contextual_phrases.clone();
        merged_contextual.extend(contextual.iter().cloned());

        Self::from_candidates(&merged_priority, &merged_contextual)
    }

    pub(crate) fn debug_priority_phrases(&self) -> Vec<String> {
        self.priority_phrases.clone()
    }

    pub(crate) fn debug_contextual_phrases(&self) -> Vec<String> {
        self.contextual_phrases.clone()
    }

    fn combined_phrases(&self, limit: usize) -> Vec<String> {
        self.priority_phrases
            .iter()
            .chain(self.contextual_phrases.iter())
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn whisper_initial_prompt(&self) -> Option<String> {
        let phrases = self.combined_phrases(12);
        if phrases.is_empty() {
            return None;
        }
        Some(format!(
            "Names and terms that may appear in this audio: {}. Preserve spelling exactly when heard.",
            phrases.join(", ")
        ))
    }

    #[cfg(feature = "parakeet")]
    fn parakeet_local_boost_phrases(&self) -> Vec<String> {
        self.priority_phrases
            .iter()
            .take(8)
            .filter(|&phrase| {
                let has_digit = phrase.chars().any(|c| c.is_ascii_digit());
                let token_count = phrase.split_whitespace().count();
                has_digit || token_count > 1
            })
            .cloned()
            .collect()
    }
}

fn normalize_decode_hint_candidate(
    candidate: &str,
    allow_short_single_token: bool,
) -> Option<String> {
    let trimmed = candidate
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'')
        .trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "unknown"
            | "unknown speaker"
            | "speaker 0"
            | "speaker 1"
            | "speaker 2"
            | "speaker 3"
            | "unassigned"
    ) {
        return None;
    }
    if let Some((local, domain)) = trimmed.split_once('@') {
        if !local.is_empty() && domain.contains('.') {
            return None;
        }
    }

    let word_count = trimmed.split_whitespace().count();
    let has_signal = trimmed
        .chars()
        .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    if word_count == 1 {
        let len = trimmed.chars().count();
        if len < 3 && !trimmed.chars().any(|c| c.is_ascii_digit()) {
            return None;
        }
        if !allow_short_single_token && len < 5 && !trimmed.chars().any(|c| c.is_ascii_digit()) {
            return None;
        }
        if !has_signal && trimmed.chars().all(|c| c.is_ascii_lowercase()) {
            return None;
        }
    }

    Some(trimmed.to_string())
}

// ──────────────────────────────────────────────────────────────
// Transcription pipeline:
//
//   Input audio (.wav, .m4a, .mp3, .ogg)
//        │
//        ├─ .wav ──────────────────────────────────▶ engine
//        │
//        └─ .m4a/.mp3/.ogg ─▶ symphonia decode ─▶ engine
//                              (to 16kHz mono PCM)
//
// Engines:
//   - whisper (default): whisper.cpp via whisper-rs, Apple Accelerate on M-series
//   - parakeet (opt-in): parakeet.cpp via subprocess, Metal on Apple Silicon
//
// Engine is selected via config.transcription.engine ("whisper" or "parakeet").
// Model must be downloaded first via `minutes setup`.
// ──────────────────────────────────────────────────────────────

/// Transcribe an audio file to text.
///
/// Dispatches to the engine configured in `config.transcription.engine`:
/// - `"whisper"` (default): whisper.cpp via whisper-rs
/// - `"parakeet"`: parakeet.cpp via subprocess
/// - `"apple-speech"`: currently live-transcript-only; batch/default paths
///   fall back to whisper until the experiment graduates
///
/// Handles format conversion (m4a/mp3/ogg → PCM) automatically via symphonia.
/// Both engines produce identical output format: `[M:SS] text` lines.
pub fn transcribe(audio_path: &Path, config: &Config) -> Result<TranscribeResult, TranscribeError> {
    transcribe_with_hints(audio_path, config, &DecodeHints::default())
}

pub fn transcribe_with_hints(
    audio_path: &Path,
    config: &Config,
    hints: &DecodeHints,
) -> Result<TranscribeResult, TranscribeError> {
    transcribe_dispatch(audio_path, config, hints)
}

fn transcribe_dispatch(
    audio_path: &Path,
    config: &Config,
    hints: &DecodeHints,
) -> Result<TranscribeResult, TranscribeError> {
    match config.transcription.engine.as_str() {
        "whisper" => transcribe_whisper_dispatch(audio_path, config, hints),
        "parakeet" => transcribe_parakeet_dispatch(audio_path, config, hints),
        "sherpa" => {
            // Whisper auto-fallback: sherpa is opt-in and needs its model. If the
            // engine isn't compiled in, or the model isn't installed, never break
            // the recording -- fall back to whisper (the bundled default). The
            // `sherpa_engine` path helpers are always compiled, so this check
            // works in any build.
            let model_dir = crate::sherpa_engine::model_dir(config);
            if cfg!(feature = "engine-sherpa")
                && crate::sherpa_engine::model_files_present(&model_dir)
            {
                transcribe_sherpa_dispatch(audio_path, config, hints)
            } else {
                tracing::warn!(
                    model_dir = %model_dir.display(),
                    compiled = cfg!(feature = "engine-sherpa"),
                    "engine = \"sherpa\" selected but unavailable (feature/model); falling back to whisper"
                );
                transcribe_whisper_dispatch(audio_path, config, hints)
            }
        }
        "apple-speech" => {
            tracing::warn!(
                "apple-speech is experimental and live-transcript-only today — falling back to whisper for batch/default transcription"
            );
            transcribe_whisper_dispatch(audio_path, config, hints)
        }
        other => {
            tracing::warn!(
                engine = other,
                "unknown transcription engine — falling back to whisper"
            );
            transcribe_whisper_dispatch(audio_path, config, hints)
        }
    }
}

fn temp_wav_path(prefix: &str) -> Result<PathBuf, TranscribeError> {
    let unique = format!(
        "{}-{}-{}.wav",
        prefix,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    Ok(std::env::temp_dir().join(unique))
}

#[cfg(feature = "parakeet")]
const PARAKEET_LONG_AUDIO_CHUNK_THRESHOLD_SECS: f64 = 60.0;
#[cfg(feature = "parakeet")]
const PARAKEET_LONG_AUDIO_CHUNK_SECS: usize = 45;
#[cfg(feature = "parakeet")]
const PARAKEET_NATIVE_VAD_CHUNK_THRESHOLD_SECS: f64 = 240.0;
#[cfg(feature = "parakeet")]
const PARAKEET_NATIVE_VAD_CHUNK_SECS: usize = 180;
#[cfg(feature = "parakeet")]
const PARAKEET_NATIVE_VAD_THRESHOLD: f32 = 0.5;

#[cfg(feature = "parakeet")]
fn fixed_length_chunks(total_samples: usize, max_chunk_samples: usize) -> Vec<(usize, usize)> {
    if total_samples == 0 || max_chunk_samples == 0 {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < total_samples {
        let end = (start + max_chunk_samples).min(total_samples);
        chunks.push((start, end));
        start = end;
    }
    chunks
}

#[cfg(feature = "parakeet")]
fn parakeet_chunk_ranges(
    total_samples: usize,
    audio_duration_secs: f64,
    has_native_vad: bool,
) -> Option<Vec<(usize, usize)>> {
    // Native Parakeet VAD lets us keep moderately long files intact, but the
    // offline models still become unreliable beyond roughly 4-5 minutes.
    let (threshold_secs, chunk_secs) = if has_native_vad {
        (
            PARAKEET_NATIVE_VAD_CHUNK_THRESHOLD_SECS,
            PARAKEET_NATIVE_VAD_CHUNK_SECS,
        )
    } else {
        (
            PARAKEET_LONG_AUDIO_CHUNK_THRESHOLD_SECS,
            PARAKEET_LONG_AUDIO_CHUNK_SECS,
        )
    };

    if audio_duration_secs <= threshold_secs {
        return None;
    }

    Some(fixed_length_chunks(total_samples, 16000 * chunk_secs))
}

fn transcribe_chunk_ranges(
    samples: &[f32],
    chunk_ranges: &[(usize, usize)],
    audio_duration_secs: f64,
    config: &Config,
    hints: &DecodeHints,
) -> Result<Option<TranscribeResult>, TranscribeError> {
    #[cfg(feature = "whisper")]
    if chunk_transcription_strategy(config) == ChunkTranscriptionStrategy::SharedWhisperContext {
        return transcribe_whisper_chunk_ranges(
            samples,
            chunk_ranges,
            audio_duration_secs,
            config,
            hints,
        );
    }

    let mut all_lines = Vec::new();
    let mut aggregate = FilterStats {
        audio_duration_secs,
        ..Default::default()
    };

    for (chunk_index, (start_sample, end_sample)) in chunk_ranges.iter().enumerate() {
        let chunk_samples = &samples[*start_sample..*end_sample];
        if chunk_samples.is_empty() {
            continue;
        }

        let tmp_wav = temp_wav_path("minutes-meeting-chunk")?;
        write_wav_16k_mono(&tmp_wav, chunk_samples)?;

        let chunk_result = match transcribe_dispatch(&tmp_wav, config, hints) {
            Ok(result) => result,
            Err(TranscribeError::EmptyAudio) | Err(TranscribeError::EmptyTranscript(_)) => {
                tracing::debug!(
                    chunk_index,
                    start_sample,
                    end_sample,
                    "skipping empty VAD chunk"
                );
                let _ = std::fs::remove_file(&tmp_wav);
                continue;
            }
            Err(error) => {
                let _ = std::fs::remove_file(&tmp_wav);
                return Err(error);
            }
        };
        let chunk_offset_secs = *start_sample as f64 / 16000.0;
        let offset_lines =
            offset_timestamped_lines(chunk_result.text.lines(), chunk_offset_secs, chunk_index);
        let _ = std::fs::remove_file(&tmp_wav);

        aggregate.samples_after_silence_strip += chunk_result.stats.samples_after_silence_strip;
        aggregate.raw_segments += chunk_result.stats.raw_segments;
        aggregate.skipped_no_speech += chunk_result.stats.skipped_no_speech;
        aggregate.after_no_speech_filter += chunk_result.stats.after_no_speech_filter;
        aggregate.rescued_no_speech += chunk_result.stats.rescued_no_speech;
        all_lines.extend(offset_lines);
    }

    if all_lines.is_empty() {
        return Ok(None);
    }

    let cleanup = run_transcript_cleanup_pipeline(all_lines);
    aggregate.after_dedup = cleanup.after(TranscriptCleanupStage::DedupSegments);
    aggregate.after_interleaved = cleanup.after(TranscriptCleanupStage::DedupInterleaved);
    aggregate.after_script_filter = cleanup.after(TranscriptCleanupStage::StripForeignScript);
    aggregate.after_noise_markers = cleanup.after(TranscriptCleanupStage::CollapseNoiseMarkers);
    aggregate.after_trailing_trim = cleanup.after(TranscriptCleanupStage::TrimTrailingNoise);

    let text = if cleanup.lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", cleanup.lines.join("\n"))
    };
    aggregate.final_words = text.split_whitespace().count();

    Ok(Some(TranscribeResult {
        text,
        stats: aggregate,
        segments: vec![],
        detected_language: None,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
enum ChunkTranscriptionStrategy {
    SharedWhisperContext,
    DispatchPerChunk,
}

#[cfg_attr(not(feature = "whisper"), allow(dead_code))]
fn chunk_transcription_strategy(config: &Config) -> ChunkTranscriptionStrategy {
    if cfg!(feature = "whisper") && config.transcription.engine == "whisper" {
        ChunkTranscriptionStrategy::SharedWhisperContext
    } else {
        ChunkTranscriptionStrategy::DispatchPerChunk
    }
}

#[cfg(feature = "whisper")]
fn transcribe_whisper_chunk_ranges(
    samples: &[f32],
    chunk_ranges: &[(usize, usize)],
    audio_duration_secs: f64,
    config: &Config,
    hints: &DecodeHints,
) -> Result<Option<TranscribeResult>, TranscribeError> {
    let ctx = load_whisper_context(config)?;
    let mut all_lines = Vec::new();
    let mut aggregate = FilterStats {
        audio_duration_secs,
        ..Default::default()
    };

    for (chunk_index, (start_sample, end_sample)) in chunk_ranges.iter().enumerate() {
        let chunk_samples = &samples[*start_sample..*end_sample];
        if chunk_samples.is_empty() {
            continue;
        }

        let chunk_result = match transcribe_whisper_samples(
            &ctx,
            chunk_samples,
            Path::new("<vad-chunk>"),
            config,
            hints,
        ) {
            Ok(result) => result,
            Err(TranscribeError::EmptyAudio) | Err(TranscribeError::EmptyTranscript(_)) => {
                tracing::debug!(
                    chunk_index,
                    start_sample,
                    end_sample,
                    "skipping empty VAD chunk"
                );
                continue;
            }
            Err(error) => return Err(error),
        };
        let chunk_offset_secs = *start_sample as f64 / 16000.0;
        let offset_lines =
            offset_timestamped_lines(chunk_result.text.lines(), chunk_offset_secs, chunk_index);

        aggregate.samples_after_silence_strip += chunk_result.stats.samples_after_silence_strip;
        aggregate.raw_segments += chunk_result.stats.raw_segments;
        aggregate.skipped_no_speech += chunk_result.stats.skipped_no_speech;
        aggregate.after_no_speech_filter += chunk_result.stats.after_no_speech_filter;
        aggregate.rescued_no_speech += chunk_result.stats.rescued_no_speech;
        all_lines.extend(offset_lines);
    }

    if all_lines.is_empty() {
        return Ok(None);
    }

    let cleanup = run_transcript_cleanup_pipeline(all_lines);
    aggregate.after_dedup = cleanup.after(TranscriptCleanupStage::DedupSegments);
    aggregate.after_interleaved = cleanup.after(TranscriptCleanupStage::DedupInterleaved);
    aggregate.after_script_filter = cleanup.after(TranscriptCleanupStage::StripForeignScript);
    aggregate.after_noise_markers = cleanup.after(TranscriptCleanupStage::CollapseNoiseMarkers);
    aggregate.after_trailing_trim = cleanup.after(TranscriptCleanupStage::TrimTrailingNoise);

    let text = if cleanup.lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", cleanup.lines.join("\n"))
    };
    aggregate.final_words = text.split_whitespace().count();

    Ok(Some(TranscribeResult {
        text,
        stats: aggregate,
        segments: vec![],
        detected_language: None,
    }))
}

/// Meeting-specialized transcription path that can split long recordings at
/// natural pauses before dispatching to the active ASR backend.
pub fn transcribe_meeting(
    audio_path: &Path,
    config: &Config,
) -> Result<TranscribeResult, TranscribeError> {
    transcribe_meeting_with_hints(audio_path, config, &DecodeHints::default())
}

pub fn transcribe_meeting_with_hints(
    audio_path: &Path,
    config: &Config,
    hints: &DecodeHints,
) -> Result<TranscribeResult, TranscribeError> {
    const MIN_MEETING_CHUNKS: usize = 2;

    let samples = load_audio_samples(audio_path)?;
    let audio_duration_secs = samples.len() as f64 / 16000.0;
    let vad_chunks = detect_meeting_vad_chunks(&samples);

    if vad_chunks.len() < MIN_MEETING_CHUNKS {
        return transcribe_dispatch(audio_path, config, hints);
    }

    tracing::info!(
        chunks = vad_chunks.len(),
        audio_secs = format!("{:.1}", audio_duration_secs),
        "meeting transcription using VAD-driven chunk rotation"
    );

    if let Some(result) =
        transcribe_chunk_ranges(&samples, &vad_chunks, audio_duration_secs, config, hints)?
    {
        return Ok(result);
    }

    transcribe_dispatch(audio_path, config, hints)
}

/// Whisper transcription path (existing behavior).
fn transcribe_whisper_dispatch(
    audio_path: &Path,
    config: &Config,
    hints: &DecodeHints,
) -> Result<TranscribeResult, TranscribeError> {
    // Step 1: Load audio as 16kHz mono f32 PCM samples
    let samples = load_audio_samples(audio_path)?;

    // Step 3: Transcribe
    #[cfg(feature = "whisper")]
    {
        let ctx = load_whisper_context(config)?;
        transcribe_whisper_samples(&ctx, &samples, audio_path, config, hints)
    }

    #[cfg(not(feature = "whisper"))]
    {
        let stats = FilterStats {
            audio_duration_secs: samples.len() as f64 / 16000.0,
            ..Default::default()
        };
        let _ = config; // suppress unused warning
        let _ = hints; // only used when the whisper feature is enabled
        let duration_secs = samples.len() as f64 / 16000.0;
        let text = format!(
            "[Transcription placeholder — whisper feature not enabled]\n\
             Audio file: {}\n\
             Duration: {:.1}s ({} samples at 16kHz)\n\
             \n\
             Build with `cargo build --features whisper` and download a model\n\
             via `minutes setup` to enable real transcription.",
            audio_path.display(),
            duration_secs,
            samples.len(),
        );
        Ok(TranscribeResult {
            text,
            stats,
            segments: vec![],
            detected_language: None,
        })
    }
}

/// Parakeet transcription path (subprocess-based).
fn transcribe_parakeet_dispatch(
    audio_path: &Path,
    config: &Config,
    hints: &DecodeHints,
) -> Result<TranscribeResult, TranscribeError> {
    #[cfg(feature = "parakeet")]
    {
        if !crate::parakeet::valid_model(&config.transcription.parakeet_model) {
            return Err(TranscribeError::ParakeetFailed(format!(
                "unknown parakeet model '{}'. Valid: {}",
                config.transcription.parakeet_model,
                crate::config::VALID_PARAKEET_MODELS.join(", ")
            )));
        }
        let samples = load_audio_samples(audio_path)?;
        let audio_duration_secs = samples.len() as f64 / 16000.0;
        let native_vad_path = resolve_parakeet_native_vad_path(config);
        if let Some(chunk_ranges) = parakeet_chunk_ranges(
            samples.len(),
            audio_duration_secs,
            native_vad_path.is_some(),
        ) {
            let chunk_secs = if native_vad_path.is_some() {
                PARAKEET_NATIVE_VAD_CHUNK_SECS
            } else {
                PARAKEET_LONG_AUDIO_CHUNK_SECS
            };
            tracing::info!(
                chunks = chunk_ranges.len(),
                audio_secs = format!("{:.1}", audio_duration_secs),
                chunk_secs,
                native_vad = native_vad_path.is_some(),
                "chunking long parakeet transcription to avoid monolithic decode"
            );
            if let Some(result) = transcribe_chunk_ranges(
                &samples,
                &chunk_ranges,
                audio_duration_secs,
                config,
                hints,
            )? {
                return Ok(result);
            }
            return Err(TranscribeError::EmptyTranscript(
                config.transcription.min_words,
            ));
        }
        transcribe_with_parakeet(audio_path, config, hints)
    }

    #[cfg(not(feature = "parakeet"))]
    {
        let _ = (audio_path, config, hints);
        Err(TranscribeError::EngineNotAvailable("parakeet".into()))
    }
}

/// Dispatch for the opt-in sherpa-onnx engine (parakeet-tdt-0.6b-v3, multilingual).
/// In-process via `sherpa_engine`; off by default behind `engine-sherpa`.
fn transcribe_sherpa_dispatch(
    audio_path: &Path,
    config: &Config,
    hints: &DecodeHints,
) -> Result<TranscribeResult, TranscribeError> {
    #[cfg(feature = "engine-sherpa")]
    {
        // Decode-hint biasing for sherpa is a future follow-up.
        let _ = hints;
        let samples = load_audio_samples(audio_path)?;
        if samples.is_empty() {
            return Err(TranscribeError::EmptyAudio);
        }
        let audio_duration_secs = samples.len() as f64 / 16_000.0;

        // Windowed segments carry a start time, so we emit `[m:ss] text` lines
        // (the same shape the parakeet path produces) -- this makes the sherpa
        // transcript diarization-compatible and renders in the timestamped
        // transcript format, rather than one untimed blob.
        //
        // sherpa exposes start_ms only; end is inferred from the next segment's
        // start (or audio_duration_secs for the final segment).
        let raw_segs = crate::sherpa_engine::transcribe_segments(&samples, config)
            .map_err(TranscribeError::TranscriptionFailed)?;
        // Build parallel (line, start_secs, end_secs, text) data
        let n_raw = raw_segs.len();
        let sherpa_seg_data: Vec<(String, f64, f64, String)> = raw_segs
            .iter()
            .enumerate()
            .map(|(i, (start_ms, text))| {
                let start_secs = *start_ms as f64 / 1000.0;
                let end_secs = if i + 1 < n_raw {
                    raw_segs[i + 1].0 as f64 / 1000.0
                } else {
                    audio_duration_secs
                };
                let secs_int = *start_ms / 1000;
                let line = format!("[{}:{:02}] {}", secs_int / 60, secs_int % 60, text);
                (line, start_secs, end_secs, text.clone())
            })
            .collect();
        let lines: Vec<String> = sherpa_seg_data
            .iter()
            .map(|(l, _, _, _)| l.clone())
            .collect();
        let raw_segments = lines.len();

        // Run the shared whisper-guard cleanup (dedups any window-boundary
        // repeats); its output lines are the transcript.
        let cleanup = run_transcript_cleanup_pipeline(lines.clone());
        let transcript = cleanup.lines.join("\n");
        let transcript = if transcript.is_empty() {
            transcript
        } else {
            format!("{transcript}\n")
        };
        let word_count = transcript.split_whitespace().count();
        if word_count < config.transcription.min_words {
            return Err(TranscribeError::EmptyTranscript(
                config.transcription.min_words,
            ));
        }
        let stats = FilterStats {
            audio_duration_secs,
            raw_segments,
            after_no_speech_filter: raw_segments,
            after_dedup: cleanup.after(TranscriptCleanupStage::DedupSegments),
            after_interleaved: cleanup.after(TranscriptCleanupStage::DedupInterleaved),
            after_script_filter: cleanup.after(TranscriptCleanupStage::StripForeignScript),
            after_noise_markers: cleanup.after(TranscriptCleanupStage::CollapseNoiseMarkers),
            after_trailing_trim: cleanup.after(TranscriptCleanupStage::TrimTrailingNoise),
            final_words: word_count,
            ..Default::default()
        };
        // Correlate surviving lines back to their timing data
        let transcript_segments =
            correlate_sherpa_segments(&cleanup.lines, &lines, &sherpa_seg_data);
        Ok(TranscribeResult {
            text: transcript,
            stats,
            segments: transcript_segments,
            detected_language: None, // sherpa does not report detected language
        })
    }

    #[cfg(not(feature = "engine-sherpa"))]
    {
        let _ = (audio_path, config, hints);
        Err(TranscribeError::EngineNotAvailable("engine-sherpa".into()))
    }
}

/// Correlate cleanup-surviving lines back to sherpa segment timing data.
///
/// `cleanup_lines` is a subsequence of `all_lines` (same order, some removed
/// by dedup). For each surviving line we find the earliest matching entry in
/// `all_seg_data` (which is parallel to `all_lines`) and emit a
/// [`TranscriptSegment`] with the real start/end times.
#[cfg(feature = "engine-sherpa")]
fn correlate_sherpa_segments(
    cleanup_lines: &[String],
    all_lines: &[String],
    all_seg_data: &[(String, f64, f64, String)],
) -> Vec<TranscriptSegment> {
    let mut si = 0usize;
    let mut result = Vec::new();
    for cleaned in cleanup_lines {
        while si < all_lines.len() {
            if &all_lines[si] == cleaned {
                let (_, start, end, text) = &all_seg_data[si];
                result.push(TranscriptSegment {
                    start: *start,
                    end: *end,
                    text: text.clone(),
                });
                si += 1;
                break;
            }
            si += 1;
        }
    }
    result
}

/// Correlate cleanup-surviving whisper lines back to raw segment timing data.
///
/// `cleanup_lines` is a subsequence of `all_lines` (same order, some removed).
/// `seg_data` is parallel to `all_lines`: `(start_secs, end_secs, clean_text)`.
#[cfg(feature = "whisper")]
fn correlate_whisper_segments(
    cleanup_lines: &[String],
    all_lines: &[String],
    seg_data: &[(f64, f64, String)],
) -> Vec<TranscriptSegment> {
    let mut si = 0usize;
    let mut result = Vec::new();
    for cleaned in cleanup_lines {
        while si < all_lines.len() {
            if &all_lines[si] == cleaned {
                let (start, end, text) = &seg_data[si];
                result.push(TranscriptSegment {
                    start: *start,
                    end: *end,
                    text: text.clone(),
                });
                si += 1;
                break;
            }
            si += 1;
        }
    }
    result
}

/// Build `WhisperContextParameters` with GPU explicitly enabled when a GPU
/// backend was compiled in. All call sites should use this instead of
/// `WhisperContextParameters::default()`.
#[cfg(feature = "whisper")]
pub(crate) fn whisper_context_params() -> whisper_rs::WhisperContextParameters<'static> {
    let mut params = whisper_rs::WhisperContextParameters::default();

    let gpu_compiled = cfg!(any(
        feature = "coreml",
        feature = "cuda",
        feature = "hipblas",
        feature = "metal",
        feature = "vulkan",
    ));
    params.use_gpu = gpu_compiled;

    let backend = if cfg!(feature = "metal") {
        "metal"
    } else if cfg!(feature = "cuda") {
        "cuda"
    } else if cfg!(feature = "coreml") {
        "coreml"
    } else if cfg!(feature = "hipblas") {
        "hipblas"
    } else if cfg!(feature = "vulkan") {
        "vulkan"
    } else {
        "cpu"
    };
    tracing::debug!(
        use_gpu = gpu_compiled,
        backend = backend,
        "whisper context params"
    );

    params
}

#[cfg(feature = "whisper")]
fn load_whisper_context(config: &Config) -> Result<whisper_rs::WhisperContext, TranscribeError> {
    let model_path = resolve_model_path(config)?;
    crate::process_trace::update_model_path(&model_path);
    crate::process_trace::stage("whisper.model_load.start");
    tracing::info!(model = %model_path.display(), "loading whisper model");

    let ctx = whisper_rs::WhisperContext::new_with_params(
        model_path
            .to_str()
            .ok_or_else(|| TranscribeError::ModelLoadError("invalid model path encoding".into()))?,
        whisper_context_params(),
    )
    .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?;
    crate::process_trace::stage("whisper.model_load.done");
    Ok(ctx)
}

#[cfg(feature = "whisper")]
fn transcribe_whisper_samples(
    ctx: &whisper_rs::WhisperContext,
    samples: &[f32],
    audio_path: &Path,
    config: &Config,
    hints: &DecodeHints,
) -> Result<TranscribeResult, TranscribeError> {
    let mut stats = FilterStats {
        audio_duration_secs: samples.len() as f64 / 16000.0,
        ..Default::default()
    };

    if samples.is_empty() {
        return Err(TranscribeError::EmptyAudio);
    }

    // Noise reduction (requires denoise feature + config enabled).
    #[cfg(feature = "denoise")]
    let samples = if config.transcription.noise_reduction {
        denoise_audio(samples, 16000)
    } else {
        samples.to_vec()
    };
    #[cfg(not(feature = "denoise"))]
    let samples = samples.to_vec();

    // If Silero VAD model is available, whisper handles silence internally via
    // integrated VAD (set in default_whisper_params). Otherwise, fall back to
    // energy-based silence stripping to prevent hallucination loops (issue #21).
    let use_integrated_vad = resolve_vad_model_path(config).is_some();

    let samples = if use_integrated_vad {
        tracing::debug!("Silero VAD available — skipping energy-based silence stripping");
        samples
    } else {
        strip_silence(&samples, 16000)
    };
    stats.samples_after_silence_strip = samples.len();

    if samples.is_empty() {
        tracing::warn!(
            audio_duration_secs = stats.audio_duration_secs,
            "silence stripping removed all audio — entire recording was below energy threshold"
        );
        return Err(TranscribeError::EmptyAudio);
    }

    let result = transcribe_with_whisper(ctx, &samples, audio_path, config, stats, false, hints)?;

    // Auto-retry without VAD if the first attempt blanked on long audio.
    // Silero VAD can be too aggressive on certain acoustic profiles or non-English
    // audio, causing whisper to see almost no speech segments. Retrying with
    // energy-based silence stripping gives whisper the full audio.
    if result.stats.final_words == 0
        && use_integrated_vad
        && result.stats.audio_duration_secs > 60.0
    {
        tracing::warn!(
            audio_secs = format!("{:.0}", result.stats.audio_duration_secs),
            raw_segments = result.stats.raw_segments,
            "blank transcript from long audio with VAD — retrying without VAD"
        );
        let mut retry_stats = FilterStats {
            audio_duration_secs: result.stats.audio_duration_secs,
            ..Default::default()
        };
        let stripped = strip_silence(&samples, 16000);
        retry_stats.samples_after_silence_strip = stripped.len();
        if !stripped.is_empty() {
            return transcribe_with_whisper(
                ctx,
                &stripped,
                audio_path,
                config,
                retry_stats,
                true,
                hints,
            );
        }
    }

    Ok(result)
}

/// Real transcription using whisper.cpp via whisper-rs.
///
/// When `force_disable_vad` is true, Silero VAD is not passed to whisper even if
/// the model exists. Used for retry after VAD-enabled transcription produced a blank.
#[cfg(feature = "whisper")]
fn transcribe_with_whisper(
    ctx: &whisper_rs::WhisperContext,
    samples: &[f32],
    _audio_path: &Path,
    config: &Config,
    mut stats: FilterStats,
    force_disable_vad: bool,
    hints: &DecodeHints,
) -> Result<TranscribeResult, TranscribeError> {
    tracing::info!(
        samples = samples.len(),
        duration_secs = samples.len() as f64 / 16000.0,
        vad_disabled = force_disable_vad,
        "starting whisper transcription"
    );

    let mut state = ctx
        .create_state()
        .map_err(|e| TranscribeError::TranscriptionFailed(format!("create state: {}", e)))?;

    // Resolve VAD model path and convert to string for FullParams lifetime.
    // Skip VAD on retry — it may have been too aggressive on this audio.
    crate::process_trace::stage("whisper.vad_setup.start");
    let vad_path = if force_disable_vad {
        None
    } else {
        resolve_vad_model_path(config)
    };
    crate::process_trace::update_vad_enabled(vad_path.is_some());
    crate::process_trace::stage("whisper.vad_setup.done");
    let vad_path_str = vad_path.as_ref().and_then(|p| p.to_str());
    let mut params = default_whisper_params(vad_path_str);
    params.set_n_threads(num_cpus());
    params.set_language(config.transcription.language.as_deref());
    params.set_token_timestamps(true);
    if let Some(initial_prompt) = hints.whisper_initial_prompt() {
        tracing::debug!(
            phrases = hints.combined_phrases(12).len(),
            "applying whisper decode hints"
        );
        params.set_initial_prompt(&initial_prompt);
    }

    // Abort callback: prevents infinite hangs on large models with problematic audio.
    // Timeout: base 5 min + 3x audio length, capped at 1 hour.
    // The small model transcribes ~15-30x faster than realtime on Apple Silicon,
    // so 3x is generous. The old 10x formula gave 7-hour timeouts for 43-min recordings.
    let audio_duration_secs = samples.len() as f64 / 16000.0;
    let timeout_secs = (300.0 + (audio_duration_secs * 3.0)).min(3600.0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(timeout_secs);
    params.set_abort_callback_safe(move || {
        let exceeded = std::time::Instant::now() > deadline;
        if exceeded {
            tracing::warn!(
                timeout_secs = format!("{:.0}", timeout_secs),
                "whisper transcription timed out — aborting"
            );
        }
        exceeded
    });

    crate::process_trace::stage_with_extra(
        "whisper.full.start",
        serde_json::json!({"timeout_secs": timeout_secs}),
    );
    state.full(params, samples).map_err(|e| {
        let msg = format!("{}", e);
        if msg.contains("abort") {
            TranscribeError::TranscriptionFailed(format!(
                "transcription timed out after {:.0}s (audio was {:.0}s). \
                     Try a smaller model or ensure Silero VAD is installed: minutes setup",
                timeout_secs, audio_duration_secs
            ))
        } else {
            TranscribeError::TranscriptionFailed(msg)
        }
    })?;
    crate::process_trace::stage("whisper.full.done");

    let num_segments = state.full_n_segments();
    stats.raw_segments = num_segments as usize;

    // Collect segments, filtering by no_speech probability.
    // We keep two lists: filtered (normal path) and rescued (all segments with text).
    // If the filter would produce 0 lines, we rescue the unfiltered segments instead
    // of producing a blank transcript — a noisy transcript beats nothing for long recordings.
    //
    // Each list has a parallel segment-data vec (start_secs, end_secs, clean_text) so
    // that after whisper-guard cleanup we can correlate surviving lines back to their
    // real centisecond-precision timestamps without re-parsing the [m:ss] prefix.
    let mut lines: Vec<String> = Vec::new();
    let mut seg_data: Vec<(f64, f64, String)> = Vec::new(); // parallel to lines
    let mut rescued_lines: Vec<String> = Vec::new();
    let mut rescued_seg_data: Vec<(f64, f64, String)> = Vec::new(); // parallel to rescued_lines
    let mut skipped_no_speech = 0u32;
    for i in 0..num_segments {
        let segment = match state.get_segment(i) {
            Some(seg) => seg,
            None => continue,
        };

        let start_ts = segment.start_timestamp();
        let end_ts = segment.end_timestamp();
        let text = segment
            .to_str_lossy()
            .map_err(|e| TranscribeError::TranscriptionFailed(format!("get text: {}", e)))?;

        let text = text.trim();
        if text.is_empty() {
            continue;
        }

        let mins = start_ts / 6000;
        let secs = (start_ts % 6000) / 100;
        let line = format!("[{}:{:02}] {}", mins, secs, text);

        // Centiseconds → seconds (centisecond precision)
        let start_secs = start_ts as f64 / 100.0;
        let end_secs = end_ts as f64 / 100.0;
        let seg_tuple = (start_secs, end_secs, text.to_string());

        // Layer 3: Skip segments with high no_speech probability (likely hallucination)
        let no_speech_prob = segment.no_speech_probability();
        if no_speech_prob > 0.8 {
            skipped_no_speech += 1;
            tracing::debug!(
                segment = i,
                no_speech_prob = format!("{:.2}", no_speech_prob),
                "flagged segment — high no_speech probability"
            );
            rescued_lines.push(line);
            rescued_seg_data.push(seg_tuple);
            continue;
        }

        rescued_lines.push(line.clone());
        rescued_seg_data.push(seg_tuple.clone());
        lines.push(line);
        seg_data.push(seg_tuple);
    }

    // Rescue: if ALL segments were filtered out but some had text, keep them.
    // This prevents blank transcripts from long recordings where the no_speech
    // detector was wrong (common with non-English audio or unusual acoustics).
    if lines.is_empty() && !rescued_lines.is_empty() {
        let rescued_count = rescued_lines.len();
        tracing::warn!(
            rescued = rescued_count,
            skipped_no_speech = skipped_no_speech,
            audio_secs = format!("{:.0}", stats.audio_duration_secs),
            "no_speech filter would blank the transcript — rescuing all segments"
        );
        lines = rescued_lines;
        seg_data = rescued_seg_data;
        stats.rescued_no_speech = rescued_count;
        // Don't count these as skipped since we rescued them
        skipped_no_speech = 0;
    }

    stats.skipped_no_speech = skipped_no_speech as usize;
    stats.after_no_speech_filter = lines.len();

    if skipped_no_speech > 0 {
        tracing::info!(
            skipped = skipped_no_speech,
            remaining = lines.len(),
            "filtered segments with high no_speech probability"
        );
    }

    // Capture detected language before cleanup consumes the state.
    // full_lang_id_from_state() returns -1 when no language was set or detected.
    let detected_language = {
        let lang_id = state.full_lang_id_from_state();
        if lang_id >= 0 {
            whisper_rs::get_lang_str(lang_id).map(str::to_owned)
        } else {
            None
        }
    };

    let cleanup = run_transcript_cleanup_pipeline(lines.clone());
    stats.after_dedup = cleanup.after(TranscriptCleanupStage::DedupSegments);
    stats.after_interleaved = cleanup.after(TranscriptCleanupStage::DedupInterleaved);
    stats.after_script_filter = cleanup.after(TranscriptCleanupStage::StripForeignScript);
    stats.after_noise_markers = cleanup.after(TranscriptCleanupStage::CollapseNoiseMarkers);
    stats.after_trailing_trim = cleanup.after(TranscriptCleanupStage::TrimTrailingNoise);

    // Correlate surviving lines back to their real timestamps
    let segments = correlate_whisper_segments(&cleanup.lines, &lines, &seg_data);

    let surviving_lines = cleanup.lines;
    let transcript = surviving_lines.join("\n");
    let transcript = if transcript.is_empty() {
        transcript
    } else {
        format!("{}\n", transcript)
    };

    let word_count = transcript.split_whitespace().count();
    stats.final_words = word_count;

    tracing::info!(
        segments = num_segments,
        words = word_count,
        diagnosis = stats.diagnosis(),
        "transcription complete"
    );

    if word_count == 0 && num_segments > 0 {
        tracing::warn!(
            diagnosis = stats.diagnosis(),
            "all segments filtered out — transcript is blank"
        );
    }

    Ok(TranscribeResult {
        text: transcript,
        stats,
        segments,
        detected_language,
    })
}

/// Load audio from any supported format as 16kHz mono f32 samples.
///
/// For non-WAV formats (m4a, mp3, ogg, etc.), prefers ffmpeg when available
/// because symphonia's AAC decoder produces samples that cause whisper to
/// hallucinate on non-English audio (issue #21). Falls back to symphonia
/// when ffmpeg is not installed.
fn load_audio_samples(path: &Path) -> Result<Vec<f32>, TranscribeError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let samples = match ext.as_str() {
        "wav" => {
            crate::process_trace::stage_with_extra(
                "decode.start",
                serde_json::json!({"decoder": "wav"}),
            );
            let samples = load_wav(path)?;
            crate::process_trace::update_audio(samples.len());
            crate::process_trace::stage_with_extra(
                "decode.done",
                serde_json::json!({"decoder": "wav"}),
            );
            samples
        }
        "m4a" | "mp3" | "ogg" | "webm" | "mp4" | "mov" | "aac" => {
            // Prefer ffmpeg — its resampler and AAC decoder produce samples that
            // whisper transcribes correctly across all languages. Symphonia's AAC
            // decoder produces subtly different samples that trigger hallucination
            // loops on non-English audio (confirmed in issue #21).
            crate::process_trace::stage_with_extra(
                "decode.start",
                serde_json::json!({"decoder": "ffmpeg"}),
            );
            match decode_with_ffmpeg(path) {
                Ok(samples) => {
                    crate::process_trace::update_audio(samples.len());
                    crate::process_trace::stage_with_extra(
                        "decode.done",
                        serde_json::json!({"decoder": "ffmpeg"}),
                    );
                    samples
                }
                Err(e) => {
                    let is_not_found = e.to_string().contains("not available")
                        || e.to_string().contains("not found");
                    if is_not_found {
                        tracing::warn!(
                            "ffmpeg not found — falling back to symphonia for {} decoding. \
                             Non-English audio may produce poor results. \
                             Install ffmpeg: brew install ffmpeg (macOS) / apt install ffmpeg (Linux)",
                            ext
                        );
                    } else {
                        tracing::warn!(
                            error = %e,
                            "ffmpeg decode failed — falling back to symphonia"
                        );
                    }
                    crate::process_trace::stage_with_extra(
                        "decode.start",
                        serde_json::json!({"decoder": "symphonia", "fallback_from": "ffmpeg"}),
                    );
                    let samples = decode_with_symphonia(path)?;
                    crate::process_trace::update_audio(samples.len());
                    crate::process_trace::stage_with_extra(
                        "decode.done",
                        serde_json::json!({"decoder": "symphonia", "fallback_from": "ffmpeg"}),
                    );
                    samples
                }
            }
        }
        other => return Err(TranscribeError::UnsupportedFormat(other.to_string())),
    };
    Ok(samples)
}

/// Load WAV file as f32 samples, converting to 16kHz mono if needed.
fn load_wav(path: &Path) -> Result<Vec<f32>, TranscribeError> {
    let reader = hound::WavReader::open(path).map_err(|e| {
        if e.to_string().contains("Not a WAVE file") || e.to_string().contains("unexpected EOF") {
            TranscribeError::UnsupportedFormat("corrupt or invalid WAV file".into())
        } else {
            TranscribeError::Io(std::io::Error::other(e.to_string()))
        }
    })?;

    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let channels = spec.channels as usize;

    // Read all samples as f32, normalizing by actual bit depth
    let bits = spec.bits_per_sample;
    let max_val = (1_i64 << (bits - 1)) as f32; // e.g. 16-bit → 32768.0
    let raw_samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .into_samples::<i32>()
            .filter_map(|s| s.ok())
            .map(|s| s as f32 / max_val)
            .collect(),
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
    };

    if raw_samples.is_empty() {
        return Err(TranscribeError::EmptyAudio);
    }

    // Convert to mono
    let mono = if channels > 1 {
        raw_samples
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        raw_samples
    };

    // Resample to 16kHz if needed
    let resampled = if sample_rate != 16000 {
        resample(&mono, sample_rate, 16000)
    } else {
        mono
    };

    // Auto-normalize: if peak is below target, boost so whisper gets usable levels.
    // Quiet mics (e.g. MacBook Pro) can produce peaks of 0.004 which whisper can't detect.
    Ok(normalize_audio(&resampled))
}

/// Decode audio with ffmpeg (preferred for non-WAV formats).
///
/// Shells out to `ffmpeg` to convert any audio to 16kHz mono f32le PCM.
/// This matches exactly what whisper-cli does and produces samples that
/// whisper transcribes correctly across all languages.
///
/// Returns an error if ffmpeg is not installed or the conversion fails,
/// allowing the caller to fall back to symphonia.
fn decode_with_ffmpeg(path: &Path) -> Result<Vec<f32>, TranscribeError> {
    let tmp_dir = std::env::temp_dir();
    let tmp_wav = tmp_dir.join(format!("minutes-ffmpeg-{}.wav", std::process::id()));

    // Pre-create temp file with restrictive permissions (contains raw audio)
    #[cfg(unix)]
    {
        // Touch the file so we can set permissions before ffmpeg writes to it
        if let Ok(f) = std::fs::File::create(&tmp_wav) {
            drop(f);
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp_wav, std::fs::Permissions::from_mode(0o600)).ok();
        }
    }

    let ffmpeg = crate::ffmpeg::resolve_ffmpeg()
        .map_err(|e| TranscribeError::TranscriptionFailed(format!("ffmpeg not available: {e}")))?;
    let output = std::process::Command::new(&ffmpeg)
        .args([
            "-i",
            path.to_str().unwrap_or(""),
            "-ar",
            "16000", // 16kHz sample rate
            "-ac",
            "1", // mono
            "-f",
            "wav", // WAV output
            "-y",  // overwrite
        ])
        .arg(&tmp_wav)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| {
            TranscribeError::TranscriptionFailed(format!(
                "ffmpeg not available at {}: {}",
                ffmpeg.display(),
                e
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Clean up temp file on failure
        let _ = std::fs::remove_file(&tmp_wav);
        return Err(TranscribeError::TranscriptionFailed(format!(
            "ffmpeg conversion failed: {}",
            stderr.lines().last().unwrap_or("unknown error")
        )));
    }

    tracing::info!(
        source = %path.display(),
        "decoded audio with ffmpeg (16kHz mono WAV)"
    );

    // Load the ffmpeg-produced WAV (already 16kHz mono)
    let result = load_wav(&tmp_wav);

    // Clean up temp file
    let _ = std::fs::remove_file(&tmp_wav);

    result
}

/// Decode audio with symphonia (handles m4a, mp3, ogg, etc.)
/// Outputs 16kHz mono f32 samples.
fn decode_with_symphonia(path: &Path) -> Result<Vec<f32>, TranscribeError> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| TranscribeError::UnsupportedFormat(format!("probe failed: {}", e)))?;

    let mut format = probed.format;

    // Find the first audio track
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| TranscribeError::UnsupportedFormat("no audio track found".into()))?;

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);

    let decoder_opts = DecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &decoder_opts)
        .map_err(|e| TranscribeError::UnsupportedFormat(format!("decoder: {}", e)))?;

    let mut all_samples: Vec<f32> = Vec::new();

    // Decode all packets
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break; // End of stream
            }
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(_) => continue, // Skip bad packets
        };

        let spec = *decoded.spec();
        let duration = decoded.capacity();

        let mut sample_buf = SampleBuffer::<f32>::new(duration as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);

        let samples = sample_buf.samples();

        // Convert to mono if needed
        if channels > 1 {
            for chunk in samples.chunks(channels) {
                let mono_sample = chunk.iter().sum::<f32>() / channels as f32;
                all_samples.push(mono_sample);
            }
        } else {
            all_samples.extend_from_slice(samples);
        }
    }

    if all_samples.is_empty() {
        return Err(TranscribeError::EmptyAudio);
    }

    // Resample to 16kHz if needed
    let resampled = if sample_rate != 16000 {
        resample(&all_samples, sample_rate, 16000)
    } else {
        all_samples
    };

    Ok(normalize_audio(&resampled))
}

// resample() and normalize_audio() are provided by whisper_guard::audio
// and re-exported at the top of this file.

fn detect_meeting_vad_chunks(samples: &[f32]) -> Vec<(usize, usize)> {
    const SAMPLE_RATE: usize = 16_000;
    const WINDOW_SAMPLES: usize = 1_600; // 100ms
    const ROTATE_SILENCE_MS: u64 = 800;
    const MIN_CHUNK_SAMPLES: usize = SAMPLE_RATE; // 1s
    const MAX_CHUNK_SAMPLES: usize = SAMPLE_RATE * 45;
    const NOISE_FLOOR_MIN: f32 = 0.0001;
    const NOISE_FLOOR_MAX: f32 = 0.02;
    const NOISE_MULTIPLIER: f32 = 4.0;
    const HANGOVER_CHUNKS: u32 = 5;
    const ADAPT_RATE: f32 = 0.02;

    if samples.is_empty() {
        return Vec::new();
    }

    let mut noise_floor = 0.001f32;
    let mut speaking = false;
    let mut hangover_remaining = 0u32;
    let mut silence_ms = 0u64;
    let mut chunks = Vec::new();
    let mut chunk_start: Option<usize> = None;
    let mut last_voice_end = 0usize;

    for (window_index, window) in samples.chunks(WINDOW_SAMPLES).enumerate() {
        let window_start = window_index * WINDOW_SAMPLES;
        let window_end = (window_start + window.len()).min(samples.len());
        let rms = (window
            .iter()
            .map(|sample| (*sample as f64) * (*sample as f64))
            .sum::<f64>()
            / window.len().max(1) as f64)
            .sqrt() as f32;

        let threshold = noise_floor * NOISE_MULTIPLIER;
        if rms > threshold {
            speaking = true;
            hangover_remaining = HANGOVER_CHUNKS;
            silence_ms = 0;
        } else if hangover_remaining > 0 {
            hangover_remaining -= 1;
            silence_ms = 0;
        } else {
            speaking = false;
            silence_ms += 100;
            if rms > noise_floor {
                noise_floor += (rms - noise_floor) * ADAPT_RATE;
            } else {
                noise_floor += (rms - noise_floor) * (ADAPT_RATE * 3.0);
            }
            noise_floor = noise_floor.clamp(NOISE_FLOOR_MIN, NOISE_FLOOR_MAX);
        }

        if speaking {
            if chunk_start.is_none() {
                chunk_start = Some(window_start);
            }
            last_voice_end = window_end;
        }

        if let Some(start) = chunk_start {
            let chunk_len = last_voice_end.saturating_sub(start);
            if chunk_len >= MAX_CHUNK_SAMPLES {
                chunks.push((start, last_voice_end.max(window_end)));
                chunk_start = None;
                last_voice_end = 0;
                noise_floor = 0.001;
                speaking = false;
                hangover_remaining = 0;
                silence_ms = 0;
                continue;
            }

            if !speaking && silence_ms >= ROTATE_SILENCE_MS && chunk_len >= MIN_CHUNK_SAMPLES {
                chunks.push((start, last_voice_end));
                chunk_start = None;
                last_voice_end = 0;
                noise_floor = 0.001;
                speaking = false;
                hangover_remaining = 0;
                silence_ms = 0;
            }
        }
    }

    if let Some(start) = chunk_start {
        let end = if last_voice_end > start {
            last_voice_end
        } else {
            samples.len()
        };
        if end > start {
            chunks.push((start, end));
        }
    }

    chunks
}

fn offset_timestamped_lines<'a>(
    lines: impl Iterator<Item = &'a str>,
    offset_secs: f64,
    _chunk_index: usize,
) -> Vec<String> {
    lines
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix('[')?;
            let close = rest.find(']')?;
            let timestamp = &rest[..close];
            let text = rest[close + 1..].trim();
            let (mins, secs) = timestamp.split_once(':')?;
            let total_secs = mins.parse::<u64>().ok()? * 60 + secs.parse::<u64>().ok()?;
            let adjusted = total_secs as f64 + offset_secs;
            let adjusted_mins = (adjusted / 60.0).floor() as u64;
            let adjusted_secs = (adjusted % 60.0).floor() as u64;
            Some(format!("[{}:{:02}] {}", adjusted_mins, adjusted_secs, text))
        })
        .collect()
}

// ── Noise reduction ──────────────────────────────────────────

/// Apply RNNoise-based noise reduction to audio samples.
///
/// nnnoiseless requires 48kHz f32 audio in 480-sample frames with values
/// in i16 range (-32768 to 32767). This function handles resampling to/from
/// 48kHz and the scaling automatically.
///
/// Primes the DenoiseState with a silence frame to avoid first-frame
/// fade-in artifacts.
#[cfg(feature = "denoise")]
fn denoise_audio(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    use nnnoiseless::{DenoiseState, FRAME_SIZE};

    if samples.is_empty() {
        return samples.to_vec();
    }

    // Resample to 48kHz if needed (nnnoiseless requires exactly 48kHz)
    let (samples_48k, original_rate) = if sample_rate != 48000 {
        (resample(samples, sample_rate, 48000), Some(sample_rate))
    } else {
        (samples.to_vec(), None)
    };

    // Scale to i16 range as nnnoiseless expects
    let scaled: Vec<f32> = samples_48k.iter().map(|s| s * 32767.0).collect();

    let mut state = DenoiseState::new();
    let mut output = Vec::with_capacity(scaled.len());
    let mut frame_out = [0.0f32; FRAME_SIZE];

    // Prime with a silence frame to avoid first-frame fade-in artifact
    let silence = [0.0f32; FRAME_SIZE];
    state.process_frame(&mut frame_out, &silence);

    for chunk in scaled.chunks(FRAME_SIZE) {
        if chunk.len() == FRAME_SIZE {
            state.process_frame(&mut frame_out, chunk);
            output.extend_from_slice(&frame_out);
        } else {
            // Pad last frame with zeros
            let mut padded = [0.0f32; FRAME_SIZE];
            padded[..chunk.len()].copy_from_slice(chunk);
            state.process_frame(&mut frame_out, &padded);
            output.extend_from_slice(&frame_out[..chunk.len()]);
        }
    }

    // Scale back to -1.0..1.0 range
    let denoised: Vec<f32> = output.iter().map(|s| s / 32767.0).collect();

    // Resample back to original rate if we upsampled
    let denoised = if let Some(orig) = original_rate {
        resample(&denoised, 48000, orig)
    } else {
        denoised
    };

    let original_rms: f32 =
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    let denoised_rms: f32 =
        (denoised.iter().map(|s| s * s).sum::<f32>() / denoised.len() as f32).sqrt();

    tracing::info!(
        original_rms = format!("{:.4}", original_rms),
        denoised_rms = format!("{:.4}", denoised_rms),
        reduction_db = format!(
            "{:.1}",
            20.0 * (denoised_rms / original_rms.max(0.0001)).log10()
        ),
        "noise reduction applied"
    );

    denoised
}

/// Conservative lower bound on the on-disk size of each whisper.cpp ggml
/// model from `huggingface.co/ggerganov/whisper.cpp`. Used to detect
/// truncated downloads (issue #229: an interrupted download left a
/// `ggml-medium.bin` at 221 MB instead of ~1.5 GB; `minutes setup` happily
/// reported it as "already downloaded" and whisper-rs aborted parsing the
/// truncated GGML header on the next transcription).
///
/// Numbers are set at roughly 90% of the canonical artifact size so a
/// legitimate file passes comfortably and only a partial download fails
/// the check. Out of scope here: per-model SHA256 manifests, which would
/// catch corruption as well as truncation.
///
/// Returns `None` for unknown / custom model names; callers should treat
/// that as "no check available" and skip size validation.
pub fn expected_whisper_model_size_bytes(model_name: &str) -> Option<u64> {
    // Real Content-Length values from the canonical URL the CLI downloads
    // (`huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{name}.bin`)
    // as of 2026-05-11:
    //   tiny      ~74.1 MB
    //   base     ~141.1 MB
    //   small    ~465.0 MB
    //   medium  ~1462.7 MB
    //   large-v3 ~2951.7 MB
    // Thresholds sit roughly 5% below those numbers so a real file passes
    // and only a meaningfully truncated file is rejected.
    //
    // Quantized variants (`ggml-medium-q5_0.bin` and friends) have
    // different sizes and intentionally aren't in this table; the parser
    // in `validate_whisper_model_size` strips only the canonical
    // `ggml-{name}.bin` shape, so quantized files resolve to a name like
    // `medium-q5_0` that hits the `None` arm and skips validation.
    match model_name {
        "tiny" | "tiny.en" => Some(70 * 1024 * 1024),
        "base" | "base.en" => Some(135 * 1024 * 1024),
        "small" | "small.en" => Some(440 * 1024 * 1024),
        "medium" | "medium.en" => Some(1_400 * 1024 * 1024),
        "large-v1" | "large-v2" | "large-v3" | "large" => Some(2_800 * 1024 * 1024),
        _ => None,
    }
}

/// If the resolved path looks like a known `ggml-{name}.bin` artifact,
/// check its on-disk size against `expected_whisper_model_size_bytes` and
/// return `Err(ModelTruncated { .. })` if it falls short. Otherwise
/// returns `Ok(path)` unchanged.
///
/// We only validate when the filename matches the canonical pattern so a
/// user-supplied custom model path (e.g. a fine-tune in a non-standard
/// location) is never falsely rejected.
#[cfg(feature = "whisper")]
fn validate_whisper_model_size(path: PathBuf) -> Result<PathBuf, TranscribeError> {
    let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
        return Ok(path);
    };
    let model_name = match file_name
        .strip_prefix("ggml-")
        .and_then(|rest| rest.strip_suffix(".bin"))
    {
        Some(name) => name,
        None => return Ok(path),
    };
    let Some(expected_min) = expected_whisper_model_size_bytes(model_name) else {
        return Ok(path);
    };
    let metadata = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return Ok(path),
    };
    let actual = metadata.len();
    if actual >= expected_min {
        return Ok(path);
    }
    Err(TranscribeError::ModelTruncated {
        path: path.display().to_string(),
        model_name: model_name.to_string(),
        actual_mb: actual as f64 / (1024.0 * 1024.0),
        expected_min_mb: expected_min as f64 / (1024.0 * 1024.0),
    })
}

/// Resolve the whisper model file path for dictation (uses dictation.model config).
#[cfg(feature = "whisper")]
pub fn resolve_model_path_for_dictation(config: &Config) -> Result<PathBuf, TranscribeError> {
    let model_name = &config.dictation.model;
    let model_dir = &config.transcription.model_path;

    let candidates = [
        model_dir.join(format!("ggml-{}.bin", model_name)),
        model_dir.join(format!("whisper-{}.bin", model_name)),
        model_dir.join(format!("{}.bin", model_name)),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return validate_whisper_model_size(candidate.clone());
        }
    }

    let direct = PathBuf::from(model_name);
    if direct.exists() {
        return validate_whisper_model_size(direct);
    }

    Err(TranscribeError::ModelNotFound(format!(
        "Expected model file \"ggml-{}.bin\" in {}.\n\nTo fix this, run:\n\n    minutes setup --model {}\n",
        model_name,
        model_dir.display(),
        model_name,
    )))
}

/// Resolve a whisper model file path by explicit model name.
/// Falls back to the dictation model if the given name doesn't resolve.
#[cfg(feature = "whisper")]
pub fn resolve_model_path_by_name(
    model_name: &str,
    config: &Config,
) -> Result<PathBuf, TranscribeError> {
    let model_dir = &config.transcription.model_path;

    let candidates = [
        model_dir.join(format!("ggml-{}.bin", model_name)),
        model_dir.join(format!("whisper-{}.bin", model_name)),
        model_dir.join(format!("{}.bin", model_name)),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return validate_whisper_model_size(candidate.clone());
        }
    }

    let direct = PathBuf::from(model_name);
    if direct.exists() {
        return validate_whisper_model_size(direct);
    }

    // Fall back to dictation model with a warning
    let model_dir_display = model_dir.display().to_string();
    let requested = model_name.to_string();
    let dictation_model = &config.dictation.model;
    tracing::warn!(
        requested = %requested,
        fallback = %dictation_model,
        "live transcript model not found, falling back to dictation model"
    );
    resolve_model_path_for_dictation(config).map_err(|_| {
        TranscribeError::ModelNotFound(format!(
            "Expected model file \"ggml-{}.bin\" in {}.\n\nTo fix this, run:\n\n    minutes setup --model {}\n",
            requested, model_dir_display, requested,
        ))
    })
}

/// Resolve the whisper model file path.
#[cfg(feature = "whisper")]
fn resolve_model_path(config: &Config) -> Result<PathBuf, TranscribeError> {
    let model_name = &config.transcription.model;
    let model_dir = &config.transcription.model_path;

    // Try common naming patterns
    let candidates = [
        model_dir.join(format!("ggml-{}.bin", model_name)),
        model_dir.join(format!("whisper-{}.bin", model_name)),
        model_dir.join(format!("{}.bin", model_name)),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return validate_whisper_model_size(candidate.clone());
        }
    }

    // If model_name is an absolute path, try it directly
    let direct = PathBuf::from(model_name);
    if direct.exists() {
        return validate_whisper_model_size(direct);
    }

    Err(TranscribeError::ModelNotFound(format!(
        "Expected model file \"ggml-{}.bin\" in {}.\n\nTo fix this, run:\n\n    minutes setup --model {}\n",
        model_name,
        model_dir.display(),
        model_name,
    )))
}

/// Resolve the Silero VAD model path. Returns None if VAD is disabled or model not found.
#[cfg(feature = "whisper")]
pub(crate) fn resolve_vad_model_path(config: &Config) -> Option<PathBuf> {
    let vad_model = &config.transcription.vad_model;
    if vad_model.is_empty() {
        return None;
    }

    let model_dir = &config.transcription.model_path;
    let mut candidates = vec![
        model_dir.join(format!("ggml-{}.bin", vad_model)),
        model_dir.join(format!("{}.bin", vad_model)),
    ];
    // Fallback: accept old "ggml-silero-vad.bin" name for backward compatibility,
    // but only when the config is using a silero-variant name (the default).
    if vad_model.starts_with("silero") {
        candidates.push(model_dir.join("ggml-silero-vad.bin"));
    }

    for candidate in &candidates {
        if candidate.exists() {
            return Some(candidate.clone());
        }
    }

    // Try as absolute path
    let direct = PathBuf::from(vad_model);
    if direct.exists() {
        return Some(direct);
    }

    tracing::debug!(
        vad_model = vad_model,
        "VAD model not found — falling back to energy-based silence stripping"
    );
    None
}

/// Resolve the Silero ONNX path used by the streaming `OrtSileroVad`
/// engine. Returns None if the file is missing — caller falls back to
/// whisper-Silero with an explicit warn-level log so the user knows
/// they opted into ort-silero but didn't get it.
///
/// The file name mirrors `vad_model` (the ggml form) but with `.onnx`
/// extension, so a single `vad_model = "silero-v6.2.0"` config drives
/// both engines' resolution.
#[cfg(all(feature = "whisper", feature = "vad-ort"))]
pub(crate) fn resolve_silero_onnx_path(config: &Config) -> Option<PathBuf> {
    let vad_model = &config.transcription.vad_model;
    if vad_model.is_empty() {
        return None;
    }
    let model_dir = &config.transcription.model_path;
    let candidates = [
        model_dir.join(format!("{}.onnx", vad_model)),
        model_dir.join("silero-vad-v6.2.0.onnx"),
        model_dir.join("silero_vad.onnx"),
    ];
    for candidate in &candidates {
        if candidate.exists() {
            return Some(candidate.clone());
        }
    }
    None
}

// default_whisper_params, streaming_whisper_params, and num_cpus
// are re-exported from whisper_guard::params via `pub use` at the top of this file.
#[cfg(feature = "whisper")]
fn num_cpus() -> i32 {
    whisper_guard::params::num_cpus()
}

// ──────────────────────────────────────────────────────────────
// Parakeet engine (subprocess-based)
//
// Shells out to parakeet.cpp CLI, parses text output with
// line-level timestamps, formats as [M:SS] lines to match
// whisper output exactly. Pipeline/diarization/summarization
// all work unchanged.
// ──────────────────────────────────────────────────────────────

/// Transcribe using parakeet.cpp as a subprocess.
#[cfg(feature = "parakeet")]
fn transcribe_with_parakeet(
    audio_path: &Path,
    config: &Config,
    hints: &DecodeHints,
) -> Result<TranscribeResult, TranscribeError> {
    let mut stats = FilterStats::default();

    // Validate model name before doing any work
    if !crate::parakeet::valid_model(&config.transcription.parakeet_model) {
        return Err(TranscribeError::ParakeetFailed(format!(
            "unknown parakeet model '{}'. Valid: {}",
            config.transcription.parakeet_model,
            crate::config::VALID_PARAKEET_MODELS.join(", ")
        )));
    }

    // Step 1: Load audio and convert to 16kHz mono (reuse existing pipeline)
    let samples = load_audio_samples(audio_path)?;
    stats.audio_duration_secs = samples.len() as f64 / 16000.0;
    if samples.is_empty() {
        return Err(TranscribeError::EmptyAudio);
    }

    // Noise reduction is not yet supported for parakeet — warn if configured
    if config.transcription.noise_reduction {
        tracing::debug!(
            "noise_reduction is enabled but not applied for parakeet engine \
             (nnnoiseless only supports the whisper path)"
        );
    }

    let native_vad_path = resolve_parakeet_native_vad_path(config);
    let samples = if native_vad_path.is_some() {
        tracing::debug!("native parakeet VAD available — skipping energy-based silence stripping");
        samples
    } else {
        strip_silence(&samples, 16000)
    };
    stats.samples_after_silence_strip = samples.len();
    if samples.is_empty() {
        return Err(TranscribeError::EmptyAudio);
    }

    // Step 2: Write samples to temp WAV (NamedTempFile avoids PID collisions
    // when the watcher processes multiple files concurrently)
    let tmp_wav = tempfile::Builder::new()
        .prefix("minutes-parakeet-")
        .suffix(".wav")
        .tempfile()
        .map_err(TranscribeError::Io)?;
    write_wav_16k_mono(tmp_wav.path(), &samples)?;

    // Step 3: Resolve model and vocab paths
    let model_path = resolve_parakeet_model_path(config)?;
    let vocab_path = resolve_parakeet_vocab_path(config)?;
    let sidecar_audio_duration_secs = samples.len() as f64 / 16000.0;
    let resolved_binary = crate::parakeet::resolve_parakeet_binary(
        &config.transcription.parakeet_binary,
        crate::parakeet::ResolveParakeetBinaryMode::WarnAndFallback,
    )
    .map_err(|error| TranscribeError::ParakeetFailed(error.to_string()))?;
    let resolved_binary_str = resolved_binary.to_str().ok_or_else(|| {
        TranscribeError::ParakeetFailed("resolved parakeet binary path is not valid UTF-8".into())
    })?;

    if crate::parakeet_sidecar::sidecar_enabled_effective(config) {
        match crate::parakeet_sidecar::transcribe_via_global_sidecar(
            config,
            &model_path,
            &vocab_path,
            native_vad_path.as_deref(),
            tmp_wav.path(),
            sidecar_audio_duration_secs,
            hints,
        ) {
            Ok(result) => {
                tracing::info!(
                    "parakeet-sidecar: using warm server path elapsed_ms={} first_request={} fp16={}",
                    result.elapsed_ms,
                    result.first_request_on_process,
                    result.effective_fp16
                );
                return transcribe_result_from_parakeet_parsed(
                    result.transcript,
                    stats,
                    result.first_request_on_process,
                    result.elapsed_ms,
                    config,
                );
            }
            Err(error) => {
                tracing::warn!(
                    "parakeet-sidecar: falling back to subprocess path: {}",
                    error
                );
            }
        }
    }

    // Step 4: Run parakeet subprocess
    // CLI syntax: parakeet <model.safetensors> <audio.wav> --vocab <tokenizer.vocab>
    // [--model type] [--timestamps] [--gpu]
    tracing::info!(
        binary = %resolved_binary.display(),
        model = %model_path.display(),
        vocab = %vocab_path.display(),
        audio = %audio_path.display(),
        "starting parakeet transcription"
    );

    let invocation_started = Instant::now();
    let host_process_key = format!(
        "{}::{}",
        resolved_binary.display(),
        config.transcription.parakeet_model.as_str()
    );
    let host_process_first_use = {
        let mut seen = parakeet_seen_models()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        seen.insert(host_process_key)
    };

    let use_gpu = cfg!(all(target_os = "macos", target_arch = "aarch64"));
    let use_fp16 = use_gpu && config.transcription.parakeet_fp16;
    let helper_allowed = hints.is_empty()
        && std::env::var_os("MINUTES_PARAKEET_FORCE_DIRECT").is_none()
        && std::env::var_os("MINUTES_PARAKEET_HELPER_ACTIVE").is_none();
    let parsed = if helper_allowed {
        if let Some(helper_path) = resolve_minutes_parakeet_helper() {
            let mut helper_command = std::process::Command::new(helper_path);
            helper_command
                .arg("parakeet-helper")
                .args(["--binary", resolved_binary_str])
                .args([
                    "--model-path",
                    model_path.to_str().ok_or_else(|| {
                        TranscribeError::ParakeetFailed("model path is not valid UTF-8".into())
                    })?,
                ])
                .args([
                    "--audio-path",
                    tmp_wav.path().to_str().ok_or_else(|| {
                        TranscribeError::ParakeetFailed("temp WAV path is not valid UTF-8".into())
                    })?,
                ])
                .args([
                    "--vocab-path",
                    vocab_path.to_str().ok_or_else(|| {
                        TranscribeError::ParakeetFailed("vocab path is not valid UTF-8".into())
                    })?,
                ])
                .args(["--model-id", &config.transcription.parakeet_model])
                .args(if use_gpu { vec!["--gpu"] } else { Vec::new() })
                .args(if use_fp16 { vec!["--fp16"] } else { Vec::new() })
                .env("MINUTES_PARAKEET_HELPER_ACTIVE", "1")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            if let Some(vad_path) = native_vad_path.as_ref().and_then(|path| path.to_str()) {
                helper_command.args(["--vad-path", vad_path]).args([
                    "--vad-threshold",
                    &PARAKEET_NATIVE_VAD_THRESHOLD.to_string(),
                ]);
            }
            let helper_output = helper_command.output();

            match helper_output {
                Ok(output) if output.status.success() => serde_json::from_slice(&output.stdout)
                    .map_err(|error| {
                        TranscribeError::ParakeetFailed(format!(
                            "failed to parse helper JSON output: {}",
                            error
                        ))
                    })?,
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    log_parakeet_helper_failure_once(&output.status, &stderr);
                    match run_parakeet_cli_structured(
                        resolved_binary_str,
                        &model_path,
                        tmp_wav.path(),
                        &vocab_path,
                        &config.transcription.parakeet_model,
                        use_gpu,
                        native_vad_path.as_deref(),
                        PARAKEET_NATIVE_VAD_THRESHOLD,
                        config,
                        hints,
                    ) {
                        Ok(parsed) => parsed,
                        Err(error @ TranscribeError::EmptyAudio)
                        | Err(error @ TranscribeError::EmptyTranscript(_)) => {
                            return Err(error);
                        }
                        Err(_) => {
                            return Err(TranscribeError::ParakeetFailed(
                                stderr
                                    .lines()
                                    .last()
                                    .unwrap_or("unknown helper error")
                                    .to_string(),
                            ));
                        }
                    }
                }
                Err(spawn_error) => {
                    log_parakeet_helper_spawn_failure_once(&spawn_error);
                    run_parakeet_cli_structured(
                        resolved_binary_str,
                        &model_path,
                        tmp_wav.path(),
                        &vocab_path,
                        &config.transcription.parakeet_model,
                        use_gpu,
                        native_vad_path.as_deref(),
                        PARAKEET_NATIVE_VAD_THRESHOLD,
                        config,
                        hints,
                    )?
                }
            }
        } else {
            run_parakeet_cli_structured(
                resolved_binary_str,
                &model_path,
                tmp_wav.path(),
                &vocab_path,
                &config.transcription.parakeet_model,
                use_gpu,
                native_vad_path.as_deref(),
                PARAKEET_NATIVE_VAD_THRESHOLD,
                config,
                hints,
            )?
        }
    } else {
        run_parakeet_cli_structured(
            resolved_binary_str,
            &model_path,
            tmp_wav.path(),
            &vocab_path,
            &config.transcription.parakeet_model,
            use_gpu,
            native_vad_path.as_deref(),
            PARAKEET_NATIVE_VAD_THRESHOLD,
            config,
            hints,
        )?
    };
    let elapsed_ms = invocation_started.elapsed().as_millis() as u64;
    transcribe_result_from_parakeet_parsed(
        parsed,
        stats,
        host_process_first_use,
        elapsed_ms,
        config,
    )
}

/// Parse parakeet.cpp text output into `[M:SS] text` lines matching whisper format.
///
/// parakeet.cpp with `--timestamps` outputs lines like:
///   `[0.00 - 2.50] Hello world`
///   `[2.80 - 5.10] How are you`
///
/// Applies the full anti-hallucination pipeline: dedup_segments, dedup_interleaved,
/// and trim_trailing_noise — matching the whisper path exactly.
#[cfg(feature = "parakeet")]
#[derive(Debug)]
struct ParakeetFilterStats {
    raw_segments: usize,
    after_dedup: usize,
    after_interleaved: usize,
    after_script_filter: usize,
    after_noise_markers: usize,
    after_trailing_trim: usize,
}

#[cfg(feature = "parakeet")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParakeetCliSegment {
    pub start_secs: f64,
    pub end_secs: f64,
    pub confidence: Option<f32>,
    pub text: String,
}

#[cfg(feature = "parakeet")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParakeetCliTranscript {
    pub raw_output: String,
    pub segments: Vec<ParakeetCliSegment>,
    pub transcript: String,
}

#[cfg(feature = "parakeet")]
fn transcribe_result_from_parakeet_parsed(
    parsed: ParakeetCliTranscript,
    mut stats: FilterStats,
    host_process_first_use: bool,
    elapsed_ms: u64,
    config: &Config,
) -> Result<TranscribeResult, TranscribeError> {
    let transcript = parsed.transcript.clone();
    let pstats = {
        let raw_segments = parsed.segments.len();
        let lines: Vec<String> = parsed
            .segments
            .iter()
            .map(|segment| {
                let mins = (segment.start_secs / 60.0) as u64;
                let secs = (segment.start_secs % 60.0) as u64;
                format!("[{}:{:02}] {}", mins, secs, segment.text)
            })
            .collect();
        let cleanup = run_transcript_cleanup_pipeline(lines);
        ParakeetFilterStats {
            raw_segments,
            after_dedup: cleanup.after(TranscriptCleanupStage::DedupSegments),
            after_interleaved: cleanup.after(TranscriptCleanupStage::DedupInterleaved),
            after_script_filter: cleanup.after(TranscriptCleanupStage::StripForeignScript),
            after_noise_markers: cleanup.after(TranscriptCleanupStage::CollapseNoiseMarkers),
            after_trailing_trim: cleanup.after(TranscriptCleanupStage::TrimTrailingNoise),
        }
    };

    stats.raw_segments = pstats.raw_segments;
    stats.after_no_speech_filter = pstats.raw_segments;
    stats.after_dedup = pstats.after_dedup;
    stats.after_interleaved = pstats.after_interleaved;
    stats.after_script_filter = pstats.after_script_filter;
    stats.after_noise_markers = pstats.after_noise_markers;
    stats.after_trailing_trim = pstats.after_trailing_trim;

    let word_count = transcript.split_whitespace().count();
    stats.final_words = word_count;
    tracing::info!(
        words = word_count,
        segments = parsed.segments.len(),
        host_process_first_use,
        elapsed_ms,
        diagnosis = stats.diagnosis(),
        "parakeet transcription complete"
    );
    if let (Some(first), Some(last)) = (parsed.segments.first(), parsed.segments.last()) {
        tracing::debug!(
            first_segment_start_secs = first.start_secs,
            first_segment_confidence = first.confidence.unwrap_or(-1.0),
            last_segment_end_secs = last.end_secs,
            last_segment_confidence = last.confidence.unwrap_or(-1.0),
            sample_text = %first.text,
            raw_output_len = parsed.raw_output.len(),
            "structured parakeet transcript parsed"
        );
    }

    if transcript.is_empty() {
        return Err(TranscribeError::EmptyTranscript(
            config.transcription.min_words,
        ));
    }

    Ok(TranscribeResult {
        text: transcript,
        stats,
        segments: vec![],
        detected_language: None,
    })
}

#[cfg(feature = "parakeet")]
fn parakeet_seen_models() -> &'static Mutex<HashSet<String>> {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashSet::new()))
}

#[cfg(feature = "parakeet")]
pub(crate) fn combined_parakeet_boost_phrases(config: &Config, hints: &DecodeHints) -> Vec<String> {
    let mut phrases = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for phrase in hints.parakeet_local_boost_phrases() {
        let key = phrase.to_ascii_lowercase();
        if seen.insert(key) {
            phrases.push(phrase);
        }
    }

    let limit = config.transcription.parakeet_boost_limit;
    if limit > 0 {
        match crate::graph::parakeet_boost_phrases(limit) {
            Ok(global_phrases) => {
                for phrase in global_phrases {
                    let key = phrase.to_ascii_lowercase();
                    if seen.insert(key) {
                        phrases.push(phrase);
                    }
                }
            }
            Err(error) => {
                tracing::debug!(error = %error, "could not load parakeet boost phrases");
            }
        }
    }

    phrases
}

#[cfg(feature = "parakeet")]
fn append_parakeet_boost_args(
    command: &mut std::process::Command,
    config: &Config,
    hints: &DecodeHints,
) {
    let phrases = combined_parakeet_boost_phrases(config, hints);
    if phrases.is_empty() {
        return;
    }

    command.args([
        "--boost-score",
        &config.transcription.parakeet_boost_score.to_string(),
    ]);
    for phrase in &phrases {
        command.args(["--boost", phrase]);
    }
    tracing::debug!(phrases = phrases.len(), "applied parakeet boost phrases");
}

#[cfg(feature = "parakeet")]
fn parakeet_gpu_unavailable(stderr: &str) -> bool {
    stderr.contains("Metal GPU not available")
}

#[cfg(feature = "parakeet")]
#[allow(clippy::too_many_arguments)]
fn build_parakeet_command(
    binary: &str,
    model_str: &str,
    audio_args: &[&str],
    vocab_str: &str,
    model_id: &str,
    use_gpu: bool,
    use_fp16: bool,
    vad_path: Option<&str>,
    vad_threshold: f32,
    config: &Config,
    hints: &DecodeHints,
) -> std::process::Command {
    let mut command = std::process::Command::new(binary);
    command.arg(model_str);
    for audio_arg in audio_args {
        command.arg(audio_arg);
    }
    command
        .args(["--vocab", vocab_str])
        .args(["--model", model_id])
        .arg("--timestamps");
    if use_gpu {
        command.arg("--gpu");
        if use_fp16 {
            command.arg("--fp16");
        }
    }
    if let Some(vad_path) = vad_path {
        command
            .args(["--vad", vad_path])
            .args(["--vad-threshold", &vad_threshold.to_string()]);
    }
    append_parakeet_boost_args(&mut command, config, hints);
    command
}

/// Hard ceiling on a single parakeet subprocess invocation. Scaled to 3× the
/// audio duration so batch jobs on long recordings never trip it, with a floor
/// generous enough to absorb a cold model load. Without this, a wedged
/// subprocess blocks its caller forever — the live recording sidecar froze for
/// 27 minutes mid-meeting on exactly this path (2026-06-10).
#[cfg(feature = "parakeet")]
const PARAKEET_SUBPROCESS_TIMEOUT_FLOOR_SECS: u64 = 90;
#[cfg(feature = "parakeet")]
const PARAKEET_SUBPROCESS_TIMEOUT_CEIL_SECS: u64 = 1800;

#[cfg(feature = "parakeet")]
fn parakeet_subprocess_timeout(audio_args: &[&str]) -> std::time::Duration {
    let audio_secs = audio_args
        .iter()
        .filter_map(|arg| estimate_wav_secs(Path::new(arg)))
        .fold(0.0_f64, f64::max);
    if audio_secs == 0.0 {
        // Duration unknown (non-WAV input or unreadable header). Killing a
        // legitimate long job is worse than waiting longer to declare a wedge,
        // so fall to the ceiling, not the floor.
        return std::time::Duration::from_secs(PARAKEET_SUBPROCESS_TIMEOUT_CEIL_SECS);
    }
    let scaled = (audio_secs * 3.0).ceil() as u64;
    std::time::Duration::from_secs(scaled.clamp(
        PARAKEET_SUBPROCESS_TIMEOUT_FLOOR_SECS,
        PARAKEET_SUBPROCESS_TIMEOUT_CEIL_SECS,
    ))
}

#[cfg(feature = "parakeet")]
fn estimate_wav_secs(path: &Path) -> Option<f64> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    if spec.sample_rate == 0 {
        return None;
    }
    Some(reader.duration() as f64 / spec.sample_rate as f64)
}

/// Run a command to completion like `Command::output()`, but kill the child
/// if it exceeds `timeout`. Pipes are drained on dedicated threads so a child
/// that fills stdout/stderr can't deadlock against the wait loop. Returns the
/// output plus whether the child was killed by the timeout.
#[cfg(feature = "parakeet")]
fn output_with_timeout(
    mut command: std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<(std::process::Output, bool)> {
    use std::io::Read;

    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let mut stdout_pipe = child.stdout.take();
    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut pipe) = stdout_pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let mut stderr_pipe = child.stderr.take();
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut pipe) = stderr_pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let status = child.wait()?;
                    // Don't join the drain threads here: if the child spawned
                    // helpers that inherited the pipe fds, read_to_end won't
                    // see EOF until they exit. The caller discards output on
                    // timeout anyway; let the drains finish detached.
                    drop(stdout_handle);
                    drop(stderr_handle);
                    return Ok((
                        std::process::Output {
                            status,
                            stdout: Vec::new(),
                            stderr: Vec::new(),
                        },
                        true,
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();
    Ok((
        std::process::Output {
            status,
            stdout,
            stderr,
        },
        false,
    ))
}

#[cfg(feature = "parakeet")]
#[allow(clippy::too_many_arguments)]
fn run_parakeet_command_with_cpu_fallback(
    binary: &str,
    model_str: &str,
    audio_args: &[&str],
    vocab_str: &str,
    model_id: &str,
    use_gpu: bool,
    use_fp16: bool,
    vad_path: Option<&str>,
    vad_threshold: f32,
    config: &Config,
    hints: &DecodeHints,
) -> Result<(std::process::Output, bool), TranscribeError> {
    let mut attempted_gpu = use_gpu;
    let timeout = parakeet_subprocess_timeout(audio_args);
    loop {
        let command = build_parakeet_command(
            binary,
            model_str,
            audio_args,
            vocab_str,
            model_id,
            attempted_gpu,
            use_fp16 && attempted_gpu,
            vad_path,
            vad_threshold,
            config,
            hints,
        );
        let (output, timed_out) = output_with_timeout(command, timeout).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                TranscribeError::ParakeetNotFound
            } else {
                TranscribeError::ParakeetFailed(format!("spawn error: {}", e))
            }
        })?;

        if timed_out {
            tracing::error!(
                timeout_secs = timeout.as_secs(),
                gpu = attempted_gpu,
                "parakeet subprocess exceeded timeout and was killed"
            );
            // A wedged GPU run gets the same one-shot CPU retry a crashed GPU
            // run gets — a Metal hang shouldn't be a harder failure than a
            // Metal crash.
            if attempted_gpu {
                tracing::warn!("parakeet GPU run timed out; retrying once on CPU");
                attempted_gpu = false;
                continue;
            }
            return Err(TranscribeError::ParakeetFailed(format!(
                "parakeet subprocess exceeded {}s timeout and was killed",
                timeout.as_secs()
            )));
        }

        if output.status.success() {
            return Ok((output, attempted_gpu));
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        if attempted_gpu && parakeet_gpu_unavailable(&stderr) {
            tracing::warn!("parakeet GPU path unavailable at runtime; retrying on CPU");
            attempted_gpu = false;
            continue;
        }

        return Err(TranscribeError::ParakeetFailed(
            stderr.lines().last().unwrap_or("unknown error").to_string(),
        ));
    }
}

#[cfg(feature = "parakeet")]
fn resolve_minutes_parakeet_helper() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("MINUTES_PARAKEET_HELPER") {
        let path = PathBuf::from(explicit);
        if path.exists() {
            return Some(path);
        }
    }

    if let Ok(current) = std::env::current_exe() {
        if current
            .file_name()
            .and_then(|value| value.to_str())
            .map(|value| value == "minutes")
            .unwrap_or(false)
        {
            return Some(current);
        }
    }

    which::which("minutes").ok()
}

#[cfg(feature = "parakeet")]
#[derive(Debug, Clone)]
pub struct ParakeetWarmupStats {
    pub elapsed_ms: u64,
    pub model: String,
    pub used_gpu: bool,
}

// Word-to-sentence grouping is in crate::parakeet::group_word_segments

/// Atomically take a one-shot "this is the first call" signal from a
/// caller-owned gate. Returns `true` exactly once across the lifetime of
/// the gate; subsequent calls return `false`.
///
/// Used by the parakeet-helper failure logging below to emit warn on the
/// first failure of each kind per process and then go quiet so a long
/// recording does not flood the log. Each distinct failure mode passes
/// its own `static AtomicBool` so non-zero exit and spawn failure are
/// gated independently.
///
/// Extracted as a free function (rather than inlined) so the
/// loud-first-then-quiet semantics can be unit-tested with a non-static
/// gate. The static gate would otherwise leak state across tests in the
/// same process.
#[cfg(feature = "parakeet")]
fn loud_once(gate: &AtomicBool) -> bool {
    !gate.swap(true, Ordering::Relaxed)
}

/// Log the FIRST helper-subprocess non-zero exit per process at warn level
/// before falling back to direct invocation. Subsequent failures are
/// silenced (debug only) to avoid log spam during a recording.
///
/// The motivation is issue #163: when `transcribe.rs` and the
/// `ParakeetHelper` clap struct in `crates/cli/src/main.rs` disagree about
/// what flags exist, the helper rejects every invocation and the code
/// silently falls back to spawning parakeet directly. Without this warning,
/// that kind of regression hides for arbitrary durations because the
/// fallback path is functional. Loud first occurrence forces the failure
/// onto someone's screen instead of into the void.
#[cfg(feature = "parakeet")]
fn log_parakeet_helper_failure_once(status: &std::process::ExitStatus, stderr: &str) {
    static GATE: AtomicBool = AtomicBool::new(false);
    let last_line = stderr.lines().last().unwrap_or("").trim();
    if loud_once(&GATE) {
        tracing::warn!(
            exit_status = ?status,
            stderr_tail = %last_line,
            "parakeet-helper exited non-zero; falling back to direct subprocess. \
             This message is logged once per process; subsequent failures will \
             be debug-level. If you see this, check that every argv flag in \
             transcribe::transcribe_with_parakeet is also accepted by the \
             ParakeetHelper clap struct in crates/cli/src/main.rs (issue #163)."
        );
    } else {
        tracing::debug!(
            exit_status = ?status,
            stderr_tail = %last_line,
            "parakeet-helper exited non-zero (suppressed; first occurrence already logged)"
        );
    }
}

/// Companion to [`log_parakeet_helper_failure_once`] for the case where the
/// helper subprocess could not be spawned at all (binary missing, permission
/// denied, etc.). Same loud-first-then-quiet semantics.
#[cfg(feature = "parakeet")]
fn log_parakeet_helper_spawn_failure_once(error: &std::io::Error) {
    static GATE: AtomicBool = AtomicBool::new(false);
    if loud_once(&GATE) {
        tracing::warn!(
            error = %error,
            "parakeet-helper spawn failed; falling back to direct subprocess. \
             This message is logged once per process; subsequent failures will \
             be debug-level."
        );
    } else {
        tracing::debug!(
            error = %error,
            "parakeet-helper spawn failed (suppressed; first occurrence already logged)"
        );
    }
}

#[cfg(feature = "parakeet")]
#[allow(clippy::too_many_arguments)]
pub fn run_parakeet_cli_structured(
    binary: &str,
    model_path: &Path,
    audio_path: &Path,
    vocab_path: &Path,
    model_id: &str,
    use_gpu: bool,
    vad_path: Option<&Path>,
    vad_threshold: f32,
    config: &Config,
    hints: &DecodeHints,
) -> Result<ParakeetCliTranscript, TranscribeError> {
    let model_str = model_path
        .to_str()
        .ok_or_else(|| TranscribeError::ParakeetFailed("model path is not valid UTF-8".into()))?;
    let wav_str = audio_path
        .to_str()
        .ok_or_else(|| TranscribeError::ParakeetFailed("audio path is not valid UTF-8".into()))?;
    let vocab_str = vocab_path
        .to_str()
        .ok_or_else(|| TranscribeError::ParakeetFailed("vocab path is not valid UTF-8".into()))?;

    let use_fp16 = use_gpu && config.transcription.parakeet_fp16;
    let (output, _used_gpu) = run_parakeet_command_with_cpu_fallback(
        binary,
        model_str,
        &[wav_str],
        vocab_str,
        model_id,
        use_gpu,
        use_fp16,
        vad_path.and_then(|path| path.to_str()),
        vad_threshold,
        config,
        hints,
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let (parsed, _transcript, _stats) = parse_parakeet_output(&stdout, config)?;
    Ok(parsed)
}

#[cfg(feature = "parakeet")]
#[allow(clippy::too_many_arguments)]
pub fn run_parakeet_cli_structured_batch(
    binary: &str,
    model_path: &Path,
    audio_paths: &[PathBuf],
    vocab_path: &Path,
    model_id: &str,
    use_gpu: bool,
    vad_path: Option<&Path>,
    vad_threshold: f32,
    config: &Config,
    hints: &DecodeHints,
) -> Result<Vec<Result<ParakeetCliTranscript, TranscribeError>>, TranscribeError> {
    if audio_paths.is_empty() {
        return Ok(Vec::new());
    }

    let model_str = model_path
        .to_str()
        .ok_or_else(|| TranscribeError::ParakeetFailed("model path is not valid UTF-8".into()))?;
    let vocab_str = vocab_path
        .to_str()
        .ok_or_else(|| TranscribeError::ParakeetFailed("vocab path is not valid UTF-8".into()))?;

    let use_fp16 = use_gpu && config.transcription.parakeet_fp16;
    let audio_args = audio_paths
        .iter()
        .map(|audio_path| {
            audio_path.to_str().ok_or_else(|| {
                TranscribeError::ParakeetFailed("audio path is not valid UTF-8".into())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let (output, _used_gpu) = run_parakeet_command_with_cpu_fallback(
        binary,
        model_str,
        &audio_args,
        vocab_str,
        model_id,
        use_gpu,
        use_fp16,
        vad_path.and_then(|path| path.to_str()),
        vad_threshold,
        config,
        hints,
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_parakeet_batch_output(&stdout, audio_paths.len(), config)
}

#[cfg(feature = "parakeet")]
fn parse_parakeet_batch_output(
    raw_output: &str,
    expected_sections: usize,
    config: &Config,
) -> Result<Vec<Result<ParakeetCliTranscript, TranscribeError>>, TranscribeError> {
    let mut sections: Vec<(bool, Vec<String>)> = Vec::new();
    let mut current: Option<(bool, Vec<String>)> = None;

    for line in raw_output.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with("--- [") && trimmed.contains("] ") && trimmed.contains("tokens") {
            if let Some(section) = current.take() {
                sections.push(section);
            }
            let zero_tokens = trimmed.contains("(0 tokens)");
            current = Some((zero_tokens, Vec::new()));
            continue;
        }

        if let Some((_, body)) = current.as_mut() {
            body.push(trimmed.to_string());
        }
    }

    if let Some(section) = current.take() {
        sections.push(section);
    }

    if sections.len() != expected_sections {
        return Err(TranscribeError::ParakeetFailed(format!(
            "expected {} batched parakeet sections, found {}",
            expected_sections,
            sections.len()
        )));
    }

    Ok(sections
        .into_iter()
        .map(|(zero_tokens, body)| {
            if zero_tokens {
                return Err(TranscribeError::EmptyTranscript(
                    config.transcription.min_words,
                ));
            }
            let section_output = body.join("\n");
            parse_parakeet_output(&section_output, config).map(|(parsed, _, _)| parsed)
        })
        .collect())
}

#[cfg(feature = "parakeet")]
fn parse_parakeet_output(
    raw_output: &str,
    config: &Config,
) -> Result<(ParakeetCliTranscript, String, ParakeetFilterStats), TranscribeError> {
    let raw = raw_output.trim();
    if raw.is_empty() {
        return Err(TranscribeError::EmptyTranscript(
            config.transcription.min_words,
        ));
    }

    let mut lines = Vec::new();
    let mut segments = Vec::new();
    let mut has_timestamps = false;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Try to parse "[start - end] text" format
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(bracket_end) = rest.find(']') {
                let timestamp_part = &rest[..bracket_end];
                let mut text = rest[bracket_end + 1..].trim();
                // Strip optional confidence prefix: "(0.54) actual text"
                if text.starts_with('(') {
                    if let Some(paren_end) = text.find(')') {
                        text = text[paren_end + 1..].trim();
                    }
                }

                if let Some((start_str, _end_str)) = timestamp_part.split_once('-') {
                    // Strip trailing 's' suffix (parakeet.cpp outputs "2.80s")
                    let start_clean = start_str.trim().trim_end_matches('s');
                    let end_clean = _end_str.trim().trim_end_matches('s');
                    if let (Ok(start_secs), Ok(end_secs)) =
                        (start_clean.parse::<f64>(), end_clean.parse::<f64>())
                    {
                        let mins = (start_secs / 60.0) as u64;
                        let secs = (start_secs % 60.0) as u64;
                        if !text.is_empty() {
                            lines.push(format!("[{}:{:02}] {}", mins, secs, text));
                            let confidence = rest[bracket_end + 1..]
                                .trim()
                                .strip_prefix('(')
                                .and_then(|value| value.split_once(')'))
                                .and_then(|(value, _)| value.parse::<f32>().ok());
                            segments.push(ParakeetCliSegment {
                                start_secs,
                                end_secs,
                                confidence,
                                text: text.to_string(),
                            });
                            has_timestamps = true;
                        }
                        continue;
                    }
                }
            }
        }

        // Non-timestamp line — skip (don't fake [0:00] timestamps)
    }

    if segments.is_empty() {
        let zero_token_banner = raw
            .lines()
            .map(str::trim)
            .any(|line| line == "--- Transcription (0 tokens) ---");
        if zero_token_banner {
            return Err(TranscribeError::EmptyTranscript(
                config.transcription.min_words,
            ));
        }
        if !has_timestamps {
            // No parseable output at all — include a snippet in the error for debugging
            let preview: String = raw.chars().take(200).collect();
            return Err(TranscribeError::ParakeetFailed(format!(
                "could not parse parakeet output (no [start - end] timestamps found). \
                 First 200 chars: {}",
                preview
            )));
        }
    }

    parakeet_transcript_from_segments_with_stats(raw, segments, config)
}

#[cfg(feature = "parakeet")]
pub(crate) fn parakeet_transcript_from_segments(
    raw_output: &str,
    segments: Vec<ParakeetCliSegment>,
    config: &Config,
) -> Result<ParakeetCliTranscript, TranscribeError> {
    let (parsed, _, _) =
        parakeet_transcript_from_segments_with_stats(raw_output, segments, config)?;
    Ok(parsed)
}

#[cfg(feature = "parakeet")]
fn parakeet_transcript_from_segments_with_stats(
    raw_output: &str,
    segments: Vec<ParakeetCliSegment>,
    config: &Config,
) -> Result<(ParakeetCliTranscript, String, ParakeetFilterStats), TranscribeError> {
    let segments = crate::parakeet::group_word_segments(&segments);
    let lines: Vec<String> = segments
        .iter()
        .map(|segment| {
            let mins = (segment.start_secs / 60.0) as u64;
            let secs = (segment.start_secs % 60.0) as u64;
            format!("[{}:{:02}] {}", mins, secs, segment.text)
        })
        .collect();
    let raw_segments = lines.len();

    if lines.is_empty() {
        return Err(TranscribeError::EmptyTranscript(
            config.transcription.min_words,
        ));
    }

    let cleanup = run_transcript_cleanup_pipeline(lines);

    let pstats = ParakeetFilterStats {
        raw_segments,
        after_dedup: cleanup.after(TranscriptCleanupStage::DedupSegments),
        after_interleaved: cleanup.after(TranscriptCleanupStage::DedupInterleaved),
        after_script_filter: cleanup.after(TranscriptCleanupStage::StripForeignScript),
        after_noise_markers: cleanup.after(TranscriptCleanupStage::CollapseNoiseMarkers),
        after_trailing_trim: cleanup.after(TranscriptCleanupStage::TrimTrailingNoise),
    };

    let transcript = cleanup.lines.join("\n");
    if transcript.is_empty() {
        return Err(TranscribeError::EmptyTranscript(
            config.transcription.min_words,
        ));
    }

    let transcript_with_newline = format!("{}\n", transcript);
    Ok((
        ParakeetCliTranscript {
            raw_output: raw_output.to_string(),
            segments,
            transcript: transcript_with_newline.clone(),
        },
        transcript_with_newline,
        pstats,
    ))
}

/// Write f32 samples as a 16kHz mono 16-bit WAV file.
pub(crate) fn write_wav_16k_mono(path: &Path, samples: &[f32]) -> Result<(), TranscribeError> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| TranscribeError::Io(std::io::Error::other(e.to_string())))?;
    for &s in samples {
        let sample = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
        writer
            .write_sample(sample)
            .map_err(|e| TranscribeError::Io(std::io::Error::other(e.to_string())))?;
    }
    writer
        .finalize()
        .map_err(|e| TranscribeError::Io(std::io::Error::other(e.to_string())))?;
    Ok(())
}

/// Resolve the parakeet model file path.
///
/// Looks for `.safetensors` files in `~/.minutes/models/parakeet/`.
#[cfg(feature = "parakeet")]
pub(crate) fn resolve_parakeet_model_path(config: &Config) -> Result<PathBuf, TranscribeError> {
    let model_name = &config.transcription.parakeet_model;
    let model_dir = crate::parakeet::installs_root(config);

    if let Some(candidate) = crate::parakeet::resolve_model_file(config, model_name) {
        return Ok(candidate);
    }

    // Try as absolute path
    let direct = PathBuf::from(model_name);
    if direct.exists() {
        return Ok(direct);
    }

    let mut message = format!(
        "Expected parakeet model \"{}\" in {}.\n\nTo fix this, run:\n\n    minutes setup --parakeet\n",
        model_name,
        model_dir.display(),
    );
    if model_name == "tdt-600m"
        && crate::parakeet::resolve_model_file(config, "tdt-ctc-110m").is_some()
    {
        message.push_str(
            "\nIt looks like you still have the older English-only `tdt-ctc-110m` model installed.\n\
             If you want to keep using it, add this to ~/.config/minutes/config.toml:\n\n\
                 parakeet_model = \"tdt-ctc-110m\"\n\
                 parakeet_vocab = \"tdt-ctc-110m.tokenizer.vocab\"\n",
        );
    }
    Err(TranscribeError::ModelNotFound(message))
}

/// Resolve the parakeet SentencePiece vocab file path.
///
/// Looks for the vocab file in `~/.minutes/models/parakeet/` alongside the model.
#[cfg(feature = "parakeet")]
pub(crate) fn resolve_parakeet_vocab_path(config: &Config) -> Result<PathBuf, TranscribeError> {
    let model_dir = crate::parakeet::installs_root(config);
    let vocab_name = &config.transcription.parakeet_vocab;
    if let Some(candidate) = crate::parakeet::resolve_tokenizer_file(
        config,
        &config.transcription.parakeet_model,
        vocab_name,
    ) {
        return Ok(candidate);
    }

    // Try as absolute path
    let direct = PathBuf::from(vocab_name);
    if direct.exists() {
        return Ok(direct);
    }

    Err(TranscribeError::ModelNotFound(format!(
        "Expected parakeet vocab file \"{}\" in {}. Generated during model conversion.",
        vocab_name,
        model_dir.display(),
    )))
}

#[cfg(feature = "parakeet")]
pub(crate) fn resolve_parakeet_native_vad_path(config: &Config) -> Option<PathBuf> {
    let mut candidates =
        vec![crate::parakeet::installs_root(config).join("silero_vad_v5.safetensors")];

    let configured_vad = PathBuf::from(&config.transcription.vad_model);
    if configured_vad
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("safetensors"))
        .unwrap_or(false)
    {
        candidates.push(configured_vad);
    }

    candidates.into_iter().find(|candidate| candidate.exists())
}

#[cfg(feature = "parakeet")]
pub fn transcribe_parakeet_batch(
    audio_paths: &[PathBuf],
    config: &Config,
) -> Result<Vec<Result<TranscribeResult, TranscribeError>>, TranscribeError> {
    if audio_paths.is_empty() {
        return Ok(Vec::new());
    }

    if !crate::parakeet::valid_model(&config.transcription.parakeet_model) {
        return Err(TranscribeError::ParakeetFailed(format!(
            "unknown parakeet model '{}'. Valid: {}",
            config.transcription.parakeet_model,
            crate::config::VALID_PARAKEET_MODELS.join(", ")
        )));
    }

    let model_path = resolve_parakeet_model_path(config)?;
    let vocab_path = resolve_parakeet_vocab_path(config)?;
    let native_vad_path = resolve_parakeet_native_vad_path(config);
    let resolved_binary = crate::parakeet::resolve_parakeet_binary(
        &config.transcription.parakeet_binary,
        crate::parakeet::ResolveParakeetBinaryMode::WarnAndFallback,
    )
    .map_err(|error| TranscribeError::ParakeetFailed(error.to_string()))?;
    let resolved_binary_str = resolved_binary.to_str().ok_or_else(|| {
        TranscribeError::ParakeetFailed("resolved parakeet binary path is not valid UTF-8".into())
    })?;
    let use_gpu = cfg!(all(target_os = "macos", target_arch = "aarch64"));
    let host_process_key = format!(
        "{}::{}",
        resolved_binary.display(),
        config.transcription.parakeet_model.as_str()
    );
    let host_process_first_use = {
        let mut seen = parakeet_seen_models()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        seen.insert(host_process_key)
    };

    let mut stats_per_file = Vec::with_capacity(audio_paths.len());
    for audio_path in audio_paths {
        let samples = load_audio_samples(audio_path)?;
        if samples.is_empty() {
            stats_per_file.push(Err(TranscribeError::EmptyAudio));
            continue;
        }

        let stats = FilterStats {
            audio_duration_secs: samples.len() as f64 / 16000.0,
            samples_after_silence_strip: samples.len(),
            ..Default::default()
        };
        stats_per_file.push(Ok(stats));
    }

    let invocation_started = Instant::now();
    let parsed_batch = run_parakeet_cli_structured_batch(
        resolved_binary_str,
        &model_path,
        audio_paths,
        &vocab_path,
        &config.transcription.parakeet_model,
        use_gpu,
        native_vad_path.as_deref(),
        PARAKEET_NATIVE_VAD_THRESHOLD,
        config,
        &DecodeHints::default(),
    )?;
    let elapsed_ms = invocation_started.elapsed().as_millis() as u64;

    Ok(parsed_batch
        .into_iter()
        .zip(stats_per_file)
        .map(|(parsed, stats)| match (parsed, stats) {
            (_, Err(error)) => Err(error),
            (Err(error), Ok(_)) => Err(error),
            (Ok(parsed), Ok(stats)) => transcribe_result_from_parakeet_parsed(
                parsed,
                stats,
                host_process_first_use,
                elapsed_ms,
                config,
            ),
        })
        .collect())
}

#[cfg(feature = "parakeet")]
pub fn warmup_parakeet(config: &Config) -> Result<ParakeetWarmupStats, TranscribeError> {
    let model_path = resolve_parakeet_model_path(config)?;
    let vocab_path = resolve_parakeet_vocab_path(config)?;
    let resolved_binary = crate::parakeet::resolve_parakeet_binary(
        &config.transcription.parakeet_binary,
        crate::parakeet::ResolveParakeetBinaryMode::WarnAndFallback,
    )
    .map_err(|error| TranscribeError::ParakeetFailed(error.to_string()))?;
    let native_vad_path = resolve_parakeet_native_vad_path(config);

    let tmp_wav = tempfile::Builder::new()
        .prefix("minutes-parakeet-warmup-")
        .suffix(".wav")
        .tempfile()
        .map_err(TranscribeError::Io)?;
    let silence = vec![0.0f32; 16000];
    write_wav_16k_mono(tmp_wav.path(), &silence)?;

    let model_str = model_path
        .to_str()
        .ok_or_else(|| TranscribeError::ParakeetFailed("model path is not valid UTF-8".into()))?;
    let wav_str = tmp_wav.path().to_str().ok_or_else(|| {
        TranscribeError::ParakeetFailed("temp WAV path is not valid UTF-8".into())
    })?;
    let vocab_str = vocab_path
        .to_str()
        .ok_or_else(|| TranscribeError::ParakeetFailed("vocab path is not valid UTF-8".into()))?;

    let used_gpu = cfg!(all(target_os = "macos", target_arch = "aarch64"));
    let used_fp16 = used_gpu && config.transcription.parakeet_fp16;
    let started = Instant::now();
    let resolved_binary_str = resolved_binary.to_str().ok_or_else(|| {
        TranscribeError::ParakeetFailed("resolved parakeet binary path is not valid UTF-8".into())
    })?;
    let (_output, used_gpu) = run_parakeet_command_with_cpu_fallback(
        resolved_binary_str,
        model_str,
        &[wav_str],
        vocab_str,
        &config.transcription.parakeet_model,
        used_gpu,
        used_fp16,
        native_vad_path.as_ref().and_then(|path| path.to_str()),
        PARAKEET_NATIVE_VAD_THRESHOLD,
        config,
        &DecodeHints::default(),
    )?;

    Ok(ParakeetWarmupStats {
        elapsed_ms: started.elapsed().as_millis() as u64,
        model: config.transcription.parakeet_model.clone(),
        used_gpu,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "whisper")]
    fn resolve_model_path_returns_error_for_missing() {
        let config = Config {
            transcription: crate::config::TranscriptionConfig {
                model: "nonexistent".into(),
                model_path: PathBuf::from("/tmp/no-such-dir"),
                min_words: 10,
                language: Some("en".into()),
                vad_model: String::new(),
                noise_reduction: false,
                ..crate::config::TranscriptionConfig::default()
            },
            ..Config::default()
        };
        let result = resolve_model_path(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("minutes setup --model nonexistent"),
            "error should tell user how to fix it: {}",
            err
        );
        assert!(
            err.contains("ggml-nonexistent.bin"),
            "error should include expected model filename: {}",
            err
        );
        assert!(
            err.contains("/tmp/no-such-dir"),
            "error should include the model directory: {}",
            err
        );
    }

    #[test]
    fn expected_whisper_model_sizes_cover_canonical_names() {
        for name in ["tiny", "base", "small", "medium", "large-v3"] {
            assert!(
                expected_whisper_model_size_bytes(name).is_some(),
                "missing expected size for {}",
                name
            );
        }
        assert!(expected_whisper_model_size_bytes("nonexistent").is_none());
        // Sanity: medium must be greater than small (catches regressions
        // where someone confuses the rows of the size table).
        assert!(
            expected_whisper_model_size_bytes("medium").unwrap()
                > expected_whisper_model_size_bytes("small").unwrap()
        );
    }

    #[test]
    fn chunk_strategy_reuses_context_only_for_whisper_builds() {
        let mut config = Config::default();
        config.transcription.engine = "whisper".into();
        let expected = if cfg!(feature = "whisper") {
            ChunkTranscriptionStrategy::SharedWhisperContext
        } else {
            ChunkTranscriptionStrategy::DispatchPerChunk
        };
        assert_eq!(chunk_transcription_strategy(&config), expected);
    }

    #[test]
    fn chunk_strategy_keeps_non_whisper_backends_on_dispatch_path() {
        let mut config = Config::default();
        config.transcription.engine = "parakeet".into();
        assert_eq!(
            chunk_transcription_strategy(&config),
            ChunkTranscriptionStrategy::DispatchPerChunk
        );
    }

    #[test]
    #[cfg(feature = "whisper")]
    fn validate_whisper_model_size_rejects_truncated_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ggml-medium.bin");
        // Reproduces issue #229: write a 221 MB file where ~1.5 GB is expected.
        // We use a smaller stand-in (10 MB) since the validator only cares about
        // the comparison against `expected_whisper_model_size_bytes("medium")`.
        let truncated_bytes = 10 * 1024 * 1024;
        std::fs::write(&path, vec![0u8; truncated_bytes]).unwrap();

        let result = validate_whisper_model_size(path.clone());
        let err = result.expect_err("a 10 MB ggml-medium.bin should be rejected");
        match err {
            TranscribeError::ModelTruncated {
                model_name,
                actual_mb,
                expected_min_mb,
                ..
            } => {
                assert_eq!(model_name, "medium");
                assert!(actual_mb < expected_min_mb);
            }
            other => panic!("expected ModelTruncated, got {:?}", other),
        }
    }

    #[test]
    #[cfg(feature = "whisper")]
    fn validate_whisper_model_size_accepts_full_size_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("ggml-tiny.bin");
        // A file at or above the tiny threshold should pass through.
        let bytes = expected_whisper_model_size_bytes("tiny").unwrap() as usize + 1024;
        std::fs::write(&path, vec![0u8; bytes]).unwrap();
        let validated =
            validate_whisper_model_size(path.clone()).expect("full-size tiny should pass");
        assert_eq!(validated, path);
    }

    #[test]
    #[cfg(feature = "whisper")]
    fn validate_whisper_model_size_ignores_unknown_filenames() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("custom-finetune.bin");
        std::fs::write(&path, vec![0u8; 1024]).unwrap();
        // No `ggml-` prefix means no expected size; the validator must not
        // reject otherwise legitimate user files in non-standard locations.
        let validated = validate_whisper_model_size(path.clone()).unwrap();
        assert_eq!(validated, path);
    }

    #[test]
    fn load_wav_rejects_empty_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("empty.wav");
        std::fs::write(&path, "").unwrap();
        let result = load_wav(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_wav_reads_valid_wav() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.wav");

        // Create a short WAV with hound
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..16000 {
            let sample =
                (10000.0 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin()) as i16;
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();

        let samples = load_wav(&path).unwrap();
        assert!(!samples.is_empty());
        // 1 second at 16kHz = 16000 samples
        assert_eq!(samples.len(), 16000);
    }

    #[test]
    #[cfg(not(feature = "whisper"))]
    fn traced_transcribe_path_writes_ordered_process_breadcrumbs() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _lock = ENV_LOCK.lock().unwrap();
        let old_minutes_home = std::env::var_os("MINUTES_HOME");
        let old_minutes_trace = std::env::var_os("MINUTES_TRACE");

        let dir = tempfile::TempDir::new().unwrap();
        std::env::set_var("MINUTES_HOME", dir.path());
        std::env::remove_var("MINUTES_TRACE");

        let audio_path = dir.path().join("probe.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&audio_path, spec).unwrap();
        for _ in 0..1600 {
            writer.write_sample(0_i16).unwrap();
        }
        writer.finalize().unwrap();

        let mut config = Config::default();
        config.transcription.language = Some("fr".into());
        let _trace_guard = crate::process_trace::start_process_trace(
            &audio_path,
            crate::markdown::ContentType::Memo,
            &config,
        );
        transcribe_with_hints(&audio_path, &config, &DecodeHints::default()).unwrap();
        crate::process_trace::stage("process.done");

        let trace_path = crate::process_trace::trace_file_path_for_tests().unwrap();
        let trace = std::fs::read_to_string(trace_path).unwrap();
        let events = trace
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        let stages = events
            .iter()
            .map(|event| event["stage"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            stages,
            vec![
                "process.start",
                "decode.start",
                "decode.done",
                "process.done"
            ]
        );
        assert!(events.iter().all(|event| event["ts"].is_string()));
        assert_eq!(events[0]["input_path"], audio_path.display().to_string());
        assert_eq!(events[0]["language"], "fr");
        assert_eq!(events[0]["content_type"], "memo");
        assert_eq!(events[2]["audio_samples"], 1600);

        match old_minutes_home {
            Some(value) => std::env::set_var("MINUTES_HOME", value),
            None => std::env::remove_var("MINUTES_HOME"),
        }
        match old_minutes_trace {
            Some(value) => std::env::set_var("MINUTES_TRACE", value),
            None => std::env::remove_var("MINUTES_TRACE"),
        }
    }

    #[test]
    fn load_audio_rejects_unknown_extension() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.xyz");
        std::fs::write(&path, "not audio").unwrap();
        let result = load_audio_samples(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("xyz"));
    }

    #[test]
    fn strip_silence_preserves_speech() {
        // 1s of "speech" (high energy sine wave)
        let speech: Vec<f32> = (0..16000)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin())
            .collect();
        let result = strip_silence(&speech, 16000);
        // All speech — nothing should be stripped
        assert_eq!(result.len(), speech.len());
    }

    #[test]
    fn strip_silence_trims_long_silence() {
        let mut samples = Vec::new();
        // 1s speech
        for i in 0..16000 {
            samples.push(0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin());
        }
        // 5s silence
        samples.extend(vec![0.0f32; 16000 * 5]);
        // 1s speech
        for i in 0..16000 {
            samples.push(0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin());
        }

        let result = strip_silence(&samples, 16000);
        // Should be significantly shorter than 7s (5s of silence trimmed)
        let original_secs = samples.len() as f64 / 16000.0;
        let result_secs = result.len() as f64 / 16000.0;
        assert!(
            result_secs < original_secs * 0.7,
            "expected significant trimming: {:.1}s → {:.1}s",
            original_secs,
            result_secs
        );
        // But should still have both speech segments + padding
        assert!(
            result_secs > 2.0,
            "should preserve both speech segments: {:.1}s",
            result_secs
        );
    }

    #[test]
    fn strip_silence_keeps_short_pauses() {
        let mut samples = Vec::new();
        // 1s speech
        for i in 0..16000 {
            samples.push(0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin());
        }
        // 400ms silence (short natural pause — should be kept)
        samples.extend(vec![0.0f32; 6400]);
        // 1s speech
        for i in 0..16000 {
            samples.push(0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin());
        }

        let result = strip_silence(&samples, 16000);
        // Short pause should be preserved — output ≈ input length
        let ratio = result.len() as f64 / samples.len() as f64;
        assert!(
            ratio > 0.9,
            "short pauses should be preserved: ratio {:.2}",
            ratio
        );
    }

    #[test]
    fn strip_silence_handles_all_silence() {
        let samples = vec![0.0f32; 16000 * 10]; // 10s of silence
        let result = strip_silence(&samples, 16000);
        // Should still produce something (short pad at minimum)
        assert!(result.len() < samples.len() / 2, "should trim most silence");
    }

    #[test]
    fn sinc_resample_no_aliasing() {
        // Generate a 440Hz tone at 44100Hz, resample to 16000Hz.
        // 440Hz is well below Nyquist (8000Hz), so it should survive.
        let n = 44100;
        let samples: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 44100.0).sin())
            .collect();
        let resampled = resample(&samples, 44100, 16000);

        // Check the resampled signal has reasonable amplitude (not attenuated to nothing)
        let peak = resampled.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            peak > 0.8,
            "440Hz tone should survive resampling with peak > 0.8, got {}",
            peak
        );
    }

    #[test]
    fn dedup_no_repetition() {
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:03] How are you".into(),
            "[0:06] Fine thanks".into(),
        ];
        let result = dedup_segments(lines.clone());
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
        let result = dedup_segments(lines);
        assert_eq!(result.len(), 3); // first + marker + different
        assert!(result[0].contains("Hello world"));
        assert!(result[1].contains("repeated audio removed"));
        assert!(result[2].contains("Something different"));
    }

    #[test]
    fn dedup_collapses_near_identical() {
        // Whisper often produces slight variations of the same repeated text
        let lines = vec![
            "[0:00] Ok bene le macedi diesel".into(),
            "[0:03] Ok, bene le macedi diesel".into(),
            "[0:06] Ok bene, le macedi diesel".into(),
            "[0:09] Good morning".into(),
        ];
        let result = dedup_segments(lines);
        assert_eq!(result.len(), 3); // first + marker + different
        assert!(result[1].contains("repeated audio removed"));
    }

    #[test]
    fn dedup_leaves_two_similar_alone() {
        // Only 2 similar — below threshold of 3
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:03] Hello world".into(),
            "[0:06] Something else".into(),
        ];
        let result = dedup_segments(lines.clone());
        assert_eq!(result, lines);
    }

    #[test]
    fn transcript_cleanup_pipeline_matches_legacy_cleanup_behavior() {
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:03] Hello world".into(),
            "[0:06] Hello world".into(),
            "[0:09] [music]".into(),
        ];

        let pipeline = run_transcript_cleanup_pipeline(lines.clone());
        let legacy = trim_trailing_noise(collapse_noise_markers(strip_foreign_script(
            dedup_interleaved(dedup_segments(lines)),
        )));

        assert_eq!(pipeline.lines, legacy);
        assert_eq!(
            pipeline
                .stats
                .iter()
                .map(|stat| stat.stage)
                .collect::<Vec<_>>(),
            vec![
                TranscriptCleanupStage::DedupSegments,
                TranscriptCleanupStage::DedupInterleaved,
                TranscriptCleanupStage::StripForeignScript,
                TranscriptCleanupStage::CollapseNoiseMarkers,
                TranscriptCleanupStage::TrimTrailingNoise,
            ]
        );
        assert_eq!(
            pipeline.after(TranscriptCleanupStage::TrimTrailingNoise),
            pipeline.lines.len()
        );
    }

    fn constant_samples(seconds: usize, amplitude: f32) -> Vec<f32> {
        vec![amplitude; seconds * 16_000]
    }

    #[test]
    fn meeting_vad_chunks_split_on_long_silence() {
        let mut samples = constant_samples(2, 0.05);
        samples.extend(constant_samples(2, 0.0));
        samples.extend(constant_samples(2, 0.05));

        let chunks = detect_meeting_vad_chunks(&samples);
        assert_eq!(
            chunks.len(),
            2,
            "expected split around long silence: {chunks:?}"
        );
    }

    #[test]
    fn meeting_vad_chunks_keep_short_pause_inside_chunk() {
        let mut samples = constant_samples(2, 0.05);
        samples.extend(constant_samples(0, 0.0));
        samples.extend(vec![0.0; 4_800]); // 300ms silence
        samples.extend(constant_samples(2, 0.05));

        let chunks = detect_meeting_vad_chunks(&samples);
        assert_eq!(
            chunks.len(),
            1,
            "short pause should stay in one chunk: {chunks:?}"
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn fixed_length_chunks_splits_audio_without_gaps() {
        let chunks = fixed_length_chunks(16_000 * 95, 16_000 * 45);
        assert_eq!(
            chunks,
            vec![
                (0, 16_000 * 45),
                (16_000 * 45, 16_000 * 90),
                (16_000 * 90, 16_000 * 95)
            ]
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn fixed_length_chunks_handles_empty_audio() {
        let chunks = fixed_length_chunks(0, 16_000 * 45);
        assert!(chunks.is_empty());
    }

    #[test]
    fn offset_timestamped_lines_applies_chunk_offset() {
        let lines = offset_timestamped_lines(["[0:02] hello", "[0:07] world"].into_iter(), 10.0, 0);
        assert_eq!(lines, vec!["[0:12] hello", "[0:17] world"]);
    }

    #[test]
    fn dedup_handles_empty() {
        let result = dedup_segments(vec![]);
        assert!(result.is_empty());
    }

    #[test]
    fn dedup_handles_single_line() {
        let lines = vec!["[0:00] Hello".into()];
        let result = dedup_segments(lines.clone());
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
        let result = dedup_segments(lines);
        // Two collapsed runs + normal text
        assert_eq!(result.len(), 5); // first + marker + second + marker + normal
        assert!(result[1].contains("2 identical"));
        assert!(result[3].contains("3 identical"));
    }

    #[test]
    fn engine_defaults_to_whisper_dispatch() {
        // Verify that the default engine config takes the whisper path
        let config = Config::default();
        assert_eq!(config.transcription.engine, "whisper");
    }

    #[test]
    fn engine_not_available_without_feature() {
        // When parakeet feature is not compiled in, should return EngineNotAvailable
        #[cfg(not(feature = "parakeet"))]
        {
            let config = Config {
                transcription: crate::config::TranscriptionConfig {
                    engine: "parakeet".into(),
                    ..crate::config::TranscriptionConfig::default()
                },
                ..Config::default()
            };
            // Use a dummy path — it should fail at the engine check, not file check
            let result = transcribe(Path::new("/nonexistent/test.wav"), &config);
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("parakeet"),
                "error should mention parakeet: {}",
                err
            );
        }
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parse_parakeet_text_basic() {
        let text = "[0.00 - 2.50] Hello world\n[3.00 - 5.10] How are you\n";
        let config = Config::default();
        let (_parsed, result, _) = parse_parakeet_output(text, &config).unwrap();
        let lines: Vec<&str> = result.trim().lines().collect();
        assert_eq!(lines.len(), 2, "should have 2 lines: {:?}", lines);
        assert!(
            lines[0].contains("[0:00] Hello world"),
            "first: {}",
            lines[0]
        );
        assert!(
            lines[1].contains("[0:03] How are you"),
            "second: {}",
            lines[1]
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parse_parakeet_empty_input() {
        let config = Config::default();
        let result = parse_parakeet_output("", &config);
        assert!(result.is_err(), "empty input should fail");
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parse_parakeet_plain_text_rejected() {
        // Plain text without timestamps should be rejected (not faked as [0:00])
        let text = "this is plain text output without timestamps";
        let config = Config::default();
        let result = parse_parakeet_output(text, &config);
        assert!(result.is_err(), "plain text without timestamps should fail");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no [start - end] timestamps"),
            "error should explain the issue: {}",
            err
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parse_parakeet_zero_token_banner_is_empty_transcript() {
        let text = "\
Loading model: tdt-600m
Model loaded (723 tensors)
Model moved to GPU
Encoder: 4 ms

--- Transcription (0 tokens) ---

--- Word Timestamps ---
";
        let config = Config::default();
        let result = parse_parakeet_output(text, &config);
        assert!(
            matches!(result, Err(TranscribeError::EmptyTranscript(_))),
            "zero-token banner should be treated as empty transcript: {:?}",
            result
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parse_parakeet_nonzero_banner_without_timestamps_still_fails() {
        let text = "\
Loading model: tdt-600m
Model loaded (723 tensors)

--- Transcription (3 tokens) ---
hello there friend

--- Word Timestamps ---
";
        let config = Config::default();
        let result = parse_parakeet_output(text, &config);
        assert!(
            result.is_err(),
            "nonzero banner without timestamps should fail"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no [start - end] timestamps"),
            "nonzero malformed output should still surface parser failure: {}",
            err
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parse_parakeet_timestamp_formatting() {
        // Verify that timestamps > 60s are formatted correctly
        let text = "[125.00 - 126.00] late segment\n";
        let config = Config::default();
        let (_parsed, result, _) = parse_parakeet_output(text, &config).unwrap();
        assert!(
            result.contains("[2:05]"),
            "125s should be [2:05]: {}",
            result
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parse_parakeet_dedup_applied() {
        // Repeated lines should be collapsed by the anti-hallucination pipeline
        let text = "[0.00 - 1.00] Hello world\n\
                     [1.00 - 2.00] Hello world\n\
                     [2.00 - 3.00] Hello world\n\
                     [3.00 - 4.00] Hello world\n\
                     [5.00 - 6.00] Something different\n";
        let config = Config::default();
        let (_parsed, result, _) = parse_parakeet_output(text, &config).unwrap();
        let lines: Vec<&str> = result.trim().lines().collect();
        assert!(
            lines.len() < 5,
            "dedup should collapse repetitions: {:?}",
            lines
        );
        assert!(lines.last().unwrap().contains("Something different"));
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parse_parakeet_groups_word_level_segments() {
        let text = "\
[0.00s - 0.10s] (0.90) Hello\n\
[0.11s - 0.20s] (0.90) world.\n\
[1.40s - 1.50s] (0.80) Next\n\
[1.51s - 1.60s] (0.80) sentence\n";
        let config = Config::default();
        let (_parsed, result, _) = parse_parakeet_output(text, &config).unwrap();
        let lines: Vec<&str> = result.trim().lines().collect();
        assert_eq!(lines.len(), 2, "expected two grouped segments: {:?}", lines);
        assert!(lines[0].contains("Hello world."));
        assert!(lines[1].contains("Next sentence"));
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parse_parakeet_model_validation() {
        let config = Config {
            transcription: crate::config::TranscriptionConfig {
                engine: "parakeet".into(),
                parakeet_model: "totally-fake-model".into(),
                ..crate::config::TranscriptionConfig::default()
            },
            ..Config::default()
        };
        let result = transcribe(std::path::Path::new("/nonexistent.wav"), &config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown parakeet model"),
            "should reject invalid model: {}",
            err
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn write_wav_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.wav");

        let samples: Vec<f32> = (0..16000)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin())
            .collect();

        write_wav_16k_mono(&path, &samples).unwrap();

        let reader = hound::WavReader::open(&path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 16000);
        assert_eq!(spec.bits_per_sample, 16);
        let read_samples: Vec<i16> = reader.into_samples().filter_map(|s| s.ok()).collect();
        assert_eq!(read_samples.len(), 16000);
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn resolve_parakeet_model_missing() {
        let config = Config {
            transcription: crate::config::TranscriptionConfig {
                model_path: PathBuf::from("/tmp/no-such-dir"),
                parakeet_model: "tdt-600m".into(),
                ..crate::config::TranscriptionConfig::default()
            },
            ..Config::default()
        };
        let result = resolve_parakeet_model_path(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("minutes setup --parakeet"),
            "error should tell user how to fix it: {}",
            err
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn resolve_parakeet_model_missing_suggests_pinning_110m_when_present() {
        let dir = tempfile::TempDir::new().unwrap();
        let model_dir = dir.path().join("parakeet");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("tdt-ctc-110m.safetensors"), b"legacy").unwrap();
        std::fs::write(
            model_dir.join("tdt-ctc-110m.tokenizer.vocab"),
            b"legacy-tokenizer",
        )
        .unwrap();

        let config = Config {
            transcription: crate::config::TranscriptionConfig {
                model_path: dir.path().to_path_buf(),
                parakeet_model: "tdt-600m".into(),
                ..crate::config::TranscriptionConfig::default()
            },
            ..Config::default()
        };

        let err = resolve_parakeet_model_path(&config)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("tdt-ctc-110m"),
            "should mention legacy 110m option: {}",
            err
        );
        assert!(
            err.contains("parakeet_vocab = \"tdt-ctc-110m.tokenizer.vocab\""),
            "should show how to pin the old tokenizer too: {}",
            err
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn resolve_parakeet_vocab_prefers_model_specific_tokenizer() {
        let dir = tempfile::TempDir::new().unwrap();
        let model_dir = dir.path().join("parakeet");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("tokenizer.vocab"), "wrong\t0\n").unwrap();
        std::fs::write(model_dir.join("tdt-ctc-110m.tokenizer.vocab"), "right\t0\n").unwrap();

        let config = Config {
            transcription: crate::config::TranscriptionConfig {
                model_path: dir.path().to_path_buf(),
                parakeet_model: "tdt-ctc-110m".into(),
                parakeet_vocab: "tokenizer.vocab".into(),
                ..crate::config::TranscriptionConfig::default()
            },
            ..Config::default()
        };

        let resolved = resolve_parakeet_vocab_path(&config).unwrap();
        assert_eq!(
            resolved.file_name().and_then(|name| name.to_str()),
            Some("tdt-ctc-110m.tokenizer.vocab")
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn resolve_parakeet_native_vad_prefers_packaged_weights() {
        let dir = tempfile::TempDir::new().unwrap();
        let model_dir = dir.path().join("parakeet");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("silero_vad_v5.safetensors"), "vad").unwrap();

        let config = Config {
            transcription: crate::config::TranscriptionConfig {
                model_path: dir.path().to_path_buf(),
                ..crate::config::TranscriptionConfig::default()
            },
            ..Config::default()
        };

        let resolved = resolve_parakeet_native_vad_path(&config).unwrap();
        assert_eq!(
            resolved.file_name().and_then(|name| name.to_str()),
            Some("silero_vad_v5.safetensors")
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parakeet_chunk_ranges_keep_shorter_audio_intact_with_native_vad() {
        let total_samples = 16000 * 120;
        let chunk_ranges = parakeet_chunk_ranges(total_samples, 120.0, true);
        assert!(
            chunk_ranges.is_none(),
            "native VAD should avoid 45s hard chunking for 2 minute audio"
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parakeet_chunk_ranges_raise_long_audio_boundary_with_native_vad() {
        let total_samples = 16000 * 301;
        let chunk_ranges = parakeet_chunk_ranges(total_samples, 301.0, true).unwrap();
        assert_eq!(chunk_ranges.len(), 2);
        assert_eq!(chunk_ranges[0], (0, 16000 * PARAKEET_NATIVE_VAD_CHUNK_SECS));
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parakeet_chunk_ranges_preserve_legacy_guardrails_without_native_vad() {
        let total_samples = 16000 * 61;
        let chunk_ranges = parakeet_chunk_ranges(total_samples, 61.0, false).unwrap();
        assert_eq!(chunk_ranges.len(), 2);
        assert_eq!(chunk_ranges[0], (0, 16000 * PARAKEET_LONG_AUDIO_CHUNK_SECS));
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parse_parakeet_batch_output_splits_sections() {
        let config = Config::default();
        let raw = r#"
Loading model: tdt-600m (batch mode, 2 files)
Batch transcription: 2296 ms (2 files)

--- [1/2] /tmp/a.wav (2 tokens) ---
Hello there.

--- Word Timestamps ---
  [1.00s - 1.40s] (0.95) Hello
  [1.50s - 1.90s] (0.93) there.

--- [2/2] /tmp/b.wav (0 tokens) ---
"#;

        let parsed = parse_parakeet_batch_output(raw, 2, &config).unwrap();
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0]
            .as_ref()
            .unwrap()
            .transcript
            .contains("Hello there."));
        assert!(matches!(
            parsed[1],
            Err(TranscribeError::EmptyTranscript(_))
        ));
    }

    #[test]
    fn diagnosis_shows_rescue_when_all_segments_would_be_filtered() {
        let stats = FilterStats {
            audio_duration_secs: 2585.0,
            samples_after_silence_strip: 16000 * 2585,
            raw_segments: 1,
            skipped_no_speech: 0, // 0 because rescue set it to 0
            after_no_speech_filter: 1,
            after_dedup: 1,
            after_interleaved: 1,
            after_script_filter: 1,
            after_noise_markers: 1,
            after_trailing_trim: 1,
            rescued_no_speech: 1,
            final_words: 5,
        };
        let d = stats.diagnosis();
        assert!(
            d.contains("no_speech rescue: 1 segments saved"),
            "should report rescue: {}",
            d
        );
        assert!(
            !d.contains("no_speech filter:"),
            "should not show filter when rescue happened: {}",
            d
        );
        assert!(
            d.contains("final: 5 words"),
            "should show final words: {}",
            d
        );
    }

    #[test]
    fn diagnosis_shows_normal_no_speech_filter_when_some_segments_survive() {
        let stats = FilterStats {
            audio_duration_secs: 300.0,
            samples_after_silence_strip: 16000 * 300,
            raw_segments: 50,
            skipped_no_speech: 5,
            after_no_speech_filter: 45,
            after_dedup: 45,
            after_interleaved: 45,
            after_script_filter: 45,
            after_noise_markers: 45,
            after_trailing_trim: 45,
            rescued_no_speech: 0,
            final_words: 500,
        };
        let d = stats.diagnosis();
        assert!(
            d.contains("no_speech filter: -5 → 45"),
            "should show normal filter: {}",
            d
        );
        assert!(!d.contains("rescue"), "should not mention rescue: {}", d);
    }

    #[test]
    fn timeout_cap_limits_to_one_hour() {
        // 43 minutes of audio: old formula gave 300 + 2580 * 10 = 26100s (7+ hours)
        // New formula: min(300 + 2580 * 3, 3600) = 3600s (1 hour)
        let audio_secs = 2580.0_f64;
        let timeout = (300.0 + (audio_secs * 3.0)).min(3600.0);
        assert_eq!(timeout, 3600.0, "should cap at 1 hour for long audio");

        // Short audio: 30 seconds → 300 + 90 = 390s (no cap needed)
        let short_audio = 30.0_f64;
        let timeout = (300.0 + (short_audio * 3.0)).min(3600.0);
        assert_eq!(timeout, 390.0, "short audio should not be capped");
    }

    #[test]
    fn decode_hints_keep_priority_names_and_filter_weak_context() {
        let hints = DecodeHints::from_candidates(
            &[
                "Mat".to_string(),
                "Mathieu".to_string(),
                "Mat".to_string(),
                "alex@example.com".to_string(),
            ],
            &[
                "X1 Integration".to_string(),
                "Box".to_string(),
                "plan".to_string(),
                "AI".to_string(),
            ],
        );

        assert_eq!(
            hints.combined_phrases(10),
            vec![
                "Mat".to_string(),
                "Mathieu".to_string(),
                "X1 Integration".to_string(),
            ]
        );
    }

    #[test]
    fn decode_hints_build_whisper_prompt() {
        let hints = DecodeHints::from_candidates(
            &["Mat".to_string(), "Alex Chen".to_string()],
            &["X1 Planning".to_string()],
        );

        let prompt = hints.whisper_initial_prompt().expect("prompt");
        assert!(prompt.contains("Mat"));
        assert!(prompt.contains("Alex Chen"));
        assert!(prompt.contains("X1 Planning"));
        assert!(prompt.contains("Preserve spelling exactly"));
    }

    #[test]
    fn decode_hints_merge_additional_candidates() {
        let base = DecodeHints::from_candidates(&["Mat".to_string()], &["X1 Planning".to_string()]);
        let merged = base.with_additional_candidates(
            &["Casey Rowan".to_string()],
            &["Northstar Studio".to_string()],
        );

        assert_eq!(
            merged.combined_phrases(10),
            vec![
                "Mat".to_string(),
                "Casey Rowan".to_string(),
                "X1 Planning".to_string(),
                "Northstar Studio".to_string(),
            ]
        );
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn local_parakeet_hints_apply_even_when_global_boost_disabled() {
        let config = Config::default();
        let hints = DecodeHints::from_candidates(
            &["Mat".to_string(), "Alex Chen".to_string()],
            &["X1 Planning".to_string()],
        );

        let phrases = combined_parakeet_boost_phrases(&config, &hints);
        assert_eq!(phrases, vec!["Alex Chen".to_string(),]);
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn local_parakeet_hints_keep_priority_names_and_drop_context_terms() {
        let config = Config::default();
        let hints = DecodeHints::from_candidates(
            &["Casey Rowan".to_string()],
            &[
                "Northstar Studio".to_string(),
                "direct response projects".to_string(),
            ],
        );

        let phrases = combined_parakeet_boost_phrases(&config, &hints);
        assert_eq!(phrases, vec!["Casey Rowan".to_string()]);
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parakeet_gpu_unavailable_matches_runtime_error() {
        assert!(parakeet_gpu_unavailable("Error: Metal GPU not available"));
        assert!(!parakeet_gpu_unavailable("Error: tokenizer file missing"));
    }

    /// Regression guard for the loud-once log gate that
    /// `log_parakeet_helper_failure_once` and its spawn-failure twin rely
    /// on. The original issue #163 stayed invisible to maintainers
    /// partly because every helper failure was silently swallowed; the
    /// fix logs warn on first failure and debug afterward. If a future
    /// change weakens the gate (e.g. swaps `swap` for `load+store`,
    /// breaking atomicity) the log volume during a real recording goes
    /// back to one warn per audio chunk. This test exercises the gate
    /// with a non-static AtomicBool so test order does not interfere.
    #[test]
    #[cfg(feature = "parakeet")]
    fn loud_once_gate_signals_first_call_only() {
        let gate = AtomicBool::new(false);
        assert!(loud_once(&gate), "first call must signal loud");
        assert!(!loud_once(&gate), "second call must signal quiet");
        assert!(!loud_once(&gate), "third call must signal quiet");
    }

    /// Confirms the function treats each caller-owned gate independently.
    /// This is a property of `loud_once` itself; it does NOT catch a
    /// hypothetical regression where the production helpers
    /// (`log_parakeet_helper_failure_once`,
    /// `log_parakeet_helper_spawn_failure_once`) collapse onto a shared
    /// static gate, because that would require introspecting the
    /// production statics rather than the function's argument behavior.
    #[test]
    #[cfg(feature = "parakeet")]
    fn loud_once_gates_are_independent_per_instance() {
        let gate_a = AtomicBool::new(false);
        let gate_b = AtomicBool::new(false);
        assert!(loud_once(&gate_a));
        assert!(loud_once(&gate_b), "second gate must fire independently");
        assert!(!loud_once(&gate_a));
        assert!(!loud_once(&gate_b));
    }
}
