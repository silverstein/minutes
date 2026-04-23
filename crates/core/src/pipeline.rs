use crate::config::{Config, IdentityConfig};
use crate::diarize;
use crate::error::MinutesError;
use crate::logging;
use crate::markdown::{self, ContentType, Frontmatter, OutputStatus, WriteResult};
use crate::notes;
use crate::summarize;
use chrono::{DateTime, Local};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Result of Level 2 voice enrollment matching.
struct VoiceMatchResult {
    /// Speaker attributions from voice matching (one per matched label).
    attributions: Vec<diarize::SpeakerAttribution>,
    /// Whether the user's own enrolled profile exists in the database
    /// (by `config.identity.name`), regardless of whether it matched a speaker.
    self_profile_exists: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum SelfAttributionAppliedVia {
    VoiceStemMatch,
    SourceBackedStem,
    FallbackIdentityOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum SelfAttributionSkippedReason {
    NoDiarizedSpeakers,
    DiarizationNotFromStems,
    AlreadyMapped,
    NoStableLabel,
    RemoteOnlyLabel,
    NoSelfProfile,
    NoStems,
    EmptyVoiceStem,
    VoiceStemDiarizationFailed,
    VoiceStemNoSelfMatch,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SelfAttributionDebug {
    returned_some: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    applied_via: Option<SelfAttributionAppliedVia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped_reason: Option<SelfAttributionSkippedReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fallback_reason: Option<SelfAttributionSkippedReason>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SpeakerAttributionDebug {
    speaker_label: String,
    name: String,
    confidence: String,
    source: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct AttributionDebugInfo {
    capture_backend: String,
    diarization_from_stems: bool,
    raw_diarization_num_speakers: usize,
    effective_transcript_speaker_labels: Vec<String>,
    self_attribution: SelfAttributionDebug,
    final_speaker_map: Vec<SpeakerAttributionDebug>,
}

#[derive(Debug, Clone)]
struct AttributionProcessingResult {
    transcript: String,
    speaker_map: Vec<diarize::SpeakerAttribution>,
    debug: AttributionDebugInfo,
}

#[derive(Debug, Clone)]
struct SelfAttributionOutcome {
    attribution: Option<diarize::SpeakerAttribution>,
    debug: SelfAttributionDebug,
}

impl SelfAttributionOutcome {
    fn applied(
        attribution: diarize::SpeakerAttribution,
        applied_via: SelfAttributionAppliedVia,
        fallback_reason: Option<SelfAttributionSkippedReason>,
    ) -> Self {
        Self {
            debug: SelfAttributionDebug {
                returned_some: true,
                speaker_label: Some(attribution.speaker_label.clone()),
                name: Some(attribution.name.clone()),
                confidence: Some(confidence_label(attribution.confidence)),
                source: Some(attribution_source_label(attribution.source)),
                applied_via: Some(applied_via),
                skipped_reason: None,
                fallback_reason,
            },
            attribution: Some(attribution),
        }
    }

    fn skipped(reason: SelfAttributionSkippedReason) -> Self {
        Self {
            attribution: None,
            debug: SelfAttributionDebug {
                returned_some: false,
                speaker_label: None,
                name: None,
                confidence: None,
                source: None,
                applied_via: None,
                skipped_reason: Some(reason),
                fallback_reason: None,
            },
        }
    }
}

/// Match diarized speaker embeddings against enrolled voice profiles (Level 2).
///
/// For each speaker label, `match_embedding` returns at most one name — the
/// profile with the highest cosine similarity above threshold. This means each
/// label gets at most one attribution, even if multiple profiles exceed the
/// threshold.
fn match_speakers_by_voice(
    config: &Config,
    diarization_embeddings: &std::collections::HashMap<String, Vec<f32>>,
) -> VoiceMatchResult {
    if !config.voice.enabled || diarization_embeddings.is_empty() {
        return VoiceMatchResult {
            attributions: Vec::new(),
            self_profile_exists: false,
        };
    }

    let profiles = crate::voice::open_db()
        .ok()
        .and_then(|conn| crate::voice::load_all_with_embeddings(&conn).ok())
        .unwrap_or_default();

    if profiles.is_empty() {
        return VoiceMatchResult {
            attributions: Vec::new(),
            self_profile_exists: false,
        };
    }

    let self_profile_exists = config
        .identity
        .name
        .as_ref()
        .map(|name| {
            let slug = slugify(name);
            profiles.iter().any(|p| p.person_slug == slug)
        })
        .unwrap_or(false);

    let threshold = config.voice.match_threshold;
    let mut attributions = Vec::new();

    for (label, emb) in diarization_embeddings {
        if let Some(name) = crate::voice::match_embedding(emb, &profiles, threshold) {
            tracing::info!(
                speaker = %label,
                name = %name,
                threshold = threshold,
                "Level 2: voice enrollment match"
            );
            attributions.push(diarize::SpeakerAttribution {
                speaker_label: label.clone(),
                name,
                confidence: diarize::Confidence::High,
                source: diarize::AttributionSource::Enrollment,
            });
        }
    }

    VoiceMatchResult {
        attributions,
        self_profile_exists,
    }
}

fn confidence_label(confidence: diarize::Confidence) -> String {
    match confidence {
        diarize::Confidence::High => "high".into(),
        diarize::Confidence::Medium => "medium".into(),
        diarize::Confidence::Low => "low".into(),
    }
}

fn attribution_source_label(source: diarize::AttributionSource) -> String {
    match source {
        diarize::AttributionSource::Deterministic => "deterministic".into(),
        diarize::AttributionSource::Llm => "llm".into(),
        diarize::AttributionSource::Enrollment => "enrollment".into(),
        diarize::AttributionSource::Manual => "manual".into(),
    }
}

fn infer_capture_backend(audio_path: &Path, source: Option<&str>) -> String {
    if audio_path
        .components()
        .any(|component| component.as_os_str() == "native-captures")
    {
        "native-call".into()
    } else if let Some(source) = source {
        source.to_string()
    } else if audio_path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
    {
        "cpal".into()
    } else {
        "unknown".into()
    }
}

fn extract_effective_transcript_speaker_labels(transcript: &str) -> Vec<String> {
    let mut labels = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in transcript.lines() {
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(bracket_end) = rest.find(']') {
                let inside = &rest[..bracket_end];
                if let Some(space_pos) = inside.find(' ') {
                    let label = &inside[..space_pos];
                    if seen.insert(label.to_string()) {
                        labels.push(label.to_string());
                    }
                }
            }
        }
    }
    labels
}

fn debug_speaker_map(speaker_map: &[diarize::SpeakerAttribution]) -> Vec<SpeakerAttributionDebug> {
    speaker_map
        .iter()
        .map(|entry| SpeakerAttributionDebug {
            speaker_label: entry.speaker_label.clone(),
            name: entry.name.clone(),
            confidence: confidence_label(entry.confidence),
            source: attribution_source_label(entry.source),
        })
        .collect()
}

fn expected_voice_stem_path(audio_path: &Path) -> Option<std::path::PathBuf> {
    let stem = audio_path.file_stem()?.to_str()?;
    let dir = audio_path.parent()?;
    Some(dir.join(format!("{}.voice.wav", stem)))
}

fn log_attribution_decision(
    audio_path: &Path,
    output_path: &Path,
    duration_ms: u64,
    details: &AttributionDebugInfo,
) {
    let extra = serde_json::json!({
        "output": output_path.display().to_string(),
        "capture_backend": details.capture_backend,
        "diarization_from_stems": details.diarization_from_stems,
        "raw_diarization_num_speakers": details.raw_diarization_num_speakers,
        "effective_transcript_speaker_labels": details.effective_transcript_speaker_labels,
        "self_attribution": details.self_attribution,
        "speaker_map": details.final_speaker_map,
    });
    logging::log_step(
        "attribution",
        &audio_path.display().to_string(),
        duration_ms,
        extra,
    );
    tracing::info!(
        audio = %audio_path.display(),
        output = %output_path.display(),
        capture_backend = %details.capture_backend,
        diarization_from_stems = details.diarization_from_stems,
        raw_diarization_num_speakers = details.raw_diarization_num_speakers,
        effective_transcript_speaker_labels = ?details.effective_transcript_speaker_labels,
        self_attribution = ?details.self_attribution,
        speaker_map = ?details.final_speaker_map,
        "meeting attribution instrumentation"
    );
}

fn summary_signal_chars(summary: &summarize::Summary) -> usize {
    summary.text.len()
        + summary
            .decisions
            .iter()
            .map(|item| item.len())
            .sum::<usize>()
        + summary
            .action_items
            .iter()
            .map(|item| item.len())
            .sum::<usize>()
        + summary
            .open_questions
            .iter()
            .map(|item| item.len())
            .sum::<usize>()
        + summary
            .commitments
            .iter()
            .map(|item| item.len())
            .sum::<usize>()
        + summary
            .key_points
            .iter()
            .map(|item| item.len())
            .sum::<usize>()
        + summary
            .participants
            .iter()
            .map(|item| item.len())
            .sum::<usize>()
}

fn serialized_chars<T: serde::Serialize>(value: &T) -> usize {
    serde_json::to_string(value)
        .map(|json| json.len())
        .unwrap_or(0)
}

struct StructuredLlmLogFields {
    outcome: &'static str,
    model: String,
    input_chars: usize,
    output_chars: usize,
    extra: serde_json::Value,
}

fn log_structured_llm_step(
    step: &str,
    audio_path: &Path,
    started: std::time::Instant,
    fields: StructuredLlmLogFields,
) {
    let mut payload = serde_json::Map::from_iter([
        ("outcome".to_string(), serde_json::json!(fields.outcome)),
        ("model".to_string(), serde_json::json!(fields.model)),
        (
            "input_chars".to_string(),
            serde_json::json!(fields.input_chars),
        ),
        (
            "output_chars".to_string(),
            serde_json::json!(fields.output_chars),
        ),
    ]);
    if let Some(obj) = fields.extra.as_object() {
        payload.extend(obj.clone());
    }
    logging::log_step(
        step,
        &audio_path.display().to_string(),
        started.elapsed().as_millis() as u64,
        serde_json::Value::Object(payload),
    );
}

#[allow(clippy::too_many_arguments)]
fn single_stem_speaker_self_attribution(
    audio_path: &Path,
    config: &Config,
    voice_result: &VoiceMatchResult,
    diarization_from_stems: bool,
    transcript: &str,
    transcript_labels: &[String],
    already_mapped_labels: &std::collections::HashSet<String>,
) -> SelfAttributionOutcome {
    if !diarization_from_stems || !already_mapped_labels.is_empty() {
        return if !diarization_from_stems {
            SelfAttributionOutcome::skipped(SelfAttributionSkippedReason::DiarizationNotFromStems)
        } else {
            SelfAttributionOutcome::skipped(SelfAttributionSkippedReason::AlreadyMapped)
        };
    }

    let source_backed_speaker_label = if transcript_labels.iter().any(|label| label == "SPEAKER_0")
    {
        Some("SPEAKER_0".to_string())
    } else {
        None
    };
    let speaker_label = if let Some(label) = source_backed_speaker_label.clone() {
        label
    } else if transcript_labels.len() == 1 && transcript_labels[0] == "SPEAKER_1" {
        "SPEAKER_1".to_string()
    } else if transcript.contains("[UNKNOWN ") {
        "UNKNOWN".to_string()
    } else {
        return SelfAttributionOutcome::skipped(SelfAttributionSkippedReason::NoStableLabel);
    };

    let Some(my_name) = config.identity.name.as_ref() else {
        return SelfAttributionOutcome::skipped(SelfAttributionSkippedReason::NoSelfProfile);
    };
    if let Some(voice_stem_path) = expected_voice_stem_path(audio_path) {
        if let Ok(metadata) = std::fs::metadata(&voice_stem_path) {
            if metadata.len() <= 44 {
                return SelfAttributionOutcome::skipped(
                    SelfAttributionSkippedReason::EmptyVoiceStem,
                );
            }
        }
    }
    if let Some(stems) = diarize::discover_stems(audio_path) {
        if let Some(source_backed_label) = source_backed_speaker_label.clone() {
            if let Some(voice_stem_result) = diarize::diarize(&stems.voice, config) {
                let matched_self =
                    match_speakers_by_voice(config, &voice_stem_result.speaker_embeddings)
                        .attributions
                        .iter()
                        .any(|attr| attr.name == *my_name);
                return SelfAttributionOutcome::applied(
                    diarize::SpeakerAttribution {
                        speaker_label: source_backed_label,
                        name: my_name.clone(),
                        confidence: if matched_self {
                            diarize::Confidence::High
                        } else {
                            diarize::Confidence::Medium
                        },
                        source: if matched_self {
                            diarize::AttributionSource::Enrollment
                        } else {
                            diarize::AttributionSource::Deterministic
                        },
                    },
                    if matched_self {
                        SelfAttributionAppliedVia::VoiceStemMatch
                    } else {
                        SelfAttributionAppliedVia::SourceBackedStem
                    },
                    None,
                );
            }

            return SelfAttributionOutcome::applied(
                diarize::SpeakerAttribution {
                    speaker_label: source_backed_label,
                    name: my_name.clone(),
                    confidence: diarize::Confidence::Medium,
                    source: diarize::AttributionSource::Deterministic,
                },
                SelfAttributionAppliedVia::SourceBackedStem,
                Some(SelfAttributionSkippedReason::VoiceStemDiarizationFailed),
            );
        }

        if let Some(voice_stem_result) = diarize::diarize(&stems.voice, config) {
            let _ = voice_stem_result;
            if speaker_label == "SPEAKER_1" {
                return SelfAttributionOutcome::skipped(
                    SelfAttributionSkippedReason::RemoteOnlyLabel,
                );
            }

            return SelfAttributionOutcome::skipped(
                SelfAttributionSkippedReason::VoiceStemNoSelfMatch,
            );
        }

        return SelfAttributionOutcome::skipped(
            SelfAttributionSkippedReason::VoiceStemDiarizationFailed,
        );
    }

    if speaker_label == "SPEAKER_1" {
        return SelfAttributionOutcome::skipped(SelfAttributionSkippedReason::RemoteOnlyLabel);
    }

    SelfAttributionOutcome::applied(
        diarize::SpeakerAttribution {
            speaker_label,
            name: my_name.clone(),
            confidence: diarize::Confidence::Medium,
            source: diarize::AttributionSource::Deterministic,
        },
        SelfAttributionAppliedVia::FallbackIdentityOnly,
        Some(if voice_result.self_profile_exists {
            SelfAttributionSkippedReason::NoStems
        } else {
            SelfAttributionSkippedReason::NoSelfProfile
        }),
    )
}

#[allow(clippy::too_many_arguments)]
fn attribute_meeting_speakers(
    audio_path: &Path,
    content_type: ContentType,
    source: Option<&str>,
    config: &Config,
    trusted_attendees: &[String],
    llm_attendees: &[String],
    diarization_num_speakers: usize,
    diarization_from_stems: bool,
    diarization_embeddings: &std::collections::HashMap<String, Vec<f32>>,
    transcript: String,
) -> AttributionProcessingResult {
    let mut transcript = transcript;
    let mut speaker_map: Vec<diarize::SpeakerAttribution> = Vec::new();
    let capture_backend = infer_capture_backend(audio_path, source);

    let self_attribution = if content_type == ContentType::Meeting && diarization_num_speakers == 0
    {
        SelfAttributionOutcome::skipped(SelfAttributionSkippedReason::NoDiarizedSpeakers)
    } else if content_type != ContentType::Meeting {
        SelfAttributionOutcome::skipped(SelfAttributionSkippedReason::NoStableLabel)
    } else {
        let voice_result = match_speakers_by_voice(config, diarization_embeddings);
        speaker_map.extend(voice_result.attributions.clone());

        let transcript_labels = crate::summarize::extract_speaker_labels_pub(&transcript);
        let l2_labels: std::collections::HashSet<String> = speaker_map
            .iter()
            .map(|a| a.speaker_label.clone())
            .collect();

        if !trusted_attendees.is_empty()
            && diarization_num_speakers == trusted_attendees.len()
            && diarization_num_speakers == 2
            && transcript_labels.len() == 2
            && l2_labels.is_empty()
        {
            if let Some(my_name) = config.identity.name.as_ref() {
                let my_slug = slugify(my_name);
                let other = trusted_attendees
                    .iter()
                    .find(|attendee| slugify(attendee) != my_slug);
                if let Some(other_name) = other {
                    speaker_map.push(diarize::SpeakerAttribution {
                        speaker_label: transcript_labels[0].clone(),
                        name: my_name.clone(),
                        confidence: diarize::Confidence::Medium,
                        source: diarize::AttributionSource::Deterministic,
                    });
                    speaker_map.push(diarize::SpeakerAttribution {
                        speaker_label: transcript_labels[1].clone(),
                        name: other_name.clone(),
                        confidence: diarize::Confidence::Medium,
                        source: diarize::AttributionSource::Deterministic,
                    });
                    tracing::info!(
                        my_name = %my_name,
                        other_name = %other_name,
                        labels = ?transcript_labels,
                        "Level 0: deterministic 1-on-1 speaker attribution"
                    );
                }
            }
        }

        // Recompute the mapped-labels set so it reflects BOTH L2 voice matches
        // (extended into speaker_map at line 456) AND any L0 deterministic
        // mapping that just fired above. l2_labels was captured before L0,
        // so passing it here would let self_attribution duplicate a label
        // L0 already mapped (regression introduced by f15a7e8).
        let already_mapped_labels: std::collections::HashSet<String> = speaker_map
            .iter()
            .map(|a| a.speaker_label.clone())
            .collect();
        let self_attribution = single_stem_speaker_self_attribution(
            audio_path,
            config,
            &voice_result,
            diarization_from_stems,
            &transcript,
            &transcript_labels,
            &already_mapped_labels,
        );
        if let Some(attr) = self_attribution.attribution.clone() {
            speaker_map.push(attr);
        }

        let mapped_labels: std::collections::HashSet<String> = speaker_map
            .iter()
            .map(|attribution| attribution.speaker_label.clone())
            .collect();
        let has_unmapped = transcript.lines().any(|line| {
            if let Some(rest) = line.strip_prefix('[') {
                if let Some(bracket_end) = rest.find(']') {
                    let inside = &rest[..bracket_end];
                    if let Some(space_pos) = inside.find(' ') {
                        let label = &inside[..space_pos];
                        return label.starts_with("SPEAKER_") && !mapped_labels.contains(label);
                    }
                }
            }
            false
        });
        if has_unmapped {
            // Keep L0 deterministic mapping fenced to trusted attendees; the
            // broader merged attendee list is only for the L1 name-mapping fallback.
            let log_file = audio_path.display().to_string();
            for attribution in
                summarize::map_speakers(&transcript, llm_attendees, config, Some(&log_file))
            {
                if !mapped_labels.contains(&attribution.speaker_label) {
                    speaker_map.push(attribution);
                }
            }
        }

        let effective_transcript_speaker_labels =
            extract_effective_transcript_speaker_labels(&transcript);

        if speaker_map
            .iter()
            .any(|attribution| attribution.confidence == diarize::Confidence::High)
        {
            transcript = diarize::apply_confirmed_names(&transcript, &speaker_map);
        }

        return AttributionProcessingResult {
            debug: AttributionDebugInfo {
                capture_backend,
                diarization_from_stems,
                raw_diarization_num_speakers: diarization_num_speakers,
                effective_transcript_speaker_labels,
                self_attribution: self_attribution.debug,
                final_speaker_map: debug_speaker_map(&speaker_map),
            },
            transcript,
            speaker_map,
        };
    };

    AttributionProcessingResult {
        debug: AttributionDebugInfo {
            capture_backend,
            diarization_from_stems,
            raw_diarization_num_speakers: diarization_num_speakers,
            effective_transcript_speaker_labels: extract_effective_transcript_speaker_labels(
                &transcript,
            ),
            self_attribution: self_attribution.debug,
            final_speaker_map: debug_speaker_map(&speaker_map),
        },
        transcript,
        speaker_map,
    }
}

// ──────────────────────────────────────────────────────────────
// Pipeline orchestration:
//
//   Audio → Transcribe → [Diarize] → [Summarize] → Write Markdown
//                           ▲             ▲
//                           │             │
//                     config.diarization  config.summarization
//                     .engine != "none"   .engine != "none"
//
// Transcription uses whisper-rs (whisper.cpp) with symphonia for
// format conversion (m4a/mp3/ogg → 16kHz mono PCM).
// Phase 1b adds Diarize + Summarize with if-guards.
// ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PipelineStage {
    Transcribing,
    Diarizing,
    Summarizing,
    Saving,
}

#[derive(Debug, Clone, Default)]
pub struct BackgroundPipelineContext {
    pub sidecar: Option<SidecarMetadata>,
    pub user_notes: Option<String>,
    pub pre_context: Option<String>,
    pub calendar_event: Option<crate::calendar::CalendarEvent>,
    pub recorded_at: Option<DateTime<Local>>,
    pub requested_title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TranscriptArtifact {
    pub write_result: WriteResult,
    pub frontmatter: Frontmatter,
    pub transcript: String,
}

/// Optional metadata from a sidecar JSON file (e.g., from iPhone Apple Shortcut).
/// Merged into frontmatter when present.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SidecarMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<chrono::DateTime<Local>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Process an audio file through the full pipeline.
pub fn process(
    audio_path: &Path,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
) -> Result<WriteResult, MinutesError> {
    process_with_sidecar(audio_path, content_type, title, config, None, |_| {})
}

/// Process an audio file with optional sidecar metadata (from iPhone, etc.).
pub fn process_with_sidecar<F>(
    audio_path: &Path,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
    sidecar: Option<&SidecarMetadata>,
    on_progress: F,
) -> Result<WriteResult, MinutesError>
where
    F: FnMut(PipelineStage),
{
    process_with_progress_and_sidecar(
        audio_path,
        content_type,
        title,
        config,
        sidecar,
        on_progress,
    )
}

pub fn process_with_progress<F>(
    audio_path: &Path,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
    on_progress: F,
) -> Result<WriteResult, MinutesError>
where
    F: FnMut(PipelineStage),
{
    process_with_progress_and_sidecar(audio_path, content_type, title, config, None, on_progress)
}

pub fn transcribe_to_artifact(
    audio_path: &Path,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
    context: &BackgroundPipelineContext,
    existing_output_path: Option<&Path>,
) -> Result<TranscriptArtifact, MinutesError> {
    let metadata = std::fs::metadata(audio_path)?;
    if metadata.len() == 0 {
        return Err(crate::error::TranscribeError::EmptyAudio.into());
    }
    let recording_date =
        infer_recording_date(context.recorded_at, context.sidecar.as_ref(), &metadata);

    if let Ok(canonical) = audio_path.canonicalize() {
        let allowed = &config.security.allowed_audio_dirs;
        if !allowed.is_empty() {
            let in_allowed = allowed.iter().any(|dir| {
                dir.canonicalize()
                    .map(|d| canonical.starts_with(&d))
                    .unwrap_or(false)
            });
            if !in_allowed {
                return Err(crate::error::TranscribeError::UnsupportedFormat(format!(
                    "file not in allowed directories: {}",
                    audio_path.display()
                ))
                .into());
            }
        }
    }

    let matched_event = if content_type == ContentType::Meeting {
        context.calendar_event.clone().or_else(|| {
            select_calendar_event(&crate::calendar::events_overlapping(recording_date), title)
        })
    } else {
        None
    };
    let calendar_event_title = matched_event.as_ref().map(|event| event.title.clone());
    let attendees = matched_event
        .as_ref()
        .map(|event| event.attendees.clone())
        .unwrap_or_default();
    let decode_hints = build_decode_hints(
        title,
        calendar_event_title.as_deref(),
        context.pre_context.as_deref(),
        &attendees,
        Some(&config.identity),
    );

    let step_start = std::time::Instant::now();
    let result = crate::transcription_coordinator::transcribe_path_for_content_with_hints(
        audio_path,
        content_type,
        config,
        decode_hints,
    )?;
    let transcript = if content_type == ContentType::Meeting {
        normalize_transcript_for_self_name_participant(&result.text, &attendees, &config.identity)
    } else {
        result.text
    };
    let filter_stats = result.stats;
    write_transcript_artifact(
        audio_path,
        content_type,
        title,
        config,
        context,
        existing_output_path,
        transcript,
        filter_stats,
        step_start.elapsed().as_millis() as u64,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn write_transcript_artifact(
    audio_path: &Path,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
    context: &BackgroundPipelineContext,
    existing_output_path: Option<&Path>,
    transcript: String,
    filter_stats: crate::transcribe::FilterStats,
    transcribe_ms: u64,
) -> Result<TranscriptArtifact, MinutesError> {
    let metadata = std::fs::metadata(audio_path)?;
    let recording_date =
        infer_recording_date(context.recorded_at, context.sidecar.as_ref(), &metadata);
    let matched_event = if content_type == ContentType::Meeting {
        context.calendar_event.clone().or_else(|| {
            select_calendar_event(&crate::calendar::events_overlapping(recording_date), title)
        })
    } else {
        None
    };
    let calendar_event_title = matched_event.as_ref().map(|event| event.title.clone());
    let attendees = matched_event
        .as_ref()
        .map(|event| event.attendees.clone())
        .unwrap_or_default();
    let transcript = if content_type == ContentType::Meeting {
        normalize_transcript_for_self_name_participant(&transcript, &attendees, &config.identity)
    } else {
        transcript
    };
    let word_count = transcript.split_whitespace().count();
    logging::log_step(
        "transcribe",
        &audio_path.display().to_string(),
        transcribe_ms,
        serde_json::json!({"words": word_count, "mode": "background", "diagnosis": filter_stats.diagnosis()}),
    );

    let status = if word_count < config.transcription.min_words {
        Some(OutputStatus::NoSpeech)
    } else {
        Some(OutputStatus::TranscriptOnly)
    };

    let auto_title = title.map(String::from).unwrap_or_else(|| {
        if status == Some(OutputStatus::NoSpeech) {
            "Untitled Recording".into()
        } else {
            calendar_event_title
                .as_deref()
                .and_then(title_from_context)
                .map(finalize_title)
                .unwrap_or_else(|| generate_title(&transcript, context.pre_context.as_deref()))
        }
    });

    let entities = build_entity_links(
        &auto_title,
        context.pre_context.as_deref(),
        &attendees,
        &[],
        &[],
        &[],
        &[],
        Some(&config.identity),
    );
    let people = entities
        .people
        .iter()
        .map(|entity| entity.label.clone())
        .collect();

    let source = if let Some(source) = context
        .sidecar
        .as_ref()
        .and_then(|sidecar| sidecar.source.clone())
    {
        Some(source)
    } else {
        match content_type {
            ContentType::Memo => Some("voice-memos".into()),
            ContentType::Meeting => None,
            ContentType::Dictation => Some("dictation".into()),
        }
    };
    let tags = derive_structured_tags(
        content_type,
        source.as_deref(),
        context
            .sidecar
            .as_ref()
            .and_then(|sidecar| sidecar.device.as_deref()),
        &entities,
        &[],
        &[],
    );

    let frontmatter = Frontmatter {
        title: auto_title,
        r#type: content_type,
        date: recording_date,
        duration: estimate_duration(audio_path),
        source,
        status,
        tags,
        attendees,
        attendees_raw: None,
        calendar_event: calendar_event_title,
        people,
        entities,
        device: context
            .sidecar
            .as_ref()
            .and_then(|sidecar| sidecar.device.clone()),
        captured_at: context
            .sidecar
            .as_ref()
            .and_then(|sidecar| sidecar.captured_at),
        context: context.pre_context.clone(),
        action_items: vec![],
        decisions: vec![],
        intents: vec![],
        recorded_by: config.identity.name.clone(),
        visibility: None,
        speaker_map: vec![],
        filter_diagnosis: if status == Some(OutputStatus::NoSpeech) {
            Some(filter_stats.diagnosis())
        } else {
            None
        },
    };

    let write_result = if let Some(path) = existing_output_path {
        markdown::rewrite_with_retry_path(
            path,
            &frontmatter,
            &transcript,
            None,
            context.user_notes.as_deref(),
            Some(audio_path),
        )?
    } else {
        markdown::write_with_retry_path(
            &frontmatter,
            &transcript,
            None,
            context.user_notes.as_deref(),
            Some(audio_path),
            config,
        )?
    };

    Ok(TranscriptArtifact {
        write_result,
        frontmatter,
        transcript,
    })
}

pub fn enrich_transcript_artifact<F>(
    audio_path: &Path,
    artifact: &TranscriptArtifact,
    config: &Config,
    context: &BackgroundPipelineContext,
    mut on_progress: F,
) -> Result<WriteResult, MinutesError>
where
    F: FnMut(PipelineStage),
{
    if artifact.frontmatter.status == Some(OutputStatus::NoSpeech) {
        return Ok(artifact.write_result.clone());
    }

    let mut transcript = artifact.transcript.clone();
    let mut diarization_num_speakers = 0usize;
    let mut diarization_from_stems = false;
    let mut diarization_embeddings: std::collections::HashMap<String, Vec<f32>> =
        std::collections::HashMap::new();
    if config.diarization.engine != "none" && artifact.frontmatter.r#type == ContentType::Meeting {
        on_progress(PipelineStage::Diarizing);
        let diarize_start = std::time::Instant::now();
        if let Some(result) = diarize::diarize(audio_path, config) {
            let diarize_ms = diarize_start.elapsed().as_millis() as u64;
            diarization_num_speakers = result.num_speakers;
            diarization_from_stems = result.source_aware;
            diarization_embeddings = result.speaker_embeddings.clone();
            logging::log_step(
                "diarize",
                &audio_path.display().to_string(),
                diarize_ms,
                serde_json::json!({
                    "speakers": result.num_speakers,
                    "segments": result.segments.len(),
                    "first_segment_start": result.segments.first().map(|s| s.start),
                    "last_segment_end": result.segments.last().map(|s| s.end),
                }),
            );
            transcript = diarize::apply_speakers(&transcript, &result);
        } else {
            logging::log_step(
                "diarize",
                &audio_path.display().to_string(),
                diarize_start.elapsed().as_millis() as u64,
                serde_json::json!({"skipped": true}),
            );
        }
    }

    let screen_dir = crate::screen::screens_dir_for(audio_path);
    let screen_files = if screen_dir.exists() {
        crate::screen::list_screenshots(&screen_dir)
    } else {
        vec![]
    };

    let mut summary_participants: Vec<String> = Vec::new();
    let mut structured_actions: Vec<markdown::ActionItem> = Vec::new();
    let mut structured_decisions: Vec<markdown::Decision> = Vec::new();
    let mut structured_intents: Vec<markdown::Intent> = Vec::new();
    let audio_log_target = audio_path.display().to_string();
    let summary_model = summarize::summarization_model_hint(config, !screen_files.is_empty());

    let mut raw_summary: Option<summarize::Summary> = None;
    let summary = if config.summarization.engine != "none" {
        on_progress(PipelineStage::Summarizing);
        let transcript_with_notes = if let Some(notes) = context.user_notes.as_ref() {
            format!(
                "USER NOTES (these moments were marked as important — weight them heavily):\n{}\n\nTRANSCRIPT:\n{}",
                notes, transcript
            )
        } else {
            transcript.clone()
        };

        summarize::summarize_with_screens(
            &transcript_with_notes,
            &screen_files,
            config,
            Some(&audio_log_target),
        )
        .map(|summary| {
            let summary_chars = summary_signal_chars(&summary);

            let actions_started = std::time::Instant::now();
            structured_actions = extract_action_items(&summary);
            log_structured_llm_step(
                "action_items",
                audio_path,
                actions_started,
                StructuredLlmLogFields {
                    outcome: if structured_actions.is_empty() {
                        "empty"
                    } else {
                        "ok"
                    },
                    model: summary_model.clone(),
                    input_chars: summary_chars,
                    output_chars: serialized_chars(&structured_actions),
                    extra: serde_json::json!({ "count": structured_actions.len() }),
                },
            );

            structured_decisions = extract_decisions(&summary);

            let intents_started = std::time::Instant::now();
            structured_intents = extract_intents(&summary);
            log_structured_llm_step(
                "intent_extract",
                audio_path,
                intents_started,
                StructuredLlmLogFields {
                    outcome: if structured_intents.is_empty() {
                        "empty"
                    } else {
                        "ok"
                    },
                    model: summary_model.clone(),
                    input_chars: summary_chars,
                    output_chars: serialized_chars(&structured_intents),
                    extra: serde_json::json!({ "count": structured_intents.len() }),
                },
            );

            summary_participants = summary.participants.clone();
            let formatted = summarize::format_summary(&summary);
            raw_summary = Some(summary);
            formatted
        })
    } else {
        None
    };
    if summary.is_none() && config.summarization.engine != "none" {
        log_structured_llm_step(
            "action_items",
            audio_path,
            std::time::Instant::now(),
            StructuredLlmLogFields {
                outcome: "fallback",
                model: summary_model.clone(),
                input_chars: transcript.len(),
                output_chars: 0,
                extra: serde_json::json!({ "count": 0 }),
            },
        );
        log_structured_llm_step(
            "intent_extract",
            audio_path,
            std::time::Instant::now(),
            StructuredLlmLogFields {
                outcome: "fallback",
                model: summary_model.clone(),
                input_chars: transcript.len(),
                output_chars: 0,
                extra: serde_json::json!({ "count": 0 }),
            },
        );
    }

    if !screen_files.is_empty()
        && !config.screen_context.keep_after_summary
        && std::fs::remove_dir_all(&screen_dir).is_ok()
    {
        tracing::info!(dir = %screen_dir.display(), "screen captures cleaned up");
    }

    let attendees = merge_attendees(&artifact.frontmatter.attendees, &summary_participants);

    let attribution_start = std::time::Instant::now();
    let attribution = attribute_meeting_speakers(
        audio_path,
        artifact.frontmatter.r#type,
        artifact.frontmatter.source.as_deref(),
        config,
        &artifact.frontmatter.attendees,
        &attendees,
        diarization_num_speakers,
        diarization_from_stems,
        &diarization_embeddings,
        transcript,
    );
    let attribution_ms = attribution_start.elapsed().as_millis() as u64;
    transcript = attribution.transcript;
    let speaker_map = attribution.speaker_map;
    let attendees = normalize_attendees_with_speaker_map(&attendees, &speaker_map);
    let structured_actions =
        normalize_action_items_with_speaker_map(structured_actions, &speaker_map);
    let structured_intents = normalize_intents_with_speaker_map(structured_intents, &speaker_map);
    let structured_decisions =
        normalize_decisions_with_speaker_map(structured_decisions, &speaker_map);

    let entities_started = std::time::Instant::now();
    let entities = build_entity_links(
        &artifact.frontmatter.title,
        context.pre_context.as_deref(),
        &attendees,
        &structured_actions,
        &structured_decisions,
        &structured_intents,
        &artifact.frontmatter.tags,
        Some(&config.identity),
    );
    log_structured_llm_step(
        "entity_extract",
        audio_path,
        entities_started,
        StructuredLlmLogFields {
            outcome: if entities.people.is_empty() && entities.projects.is_empty() {
                "empty"
            } else if raw_summary.is_some() {
                "ok"
            } else {
                "fallback"
            },
            model: summary_model.clone(),
            input_chars: transcript.len(),
            output_chars: serialized_chars(&entities),
            extra: serde_json::json!({
                "people": entities.people.len(),
                "projects": entities.projects.len(),
            }),
        },
    );
    let people = entities
        .people
        .iter()
        .map(|entity| entity.label.clone())
        .collect();
    let title_generation = maybe_refine_title_with_llm(
        &artifact.frontmatter.title,
        context.requested_title.as_deref(),
        summary.as_deref(),
        raw_summary.as_ref(),
        &entities,
        config,
        summarize::refine_title,
    );

    let mut frontmatter = artifact.frontmatter.clone();
    frontmatter.status = if config.summarization.engine != "none" {
        Some(OutputStatus::Complete)
    } else {
        Some(OutputStatus::TranscriptOnly)
    };
    frontmatter.attendees = attendees;
    frontmatter.people = people;
    frontmatter.entities = entities;
    frontmatter.action_items = structured_actions;
    frontmatter.decisions = structured_decisions;
    frontmatter.intents = structured_intents;
    frontmatter.speaker_map = speaker_map;

    on_progress(PipelineStage::Saving);
    let mut result = markdown::rewrite_with_retry_path(
        &artifact.write_result.path,
        &frontmatter,
        &transcript,
        summary.as_deref(),
        context.user_notes.as_deref(),
        Some(audio_path),
    )?;
    apply_title_generation(
        audio_path,
        &mut result,
        &mut frontmatter,
        title_generation,
        |duration_ms, extra| {
            logging::log_step(
                "title_generation",
                &audio_path.display().to_string(),
                duration_ms,
                extra,
            );
        },
    );

    if frontmatter.r#type == ContentType::Meeting {
        log_attribution_decision(audio_path, &result.path, attribution_ms, &attribution.debug);
    }

    if !diarization_embeddings.is_empty() {
        crate::voice::save_meeting_embeddings(&result.path, &diarization_embeddings);
    }

    // Emit structured insight events for agent subscription
    if let Some(ref summary_data) = raw_summary {
        crate::events::emit_insights_from_summary(
            summary_data,
            &result.path.display().to_string(),
            &frontmatter.title,
            &frontmatter.attendees,
        );
    }

    if let Err(error) = crate::daily_notes::append_backlink(
        &result,
        frontmatter.date,
        summary.as_deref(),
        Some(&frontmatter),
        config,
    ) {
        tracing::warn!(
            error = %error,
            output = %result.path.display(),
            "failed to append daily note backlink"
        );
    }

    match crate::vault::sync_file(&result.path, config) {
        Ok(Some(vault_path)) => {
            crate::events::append_event(crate::events::MinutesEvent::VaultSynced {
                source_path: result.path.display().to_string(),
                vault_path: vault_path.display().to_string(),
                strategy: config.vault.strategy.clone(),
            });
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(error = %error, output = %result.path.display(), "vault sync failed");
        }
    }

    if config.knowledge.enabled {
        match crate::knowledge::update_from_meeting(&result, &frontmatter, &transcript, config) {
            Ok(update) => {
                if update.facts_written > 0 {
                    tracing::info!(
                        facts_written = update.facts_written,
                        facts_skipped = update.facts_skipped,
                        people = ?update.people_updated,
                        "knowledge base updated"
                    );
                    crate::events::append_event(crate::events::MinutesEvent::KnowledgeUpdated {
                        meeting_path: result.path.display().to_string(),
                        facts_written: update.facts_written,
                        facts_skipped: update.facts_skipped,
                        people_updated: update.people_updated,
                    });
                }
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    meeting = %result.path.display(),
                    "knowledge update failed"
                );
            }
        }
    }

    Ok(result)
}

fn process_with_progress_and_sidecar<F>(
    audio_path: &Path,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
    sidecar: Option<&SidecarMetadata>,
    mut on_progress: F,
) -> Result<WriteResult, MinutesError>
where
    F: FnMut(PipelineStage),
{
    let start = std::time::Instant::now();
    tracing::info!(
        file = %audio_path.display(),
        content_type = ?content_type,
        "starting pipeline"
    );

    // Verify file exists and is not empty
    let metadata = std::fs::metadata(audio_path)?;
    let recording_date =
        infer_recording_date(sidecar.and_then(|s| s.captured_at), sidecar, &metadata);
    if metadata.len() == 0 {
        return Err(crate::error::TranscribeError::EmptyAudio.into());
    }

    // Security: verify file is in an allowed directory (prevents path traversal via MCP)
    if let Ok(canonical) = audio_path.canonicalize() {
        let allowed = &config.security.allowed_audio_dirs;
        if !allowed.is_empty() {
            let in_allowed = allowed.iter().any(|dir| {
                dir.canonicalize()
                    .map(|d| canonical.starts_with(&d))
                    .unwrap_or(false)
            });
            if !in_allowed {
                return Err(crate::error::TranscribeError::UnsupportedFormat(format!(
                    "file not in allowed directories: {}",
                    audio_path.display()
                ))
                .into());
            }
        }
    }

    // Read user notes and pre-meeting context before transcription so they can
    // inform batch decode hints.
    let user_notes = notes::read_notes();
    let pre_context = notes::read_context();

    let calendar_events = if content_type == ContentType::Meeting {
        crate::calendar::events_overlapping(recording_date)
    } else {
        Vec::new()
    };
    let matched_event = select_calendar_event(&calendar_events, title);
    let calendar_event_title = matched_event.as_ref().map(|e| e.title.clone());
    let calendar_attendees: Vec<String> = matched_event
        .as_ref()
        .map(|e| e.attendees.clone())
        .unwrap_or_default();
    let decode_hints = build_decode_hints(
        title,
        calendar_event_title.as_deref(),
        pre_context.as_deref(),
        &calendar_attendees,
        Some(&config.identity),
    );

    // Step 1: Transcribe (always)
    on_progress(PipelineStage::Transcribing);
    tracing::info!(step = "transcribe", file = %audio_path.display(), "transcribing audio");
    let step_start = std::time::Instant::now();
    let result = crate::transcription_coordinator::transcribe_path_for_content_with_hints(
        audio_path,
        content_type,
        config,
        decode_hints,
    )?;
    let transcribe_ms = step_start.elapsed().as_millis() as u64;
    let transcript = if content_type == ContentType::Meeting {
        normalize_transcript_for_self_name_participant(
            &result.text,
            &calendar_attendees,
            &config.identity,
        )
    } else {
        result.text
    };
    let filter_stats = result.stats;

    let word_count = transcript.split_whitespace().count();
    tracing::info!(
        step = "transcribe",
        words = word_count,
        diagnosis = filter_stats.diagnosis(),
        "transcription complete"
    );
    logging::log_step(
        "transcribe",
        &audio_path.display().to_string(),
        transcribe_ms,
        serde_json::json!({"words": word_count, "diagnosis": filter_stats.diagnosis()}),
    );

    // Check minimum word threshold
    let status = if word_count < config.transcription.min_words {
        tracing::warn!(
            words = word_count,
            min = config.transcription.min_words,
            diagnosis = filter_stats.diagnosis(),
            "below minimum word threshold — marking as no-speech"
        );
        Some(OutputStatus::NoSpeech)
    } else if config.summarization.engine != "none" {
        Some(OutputStatus::Complete)
    } else {
        Some(OutputStatus::TranscriptOnly)
    };

    // Step 2: Diarize (optional — depends on config.diarization.engine)
    let mut diarization_num_speakers: usize = 0;
    let mut diarization_from_stems = false;
    let mut diarization_embeddings: std::collections::HashMap<String, Vec<f32>> =
        std::collections::HashMap::new();
    let transcript = if config.diarization.engine != "none" && content_type == ContentType::Meeting
    {
        on_progress(PipelineStage::Diarizing);
        tracing::info!(step = "diarize", "running speaker diarization");
        if let Some(result) = diarize::diarize(audio_path, config) {
            diarization_num_speakers = result.num_speakers;
            diarization_from_stems = result.source_aware;
            diarization_embeddings = result.speaker_embeddings.clone();
            diarize::apply_speakers(&transcript, &result)
        } else {
            transcript
        }
    } else {
        transcript
    };

    // Step 3: Summarize (optional — depends on config.summarization.engine)
    // Pass user notes to the summarizer as high-priority context
    // Step 3: Summarize + extract structured intent
    let mut structured_actions: Vec<markdown::ActionItem> = Vec::new();
    let mut structured_decisions: Vec<markdown::Decision> = Vec::new();
    let mut structured_intents: Vec<markdown::Intent> = Vec::new();

    // Collect screen context screenshots (if any were captured)
    let screen_dir = crate::screen::screens_dir_for(audio_path);
    let screen_files = if screen_dir.exists() {
        let files = crate::screen::list_screenshots(&screen_dir);
        if !files.is_empty() {
            tracing::info!(count = files.len(), "screen context screenshots found");
        }
        files
    } else {
        vec![]
    };

    let mut summary_participants: Vec<String> = Vec::new();
    let audio_log_target = audio_path.display().to_string();
    let summary_model = summarize::summarization_model_hint(config, !screen_files.is_empty());

    let mut raw_summary: Option<summarize::Summary> = None;
    let summary: Option<String> = if config.summarization.engine != "none" {
        on_progress(PipelineStage::Summarizing);
        tracing::info!(step = "summarize", "generating summary");

        // Build transcript with user notes as context
        let transcript_with_notes = if let Some(ref n) = user_notes {
            format!(
                "USER NOTES (these moments were marked as important — weight them heavily):\n{}\n\nTRANSCRIPT:\n{}",
                n, transcript
            )
        } else {
            transcript.clone()
        };

        // Send screenshots as actual images to vision-capable LLMs
        summarize::summarize_with_screens(
            &transcript_with_notes,
            &screen_files,
            config,
            Some(&audio_log_target),
        )
        .map(|s| {
            let summary_chars = summary_signal_chars(&s);

            let actions_started = std::time::Instant::now();
            structured_actions = extract_action_items(&s);
            log_structured_llm_step(
                "action_items",
                audio_path,
                actions_started,
                StructuredLlmLogFields {
                    outcome: if structured_actions.is_empty() {
                        "empty"
                    } else {
                        "ok"
                    },
                    model: summary_model.clone(),
                    input_chars: summary_chars,
                    output_chars: serialized_chars(&structured_actions),
                    extra: serde_json::json!({ "count": structured_actions.len() }),
                },
            );

            structured_decisions = extract_decisions(&s);

            let intents_started = std::time::Instant::now();
            structured_intents = extract_intents(&s);
            log_structured_llm_step(
                "intent_extract",
                audio_path,
                intents_started,
                StructuredLlmLogFields {
                    outcome: if structured_intents.is_empty() {
                        "empty"
                    } else {
                        "ok"
                    },
                    model: summary_model.clone(),
                    input_chars: summary_chars,
                    output_chars: serialized_chars(&structured_intents),
                    extra: serde_json::json!({ "count": structured_intents.len() }),
                },
            );

            summary_participants = s.participants.clone();
            if !summary_participants.is_empty() {
                tracing::info!(
                    participants = ?summary_participants,
                    "extracted participants from summary"
                );
            }
            let formatted = summarize::format_summary(&s);
            raw_summary = Some(s);
            formatted
        })
    } else {
        None
    };
    if summary.is_none() && config.summarization.engine != "none" {
        log_structured_llm_step(
            "action_items",
            audio_path,
            std::time::Instant::now(),
            StructuredLlmLogFields {
                outcome: "fallback",
                model: summary_model.clone(),
                input_chars: transcript.len(),
                output_chars: 0,
                extra: serde_json::json!({ "count": 0 }),
            },
        );
        log_structured_llm_step(
            "intent_extract",
            audio_path,
            std::time::Instant::now(),
            StructuredLlmLogFields {
                outcome: "fallback",
                model: summary_model.clone(),
                input_chars: transcript.len(),
                output_chars: 0,
                extra: serde_json::json!({ "count": 0 }),
            },
        );
    }

    // Clean up screen captures (runs regardless of summarization setting — fixes race)
    if !screen_files.is_empty()
        && !config.screen_context.keep_after_summary
        && std::fs::remove_dir_all(&screen_dir).is_ok()
    {
        tracing::info!(dir = %screen_dir.display(), "screen captures cleaned up");
    }

    // Step 4: Match calendar event + merge attendees
    on_progress(PipelineStage::Saving);

    if let Some(ref title) = calendar_event_title {
        tracing::info!(event = %title, attendees = calendar_attendees.len(), "matched calendar event");
    }

    let attendees = merge_attendees(&calendar_attendees, &summary_participants);

    if !attendees.is_empty() {
        tracing::info!(attendees = ?attendees, "merged attendee list");
    }

    // Step 4b: Speaker attribution
    // Level 2 → Level 0 → Level 1 (voice enrollment → deterministic → LLM)
    let attribution_start = std::time::Instant::now();
    let attribution = attribute_meeting_speakers(
        audio_path,
        content_type,
        sidecar.and_then(|metadata| metadata.source.as_deref()),
        config,
        &calendar_attendees,
        &attendees,
        diarization_num_speakers,
        diarization_from_stems,
        &diarization_embeddings,
        transcript,
    );
    let attribution_ms = attribution_start.elapsed().as_millis() as u64;
    let transcript = attribution.transcript;
    let speaker_map = attribution.speaker_map;
    let attendees = normalize_attendees_with_speaker_map(&attendees, &speaker_map);
    let structured_actions =
        normalize_action_items_with_speaker_map(structured_actions, &speaker_map);
    let structured_intents = normalize_intents_with_speaker_map(structured_intents, &speaker_map);
    let structured_decisions =
        normalize_decisions_with_speaker_map(structured_decisions, &speaker_map);

    // Step 5: Write markdown (always)
    let duration = estimate_duration(audio_path);
    let auto_title = title.map(String::from).unwrap_or_else(|| {
        if status == Some(OutputStatus::NoSpeech) {
            "Untitled Recording".into()
        } else {
            // Prefer calendar event title over transcript-derived title
            calendar_event_title
                .as_deref()
                .and_then(title_from_context)
                .map(finalize_title)
                .unwrap_or_else(|| generate_title(&transcript, pre_context.as_deref()))
        }
    });
    let entities_started = std::time::Instant::now();
    let entities = build_entity_links(
        &auto_title,
        pre_context.as_deref(),
        &attendees,
        &structured_actions,
        &structured_decisions,
        &structured_intents,
        &[],
        Some(&config.identity),
    );
    log_structured_llm_step(
        "entity_extract",
        audio_path,
        entities_started,
        StructuredLlmLogFields {
            outcome: if entities.people.is_empty() && entities.projects.is_empty() {
                "empty"
            } else if raw_summary.is_some() {
                "ok"
            } else {
                "fallback"
            },
            model: summary_model.clone(),
            input_chars: transcript.len(),
            output_chars: serialized_chars(&entities),
            extra: serde_json::json!({
                "people": entities.people.len(),
                "projects": entities.projects.len(),
            }),
        },
    );
    let people = entities
        .people
        .iter()
        .map(|entity| entity.label.clone())
        .collect();
    let title_generation = maybe_refine_title_with_llm(
        &auto_title,
        title,
        summary.as_deref(),
        raw_summary.as_ref(),
        &entities,
        config,
        summarize::refine_title,
    );

    // Determine source field: sidecar overrides default, normalize to "voice-memos" (plural)
    let source = if let Some(s) = sidecar.and_then(|s| s.source.clone()) {
        Some(s)
    } else {
        match content_type {
            ContentType::Memo => Some("voice-memos".into()),
            ContentType::Meeting => None,
            ContentType::Dictation => Some("dictation".into()),
        }
    };
    let tags = derive_structured_tags(
        content_type,
        source.as_deref(),
        sidecar.and_then(|s| s.device.as_deref()),
        &entities,
        &structured_decisions,
        &structured_intents,
    );

    let mut frontmatter = Frontmatter {
        title: auto_title,
        r#type: content_type,
        date: recording_date,
        duration,
        source,
        status,
        tags,
        attendees,
        attendees_raw: None,
        calendar_event: calendar_event_title,
        people,
        entities,
        device: sidecar.and_then(|s| s.device.clone()),
        captured_at: sidecar.and_then(|s| s.captured_at),
        context: pre_context,
        action_items: structured_actions,
        decisions: structured_decisions,
        intents: structured_intents,
        recorded_by: config.identity.name.clone(),
        visibility: None,
        speaker_map,
        filter_diagnosis: if status == Some(OutputStatus::NoSpeech) {
            Some(filter_stats.diagnosis())
        } else {
            None
        },
    };

    tracing::info!(step = "write", "writing markdown");
    let step_start = std::time::Instant::now();
    let mut result = markdown::write_with_retry_path(
        &frontmatter,
        &transcript,
        summary.as_deref(),
        user_notes.as_deref(),
        Some(audio_path),
        config,
    )?;
    apply_title_generation(
        audio_path,
        &mut result,
        &mut frontmatter,
        title_generation,
        |duration_ms, extra| {
            logging::log_step(
                "title_generation",
                &audio_path.display().to_string(),
                duration_ms,
                extra,
            );
        },
    );

    if frontmatter.r#type == ContentType::Meeting {
        log_attribution_decision(audio_path, &result.path, attribution_ms, &attribution.debug);
    }
    // Save per-speaker embeddings as sidecar (for Level 3 confirmed learning)
    if !diarization_embeddings.is_empty() {
        crate::voice::save_meeting_embeddings(&result.path, &diarization_embeddings);
    }

    if let Err(error) = crate::daily_notes::append_backlink(
        &result,
        frontmatter.date,
        summary.as_deref(),
        Some(&frontmatter),
        config,
    ) {
        tracing::warn!(
            error = %error,
            output = %result.path.display(),
            "failed to append daily note backlink"
        );
    }
    let write_ms = step_start.elapsed().as_millis() as u64;
    logging::log_step(
        "write",
        &audio_path.display().to_string(),
        write_ms,
        serde_json::json!({"output": result.path.display().to_string(), "words": result.word_count}),
    );

    // Emit structured insight events for agent subscription
    if let Some(ref summary_data) = raw_summary {
        crate::events::emit_insights_from_summary(
            summary_data,
            &result.path.display().to_string(),
            &result.title,
            &frontmatter.attendees,
        );
    }

    // Vault sync (non-fatal — pipeline succeeds regardless)
    match crate::vault::sync_file(&result.path, config) {
        Ok(Some(vault_path)) => {
            crate::events::append_event(crate::events::MinutesEvent::VaultSynced {
                source_path: result.path.display().to_string(),
                vault_path: vault_path.display().to_string(),
                strategy: config.vault.strategy.clone(),
            });
        }
        Ok(None) => {} // vault not enabled or no-op strategy
        Err(e) => {
            tracing::warn!(error = %e, output = %result.path.display(), "vault sync failed");
        }
    }

    // Emit event for agents/watchers
    crate::events::append_event(crate::events::audio_processed_event(
        &result,
        &audio_path.display().to_string(),
    ));

    let elapsed = start.elapsed();
    logging::log_step(
        "pipeline_complete",
        &audio_path.display().to_string(),
        elapsed.as_millis() as u64,
        serde_json::json!({"output": result.path.display().to_string(), "words": result.word_count, "content_type": format!("{:?}", content_type)}),
    );
    tracing::info!(
        file = %result.path.display(),
        words = result.word_count,
        elapsed_ms = elapsed.as_millis() as u64,
        "pipeline complete"
    );

    Ok(result)
}

/// Estimate audio duration from file size (rough approximation).
/// 16kHz mono 16-bit WAV ≈ 32KB/sec.
fn estimate_duration(audio_path: &Path) -> String {
    if let Some(duration_secs) = probe_audio_duration_secs(audio_path) {
        return format_duration_secs(duration_secs);
    }

    let bytes = std::fs::metadata(audio_path).map(|m| m.len()).unwrap_or(0);

    // WAV header is 44 bytes, then raw PCM at 32000 bytes/sec (16kHz 16-bit mono)
    let secs = if bytes > 44 { (bytes - 44) / 32_000 } else { 0 };

    format_duration_secs(secs as f64)
}

fn probe_audio_duration_secs(audio_path: &Path) -> Option<f64> {
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(audio_path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = audio_path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .ok()?;

    let track = probed.format.default_track()?;
    let params = &track.codec_params;
    let n_frames = params.n_frames?;
    let sample_rate = params.sample_rate?;
    if sample_rate == 0 {
        return None;
    }

    Some(n_frames as f64 / sample_rate as f64)
}

fn format_duration_secs(duration_secs: f64) -> String {
    let secs = duration_secs.round().max(0.0) as u64;
    let mins = secs / 60;
    let remaining_secs = secs % 60;
    if mins > 0 {
        format!("{}m {}s", mins, remaining_secs)
    } else {
        format!("{}s", remaining_secs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TitleGenerationDecision {
    final_title: String,
    refined_title: Option<String>,
    outcome: &'static str,
    model: Option<String>,
    input_chars: usize,
    detail: Option<String>,
    /// Wall-clock duration of the LLM refine call itself. Zero when the
    /// fallback paths (explicit title, missing summary) short-circuit before
    /// any LLM invocation.
    llm_duration_ms: u64,
}

fn maybe_refine_title_with_llm<F>(
    fallback_title: &str,
    explicit_title: Option<&str>,
    summary_text: Option<&str>,
    raw_summary: Option<&summarize::Summary>,
    entities: &markdown::EntityLinks,
    config: &Config,
    refine: F,
) -> TitleGenerationDecision
where
    F: FnOnce(
        &str,
        &summarize::Summary,
        &markdown::EntityLinks,
        &Config,
    ) -> Result<summarize::TitleRefinement, Box<dyn std::error::Error>>,
{
    if explicit_title.is_some() {
        return TitleGenerationDecision {
            final_title: fallback_title.to_string(),
            refined_title: None,
            outcome: "fallback",
            model: None,
            input_chars: 0,
            detail: Some("explicit-title".into()),
            llm_duration_ms: 0,
        };
    }

    let Some(summary_text) = summary_text.filter(|text| !text.trim().is_empty()) else {
        return TitleGenerationDecision {
            final_title: fallback_title.to_string(),
            refined_title: None,
            outcome: "fallback",
            model: None,
            input_chars: 0,
            detail: Some("missing-summary-text".into()),
            llm_duration_ms: 0,
        };
    };
    let Some(raw_summary) = raw_summary else {
        return TitleGenerationDecision {
            final_title: fallback_title.to_string(),
            refined_title: None,
            outcome: "fallback",
            model: None,
            input_chars: 0,
            detail: Some("missing-summary-struct".into()),
            llm_duration_ms: 0,
        };
    };

    let attempted_model = summarize::title_refinement_model(config);
    let input_chars = summarize::title_refinement_input_chars(summary_text, raw_summary, entities);

    let llm_started = std::time::Instant::now();
    let refine_result = refine(summary_text, raw_summary, entities, config);
    let llm_duration_ms = llm_started.elapsed().as_millis() as u64;

    match refine_result {
        Ok(refined) => {
            let cleaned = sanitize_llm_title_candidate(&refined.title);
            if llm_title_passes_quality(&cleaned) {
                TitleGenerationDecision {
                    final_title: cleaned.clone(),
                    refined_title: Some(cleaned),
                    outcome: "llm",
                    model: Some(refined.model),
                    input_chars: refined.input_chars,
                    detail: None,
                    llm_duration_ms,
                }
            } else {
                TitleGenerationDecision {
                    final_title: fallback_title.to_string(),
                    refined_title: None,
                    outcome: "fallback",
                    model: Some(refined.model),
                    input_chars: refined.input_chars,
                    detail: Some(format!("rejected-title: {}", cleaned)),
                    llm_duration_ms,
                }
            }
        }
        Err(error) => TitleGenerationDecision {
            final_title: fallback_title.to_string(),
            refined_title: None,
            outcome: "error",
            model: attempted_model,
            input_chars,
            detail: Some(error.to_string()),
            llm_duration_ms,
        },
    }
}

fn apply_title_generation(
    audio_path: &Path,
    result: &mut WriteResult,
    frontmatter: &mut Frontmatter,
    decision: TitleGenerationDecision,
    mut log_step: impl FnMut(u64, serde_json::Value),
) {
    let apply_start = std::time::Instant::now();
    let mut outcome = decision.outcome;
    let mut detail = decision.detail.clone();

    if let Some(refined_title) = decision.refined_title.as_ref() {
        if refined_title != &result.title {
            match markdown::rename_meeting(&result.path, refined_title) {
                Ok(new_path) => {
                    result.path = new_path;
                    result.title = refined_title.clone();
                    frontmatter.title = refined_title.clone();
                }
                Err(error) => {
                    outcome = "error";
                    detail = Some(error.to_string());
                    tracing::warn!(
                        error = %error,
                        output = %result.path.display(),
                        refined_title = %refined_title,
                        "failed to apply LLM-refined title"
                    );
                }
            }
        } else {
            frontmatter.title = refined_title.clone();
        }
    } else {
        frontmatter.title = decision.final_title.clone();
    }

    let apply_ms = apply_start.elapsed().as_millis() as u64;
    let mut extra = serde_json::json!({
        "outcome": outcome,
        "model": decision.model,
        "input_chars": decision.input_chars,
        "title": result.title,
        "apply_ms": apply_ms,
    });
    if let Some(detail) = detail {
        extra["detail"] = serde_json::json!(detail);
    }
    if result.path.as_os_str() != audio_path.as_os_str() {
        extra["output"] = serde_json::json!(result.path.display().to_string());
    }

    log_step(decision.llm_duration_ms, extra);
}

fn sanitize_llm_title_candidate(candidate: &str) -> String {
    let first_line = candidate
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let without_label = first_line
        .strip_prefix("Title:")
        .or_else(|| first_line.strip_prefix("title:"))
        .or_else(|| first_line.strip_prefix("Meeting title:"))
        .or_else(|| first_line.strip_prefix("meeting title:"))
        .unwrap_or(first_line)
        .trim();
    normalize_space(
        without_label.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '*' | '-' | ' ')),
    )
    .trim_matches(|c: char| matches!(c, '.' | ':' | ';'))
    .to_string()
}

fn llm_title_passes_quality(candidate: &str) -> bool {
    if candidate.is_empty() || candidate.chars().count() > 80 {
        return false;
    }

    let words: Vec<String> = candidate
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '&' && c != '×')
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect();

    if words.len() < 2 || words.len() > 12 {
        return false;
    }

    let normalized = words.join(" ");
    let generic_exact = [
        "call",
        "conversation",
        "meeting",
        "memo",
        "recording",
        "sync",
        "untitled",
        "untitled recording",
    ];
    if generic_exact.contains(&normalized.as_str()) {
        return false;
    }

    let generic_words = [
        "call",
        "chat",
        "conversation",
        "discussion",
        "meeting",
        "memo",
        "notes",
        "recording",
        "review",
        "sync",
        "title",
        "update",
    ];
    let stopwords = ["a", "an", "and", "for", "of", "on", "the", "to", "with"];

    !words
        .iter()
        .all(|word| generic_words.contains(&word.as_str()) || stopwords.contains(&word.as_str()))
}

/// Generate a smart title from either the user-provided context or transcript.
fn generate_title(transcript: &str, pre_context: Option<&str>) -> String {
    if let Some(context) = pre_context.and_then(title_from_context) {
        return finalize_title(context);
    }

    if let Some(transcript_title) = title_from_transcript(transcript) {
        return finalize_title(transcript_title);
    }

    "Untitled Recording".into()
}

fn title_from_context(context: &str) -> Option<String> {
    let cleaned = normalize_space(context);
    if cleaned.is_empty() {
        return None;
    }

    let lower = cleaned.to_lowercase();
    let generic = [
        "meeting",
        "recording",
        "memo",
        "voice memo",
        "call",
        "conversation",
        "note",
    ];
    if generic.contains(&lower.as_str()) {
        return None;
    }

    Some(to_display_title(&cleaned))
}

fn title_from_transcript(transcript: &str) -> Option<String> {
    let first_line = transcript.lines().find_map(clean_transcript_line)?;
    let conversationally_stripped = strip_conversational_prefixes(&first_line);
    let stripped = strip_lead_in_phrase(&conversationally_stripped);
    let candidate = normalize_space(&stripped);

    if candidate.is_empty() {
        return None;
    }

    if is_unusable_transcript_title(&candidate) {
        tracing::debug!(candidate = %candidate, "rejecting generic conversational title candidate");
        return None;
    }

    // Reject titles that are primarily non-Latin — a strong hallucination signal.
    // Whisper frequently hallucinates CJK/Arabic/Cyrillic text on low-signal audio.
    // We count Latin-script characters (including accented: é, ñ, ł, ü, etc.)
    // rather than raw ASCII to avoid rejecting valid European language titles.
    let alpha_chars: Vec<char> = candidate.chars().filter(|c| c.is_alphabetic()).collect();
    if !alpha_chars.is_empty() {
        let latin_count = alpha_chars
            .iter()
            .filter(|c| {
                c.is_ascii_alphabetic()
                    || ('\u{00C0}'..='\u{024F}').contains(c) // Latin-1 Supplement + Extended-A/B
                    || ('\u{1E00}'..='\u{1EFF}').contains(c) // Latin Extended Additional
            })
            .count();
        let latin_ratio = latin_count as f64 / alpha_chars.len() as f64;
        if latin_ratio < 0.5 {
            tracing::debug!(
                candidate = %candidate,
                latin_ratio = latin_ratio,
                "rejecting non-Latin title as likely hallucination"
            );
            return None;
        }
    }

    Some(to_display_title(&candidate))
}

pub(crate) fn clean_transcript_line(line: &str) -> Option<String> {
    let mut remaining = line.trim();

    while let Some(rest) = remaining.strip_prefix('[') {
        let bracket_end = rest.find(']')?;
        remaining = rest[bracket_end + 1..].trim();
    }

    let cleaned = normalize_space(remaining);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn strip_lead_in_phrase(line: &str) -> String {
    let cleaned = normalize_space(line);
    let lower = cleaned.to_lowercase();
    let prefixes = [
        "we need to discuss ",
        "let's talk about ",
        "lets talk about ",
        "let's discuss ",
        "lets discuss ",
        "i just had an idea about ",
        "i had an idea about ",
        "this is about ",
        "today we're talking about ",
        "today we are talking about ",
        "we're talking about ",
        "we are talking about ",
        "we should talk about ",
        "we should discuss ",
        "i want to talk about ",
        "i want to discuss ",
    ];

    for prefix in prefixes {
        if lower.starts_with(prefix) {
            return cleaned[prefix.len()..].trim().to_string();
        }
    }

    cleaned
}

pub(crate) fn normalize_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn infer_recording_date(
    recorded_at: Option<DateTime<Local>>,
    sidecar: Option<&SidecarMetadata>,
    metadata: &std::fs::Metadata,
) -> DateTime<Local> {
    recorded_at
        .or_else(|| sidecar.and_then(|s| s.captured_at))
        .or_else(|| metadata.created().ok().map(DateTime::<Local>::from))
        .unwrap_or_else(Local::now)
}

fn title_tokens(text: &str) -> BTreeSet<String> {
    const STOPWORDS: &[&str] = &[
        "a", "an", "and", "call", "for", "meeting", "prep", "session", "sync", "the", "to", "with",
    ];

    text.split(|ch: char| !ch.is_alphanumeric())
        .filter_map(|token| {
            let normalized = token.trim().to_lowercase();
            if normalized.len() < 3 || STOPWORDS.contains(&normalized.as_str()) {
                None
            } else {
                Some(normalized)
            }
        })
        .collect()
}

fn title_overlap(a: &str, b: &str) -> usize {
    let a_tokens = title_tokens(a);
    let b_tokens = title_tokens(b);
    a_tokens.intersection(&b_tokens).count()
}

fn select_calendar_event(
    events: &[crate::calendar::CalendarEvent],
    title_override: Option<&str>,
) -> Option<crate::calendar::CalendarEvent> {
    let explicit_title = title_override
        .map(str::trim)
        .filter(|title| !title.is_empty());

    events
        .iter()
        .filter(|event| {
            explicit_title
                .map(|title| title_overlap(title, &event.title) > 0)
                .unwrap_or(true)
        })
        .min_by_key(|event| event.minutes_until.abs())
        .cloned()
}

fn merge_attendees(existing: &[String], additions: &[String]) -> Vec<String> {
    let mut attendees = Vec::new();
    let mut seen_lower = std::collections::HashSet::new();

    for participant in existing.iter().chain(additions.iter()) {
        let Some(normalized) = normalize_attendee_candidate(participant) else {
            continue;
        };
        let lower = normalized.to_lowercase();
        if seen_lower.insert(lower) {
            attendees.push(normalized);
        }
    }
    attendees
}

fn split_decode_hint_fragments(text: &str) -> Vec<String> {
    text.replace(['—', '&', ',', '/'], "|")
        .split('|')
        .flat_map(|part| part.split(" with "))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
}

pub(crate) fn build_decode_hints(
    title: Option<&str>,
    calendar_event_title: Option<&str>,
    pre_context: Option<&str>,
    attendees: &[String],
    identity: Option<&IdentityConfig>,
) -> crate::transcribe::DecodeHints {
    let mut priority = Vec::new();
    let mut contextual = Vec::new();

    if let Some(identity) = identity.filter(|identity| user_is_participant(attendees, identity)) {
        if let Some(name) = identity
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            priority.push(name.to_string());
        }
        for alias in &identity.aliases {
            let normalized = strip_email_domain(strip_name_disambiguation(alias.trim())).trim();
            if !normalized.is_empty() {
                priority.push(normalized.to_string());
            }
        }
    }

    for attendee in attendees {
        if let Some(normalized) = normalize_attendee_candidate(attendee) {
            let canonical = strip_email_domain(strip_name_disambiguation(&normalized)).trim();
            if canonical.is_empty() {
                continue;
            }
            if canonical.contains('.') || canonical.contains('_') {
                let humanized = canonical
                    .split(['.', '_'])
                    .filter(|part| !part.is_empty())
                    .map(capitalize_token)
                    .collect::<Vec<_>>()
                    .join(" ");
                if !humanized.is_empty() {
                    priority.push(humanized);
                    continue;
                }
            }
            priority.push(canonical.to_string());
        }
    }

    for candidate in title
        .into_iter()
        .chain(calendar_event_title)
        .chain(pre_context)
    {
        contextual.extend(split_decode_hint_fragments(candidate));
    }

    crate::transcribe::DecodeHints::from_candidates(&priority, &contextual)
}

fn collect_user_participant_variants(
    attendees: &[String],
    identity: &IdentityConfig,
) -> Vec<String> {
    let Some(name) = identity
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Vec::new();
    };

    let canonical_slug = slugify(name);
    if canonical_slug.is_empty() {
        return Vec::new();
    }

    let canonical_lower = name.to_lowercase();
    let mut variants = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for alias in &identity.aliases {
        let normalized = strip_email_domain(strip_name_disambiguation(alias.trim())).trim();
        if normalized.is_empty() {
            continue;
        }
        let lower = normalized.to_lowercase();
        if lower != canonical_lower && seen.insert(lower) {
            variants.push(normalized.to_string());
        }
    }

    for attendee in attendees {
        let Some(normalized) = normalize_attendee_candidate(attendee) else {
            continue;
        };
        let canonical = strip_email_domain(strip_name_disambiguation(&normalized)).trim();
        if slugify(canonical) != canonical_slug {
            continue;
        }
        let lower = canonical.to_lowercase();
        if lower != canonical_lower && seen.insert(lower) {
            variants.push(canonical.to_string());
        }
    }

    variants
}

fn rewrite_intro_prefix_case_insensitive(
    body: &str,
    prefix: &str,
    variant: &str,
    replacement: &str,
) -> Option<String> {
    if !(body.is_ascii() && prefix.is_ascii() && variant.is_ascii() && replacement.is_ascii()) {
        return None;
    }

    let body_lower = body.to_ascii_lowercase();
    let prefix_lower = prefix.to_ascii_lowercase();
    let variant_lower = variant.to_ascii_lowercase();
    let target = format!("{prefix_lower}{variant_lower}");
    if !body_lower.starts_with(&target) {
        return None;
    }

    let remainder = &body[target.len()..];
    if remainder
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphanumeric())
    {
        return None;
    }

    Some(format!(
        "{}{}{}",
        &body[..prefix.len()],
        replacement,
        remainder
    ))
}

fn rewrite_exact_prefix_case_insensitive(
    body: &str,
    target: &str,
    replacement: &str,
) -> Option<String> {
    if !(body.is_ascii() && target.is_ascii() && replacement.is_ascii()) {
        return None;
    }

    let body_lower = body.to_ascii_lowercase();
    let target_lower = target.to_ascii_lowercase();
    if !body_lower.starts_with(&target_lower) {
        return None;
    }

    Some(format!("{}{}", replacement, &body[target.len()..]))
}

fn leading_name_token(text: &str) -> Option<&str> {
    let token = text.split_whitespace().next()?;
    let trimmed = token.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn levenshtein_distance_ascii(a: &str, b: &str) -> usize {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let mut dp = vec![vec![0usize; b_bytes.len() + 1]; a_bytes.len() + 1];
    for (i, row) in dp.iter_mut().enumerate().take(a_bytes.len() + 1) {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate().take(b_bytes.len() + 1) {
        *cell = j;
    }
    for i in 1..=a_bytes.len() {
        for j in 1..=b_bytes.len() {
            let cost = usize::from(a_bytes[i - 1] != b_bytes[j - 1]);
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }
    dp[a_bytes.len()][b_bytes.len()]
}

fn is_safe_self_name_fuzzy_match(token: &str, canonical: &str) -> bool {
    if !(token.is_ascii() && canonical.is_ascii()) {
        return false;
    }
    let token_lower = token.to_ascii_lowercase();
    let canonical_lower = canonical.to_ascii_lowercase();
    if token_lower == canonical_lower {
        return false;
    }
    if token_lower
        .chars()
        .next()
        .zip(canonical_lower.chars().next())
        .is_none_or(|(left, right)| left != right)
    {
        return false;
    }
    if !(token_lower.starts_with(&canonical_lower) || canonical_lower.starts_with(&token_lower)) {
        return false;
    }
    let distance = levenshtein_distance_ascii(&token_lower, &canonical_lower);
    distance <= 1
}

fn rewrite_intro_fuzzy_self_name(body: &str, prefix: &str, canonical: &str) -> Option<String> {
    if !(body.is_ascii() && prefix.is_ascii() && canonical.is_ascii()) {
        return None;
    }
    let body_lower = body.to_ascii_lowercase();
    let prefix_lower = prefix.to_ascii_lowercase();
    if !body_lower.starts_with(&prefix_lower) {
        return None;
    }

    let remainder = &body[prefix.len()..];
    let token = leading_name_token(remainder)?;
    if !is_safe_self_name_fuzzy_match(token, canonical) {
        return None;
    }
    let token_start = prefix.len();
    let token_end = token_start + token.len();
    Some(format!(
        "{}{}{}",
        &body[..token_start],
        canonical,
        &body[token_end..]
    ))
}

fn normalize_self_name_refs_in_transcript(
    transcript: &str,
    canonical: &str,
    variants: &[String],
) -> String {
    if canonical.trim().is_empty() {
        return transcript.to_string();
    }

    let intro_prefixes = [
        "this is ",
        "hey, this is ",
        "hey this is ",
        "okay, this is ",
        "ok, this is ",
        "all right, this is ",
        "alright, this is ",
    ];

    let mut out = Vec::new();
    for line in transcript.lines() {
        if let Some((head, body)) = line.split_once("] ") {
            let mut rewritten = None;
            for variant in variants {
                for prefix in intro_prefixes {
                    if let Some(new_body) =
                        rewrite_intro_prefix_case_insensitive(body, prefix, variant, canonical)
                    {
                        rewritten = Some(new_body);
                        break;
                    }
                }
                if rewritten.is_none() {
                    let pattern = format!("{variant} is ");
                    let replacement = format!("{canonical} is ");
                    if let Some(new_body) =
                        rewrite_exact_prefix_case_insensitive(body, &pattern, &replacement)
                    {
                        rewritten = Some(new_body);
                    }
                }
                if rewritten.is_some() {
                    break;
                }
            }
            if rewritten.is_none() {
                for prefix in intro_prefixes {
                    if let Some(new_body) = rewrite_intro_fuzzy_self_name(body, prefix, canonical) {
                        rewritten = Some(new_body);
                        break;
                    }
                }
            }
            if rewritten.is_none() {
                let lower = body.to_ascii_lowercase();
                if let Some(position) = lower.find(" is ") {
                    let token = body[..position].trim_matches(|c: char| !c.is_ascii_alphanumeric());
                    if is_safe_self_name_fuzzy_match(token, canonical) {
                        rewritten = Some(format!("{canonical}{}", &body[position..]));
                    }
                }
            }
            if let Some(new_body) = rewritten {
                out.push(format!("{head}] {new_body}"));
                continue;
            }
        }
        out.push(line.to_string());
    }

    if transcript.ends_with('\n') {
        format!("{}\n", out.join("\n"))
    } else {
        out.join("\n")
    }
}

pub(crate) fn normalize_transcript_for_self_name_participant(
    transcript: &str,
    attendees: &[String],
    identity: &IdentityConfig,
) -> String {
    if !user_is_participant(attendees, identity) {
        return transcript.to_string();
    }

    let Some(canonical) = identity
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return transcript.to_string();
    };

    let variants = collect_user_participant_variants(attendees, identity);
    normalize_self_name_refs_in_transcript(transcript, canonical, &variants)
}

fn user_is_participant(attendees: &[String], identity: &IdentityConfig) -> bool {
    let Some(name) = identity
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };

    let canonical_slug = slugify(name);
    if canonical_slug.is_empty() {
        return false;
    }

    let mut participant_slugs = std::collections::HashSet::new();
    participant_slugs.insert(canonical_slug.clone());
    for alias in identity.all_user_aliases() {
        let normalized = strip_email_domain(strip_name_disambiguation(alias.trim())).trim();
        let slug = slugify(normalized);
        if !slug.is_empty() {
            participant_slugs.insert(slug);
        }
    }

    attendees.iter().any(|attendee| {
        normalize_attendee_candidate(attendee).is_some_and(|normalized| {
            let canonical = strip_email_domain(strip_name_disambiguation(&normalized)).trim();
            let slug = slugify(canonical);
            !slug.is_empty() && participant_slugs.contains(&slug)
        })
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedSpeakerReference<'a> {
    label: String,
    name_hint: Option<&'a str>,
}

fn parse_speaker_reference(raw: &str) -> Option<ParsedSpeakerReference<'_>> {
    let trimmed = raw.trim().trim_start_matches('@').trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("speaker") {
        return None;
    }

    let suffix = &trimmed["speaker".len()..];
    let suffix = suffix.trim_start_matches(['_', ' ']);
    let digits_len = suffix.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits_len == 0 {
        return None;
    }

    let label = format!("SPEAKER_{}", &suffix[..digits_len]);
    let rest = suffix[digits_len..].trim();
    let name_hint = if let Some(name) = rest.strip_prefix('/') {
        let name = name.trim();
        (!name.is_empty()).then_some(name)
    } else if rest.starts_with('(') && rest.ends_with(')') {
        let name = rest.trim_start_matches('(').trim_end_matches(')').trim();
        (!name.is_empty()).then_some(name)
    } else {
        None
    };

    Some(ParsedSpeakerReference { label, name_hint })
}

fn normalize_attendee_candidate(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(reference) = parse_speaker_reference(trimmed) {
        return reference.name_hint.map(str::to_string);
    }

    Some(strip_name_disambiguation(trimmed).to_string())
}

fn resolve_speaker_reference(
    raw: &str,
    speaker_map: &[diarize::SpeakerAttribution],
    include_confidence_hint: bool,
) -> Option<String> {
    let reference = parse_speaker_reference(raw)?;
    let mapped = speaker_map
        .iter()
        .find(|attr| attr.speaker_label.eq_ignore_ascii_case(&reference.label));

    match mapped {
        Some(attr) if include_confidence_hint && attr.confidence != diarize::Confidence::High => {
            Some(format!("{} ({})", attr.name, attr.speaker_label))
        }
        Some(attr) => Some(attr.name.clone()),
        None => reference.name_hint.map(str::to_string),
    }
}

fn normalize_attendees_with_speaker_map(
    attendees: &[String],
    speaker_map: &[diarize::SpeakerAttribution],
) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for attendee in attendees {
        let cleaned = match resolve_speaker_reference(attendee, speaker_map, false)
            .or_else(|| normalize_attendee_candidate(attendee))
        {
            Some(cleaned) => cleaned,
            None if parse_speaker_reference(attendee).is_some() => continue,
            None => attendee.trim().to_string(),
        };
        if cleaned.is_empty() {
            continue;
        }

        let key = cleaned.to_lowercase();
        if seen.insert(key) {
            normalized.push(cleaned);
        }
    }

    normalized
}

fn normalize_action_items_with_speaker_map(
    action_items: Vec<markdown::ActionItem>,
    speaker_map: &[diarize::SpeakerAttribution],
) -> Vec<markdown::ActionItem> {
    action_items
        .into_iter()
        .map(|mut item| {
            if let Some(assignee) = resolve_speaker_reference(&item.assignee, speaker_map, true) {
                item.assignee = assignee;
            }
            item
        })
        .collect()
}

fn normalize_decisions_with_speaker_map(
    decisions: Vec<markdown::Decision>,
    speaker_map: &[diarize::SpeakerAttribution],
) -> Vec<markdown::Decision> {
    decisions
        .into_iter()
        .map(|mut decision| {
            if let Some(topic) = decision.topic.as_deref() {
                if let Some(resolved) = resolve_speaker_reference(topic, speaker_map, true) {
                    decision.topic = Some(resolved);
                }
            }
            decision
        })
        .collect()
}

fn normalize_intents_with_speaker_map(
    intents: Vec<markdown::Intent>,
    speaker_map: &[diarize::SpeakerAttribution],
) -> Vec<markdown::Intent> {
    intents
        .into_iter()
        .map(|mut intent| {
            intent.who = intent
                .who
                .as_deref()
                .and_then(|who| resolve_speaker_reference(who, speaker_map, true))
                .or(intent.who);
            intent
        })
        .collect()
}

fn strip_conversational_prefixes(line: &str) -> String {
    let mut remaining = line.trim();
    let fillers = ["okay", "ok", "so", "well", "alright", "all right"];

    loop {
        let lower = remaining.to_lowercase();
        let mut stripped = false;

        for filler in fillers {
            if let Some(rest) = lower.strip_prefix(filler) {
                if rest.is_empty() {
                    return String::new();
                }

                if rest.starts_with(|c: char| {
                    c == ',' || c == '.' || c == '!' || c == '?' || c.is_whitespace()
                }) {
                    remaining = remaining[filler.len()..]
                        .trim_start_matches(|c: char| {
                            c == ',' || c == '.' || c == '!' || c == '?' || c.is_whitespace()
                        })
                        .trim_start();
                    stripped = true;
                    break;
                }
            }
        }

        if !stripped {
            return remaining.to_string();
        }
    }
}

fn transcript_title_words(candidate: &str) -> Vec<String> {
    candidate
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-')
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn is_unusable_transcript_title(candidate: &str) -> bool {
    let words = transcript_title_words(candidate);
    if words.is_empty() {
        return true;
    }

    let lower = words.join(" ");
    let greetings = ["hey", "hi", "hello"];
    if greetings.contains(&words[0].as_str()) && words.len() <= 4 {
        return true;
    }

    let generic_prefixes = [
        "this is a meeting",
        "this is the meeting",
        "this is a call",
        "this is the call",
        "this is a recording",
        "this is the recording",
        "this is a test",
        "this is just a test",
    ];
    if generic_prefixes
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return true;
    }

    let generic_words = [
        "a",
        "all",
        "alright",
        "and",
        "be",
        "call",
        "doing",
        "for",
        "gonna",
        "going",
        "here",
        "is",
        "just",
        "meeting",
        "now",
        "ok",
        "okay",
        "recording",
        "right",
        "so",
        "test",
        "that",
        "the",
        "this",
        "uh",
        "um",
        "we",
        "we're",
        "well",
    ];
    let informative_words: Vec<&String> = words
        .iter()
        .filter(|word| !generic_words.contains(&word.as_str()))
        .collect();

    informative_words.is_empty()
}

fn to_display_title(text: &str) -> String {
    let trimmed = text
        .trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
        .split(['.', '!', '?', '\n'])
        .next()
        .unwrap_or("")
        .trim();

    let stopwords = [
        "a", "an", "and", "as", "at", "by", "for", "from", "in", "of", "on", "or", "the", "to",
        "with",
    ];

    trimmed
        .split_whitespace()
        .enumerate()
        .map(|(idx, word)| {
            let lower = word.to_lowercase();
            let is_edge = idx == 0;
            if word.chars().any(|c| c.is_ascii_digit())
                || word
                    .chars()
                    .all(|c| !c.is_ascii_lowercase() || !c.is_ascii_uppercase())
                    && word.chars().filter(|c| c.is_ascii_uppercase()).count() > 1
            {
                word.to_string()
            } else if !is_edge && stopwords.contains(&lower.as_str()) {
                lower
            } else {
                let mut chars = lower.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn finalize_title(title: String) -> String {
    if title.chars().count() > 60 {
        let truncated: String = title.chars().take(57).collect();
        format!("{}...", truncated)
    } else {
        title
    }
}

/// Extract structured action items from a Summary.
/// Parses lines like "- @user: Send pricing doc by Friday" into ActionItem structs.
fn extract_action_items(summary: &summarize::Summary) -> Vec<markdown::ActionItem> {
    summary
        .action_items
        .iter()
        .map(|item| {
            let (assignee, task) = if let Some(rest) = item.strip_prefix('@') {
                // "@user: Send pricing doc by Friday"
                if let Some(colon_pos) = rest.find(':') {
                    (
                        rest[..colon_pos].trim().to_string(),
                        rest[colon_pos + 1..].trim().to_string(),
                    )
                } else {
                    ("unassigned".to_string(), item.clone())
                }
            } else {
                ("unassigned".to_string(), item.clone())
            };

            // Try to extract due date from phrases like "by Friday", "(due March 21)"
            let due = extract_due_date(&task);

            markdown::ActionItem {
                assignee,
                task: task.trim_end_matches(')').trim().to_string(),
                due,
                status: "open".to_string(),
            }
        })
        .collect()
}

/// Extract structured decisions from a Summary.
fn extract_decisions(summary: &summarize::Summary) -> Vec<markdown::Decision> {
    summary
        .decisions
        .iter()
        .map(|text| {
            // Try to infer topic from the first few words
            let topic = infer_topic(text);
            markdown::Decision {
                text: text.clone(),
                topic,
                authority: None,
                supersedes: None,
            }
        })
        .collect()
}

fn parse_actor_prefix(text: &str) -> (Option<String>, String) {
    if let Some(rest) = text.strip_prefix('@') {
        if let Some(colon_pos) = rest.find(':') {
            let who = rest[..colon_pos].trim();
            let what = rest[colon_pos + 1..].trim();
            return ((!who.is_empty()).then(|| who.to_string()), what.to_string());
        }
    }
    (None, text.trim().to_string())
}

fn extract_intents(summary: &summarize::Summary) -> Vec<markdown::Intent> {
    let mut intents = Vec::new();

    for item in extract_action_items(summary) {
        intents.push(markdown::Intent {
            kind: markdown::IntentKind::ActionItem,
            what: item.task,
            who: (item.assignee != "unassigned").then_some(item.assignee),
            status: item.status,
            by_date: item.due,
        });
    }

    for decision in extract_decisions(summary) {
        intents.push(markdown::Intent {
            kind: markdown::IntentKind::Decision,
            what: decision.text,
            who: None,
            status: "decided".into(),
            by_date: None,
        });
    }

    for question in &summary.open_questions {
        let (who, what) = parse_actor_prefix(question);
        intents.push(markdown::Intent {
            kind: markdown::IntentKind::OpenQuestion,
            what,
            who,
            status: "open".into(),
            by_date: None,
        });
    }

    for commitment in &summary.commitments {
        let due = extract_due_date(commitment);
        let (who, what) = parse_actor_prefix(commitment);
        intents.push(markdown::Intent {
            kind: markdown::IntentKind::Commitment,
            what: what.trim_end_matches(')').trim().to_string(),
            who,
            status: "open".into(),
            by_date: due,
        });
    }

    intents
}

/// Try to extract a due date from action item text.
/// Matches patterns like "by Friday", "by March 21", "(due 2026-03-21)".
fn extract_due_date(text: &str) -> Option<String> {
    let lower = text.to_lowercase();

    // "by Friday", "by next week", "by March 21"
    if let Some(pos) = lower.find(" by ") {
        let after = &text[pos + 4..];
        let due: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
            .collect();
        let due = due.trim().to_string();
        if !due.is_empty() {
            return Some(due);
        }
    }

    // "(due March 21)"
    if let Some(pos) = lower.find("due ") {
        let after = &text[pos + 4..];
        let due: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
            .collect();
        let due = due.trim().to_string();
        if !due.is_empty() {
            return Some(due);
        }
    }

    None
}

/// Infer a topic from decision text by extracting the first noun phrase.
fn infer_topic(text: &str) -> Option<String> {
    // Simple heuristic: use the first 3-5 meaningful words as the topic
    let words: Vec<&str> = text
        .split_whitespace()
        .filter(|w| {
            let lower = w.to_lowercase();
            !matches!(
                lower.as_str(),
                "the"
                    | "a"
                    | "an"
                    | "to"
                    | "for"
                    | "of"
                    | "in"
                    | "on"
                    | "at"
                    | "is"
                    | "was"
                    | "will"
                    | "should"
                    | "we"
                    | "they"
                    | "it"
            )
        })
        .take(4)
        .collect();

    if words.is_empty() {
        return None;
    }

    let candidate = words.join(" ").to_lowercase();
    (!is_task_like_project_candidate(&candidate, Some(text))).then_some(candidate)
}

#[allow(clippy::too_many_arguments)]
fn build_entity_links(
    title: &str,
    pre_context: Option<&str>,
    attendees: &[String],
    action_items: &[markdown::ActionItem],
    decisions: &[markdown::Decision],
    intents: &[markdown::Intent],
    tags: &[String],
    identity: Option<&IdentityConfig>,
) -> markdown::EntityLinks {
    let mut people: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
    let mut projects: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();

    for attendee in attendees {
        add_person_entity(&mut people, attendee);
    }
    for item in action_items {
        add_person_entity(&mut people, &item.assignee);
    }
    for intent in intents {
        if let Some(who) = &intent.who {
            add_person_entity(&mut people, who);
        }
    }

    if let Some(identity) = identity {
        fold_user_identity(&mut people, identity);
    }

    for decision in decisions {
        if let Some(topic) = &decision.topic {
            add_project_entity(&mut projects, topic, Some(&decision.text));
        } else {
            add_project_entity(&mut projects, &decision.text, None);
        }
    }
    if let Some(context) = pre_context {
        add_project_entity(&mut projects, context, None);
    }
    add_project_entity(&mut projects, title, None);
    for tag in tags {
        add_project_entity(&mut projects, tag, None);
    }

    markdown::EntityLinks {
        people: people
            .into_iter()
            .map(|(slug, (label, aliases))| markdown::EntityRef {
                slug,
                label,
                aliases: aliases.into_iter().collect(),
            })
            .collect(),
        projects: projects
            .into_iter()
            .map(|(slug, (label, aliases))| markdown::EntityRef {
                slug,
                label,
                aliases: aliases.into_iter().collect(),
            })
            .collect(),
    }
}

/// Fold any person entity that matches a configured user email or alias
/// onto the canonical user entity (keyed by `slugify(identity.name)`).
///
/// Covers the common case of one human appearing under several labels
/// in a single meeting — e.g. recorded by "Mat", attending as
/// "mathieu@work.com" on one calendar and "mat@personal.com" on
/// another, mentioned in transcript as "Mathieu". Without this fold,
/// each surface spawns its own entity and both the markdown frontmatter
/// and `graph.db` end up with duplicate Person rows that compound on
/// every rerun. Non-user entities are unaffected.
fn fold_user_identity(
    people: &mut BTreeMap<String, (String, BTreeSet<String>)>,
    identity: &IdentityConfig,
) {
    let Some(name) = identity
        .name
        .as_ref()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
    else {
        return;
    };
    let canonical_slug = slugify(name);
    if canonical_slug.is_empty() {
        return;
    }
    // Only fold when the user is actually a participant in this meeting.
    // If the canonical entry doesn't exist, don't invent it — the meeting
    // may genuinely not include the user (a recorded third-party call,
    // say).
    if !people.contains_key(&canonical_slug) {
        return;
    }

    let alias_slugs: Vec<String> = identity
        .all_user_aliases()
        .into_iter()
        .filter_map(|alias| {
            let canonical = strip_email_domain(strip_name_disambiguation(alias.trim())).trim();
            if canonical.is_empty() {
                return None;
            }
            let label: String = canonical
                .split_whitespace()
                .map(capitalize_token)
                .collect::<Vec<_>>()
                .join(" ");
            let slug = slugify(&label);
            if slug.is_empty() || slug == canonical_slug {
                None
            } else {
                Some(slug)
            }
        })
        .collect();

    for slug in alias_slugs {
        if let Some((label, aliases)) = people.remove(&slug) {
            let canonical_entry = people
                .get_mut(&canonical_slug)
                .expect("canonical slug was verified to exist above");
            canonical_entry.1.insert(label.to_ascii_lowercase());
            canonical_entry.1.extend(aliases);
        }
    }
}

fn derive_structured_tags(
    content_type: ContentType,
    source: Option<&str>,
    device: Option<&str>,
    entities: &markdown::EntityLinks,
    decisions: &[markdown::Decision],
    intents: &[markdown::Intent],
) -> Vec<String> {
    let mut tags = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push_tag = |tag: String| {
        if seen.insert(tag.clone()) {
            tags.push(tag);
        }
    };

    if content_type == ContentType::Memo {
        push_tag("memo".to_string());

        if let Some(source) = source.filter(|value| !value.trim().is_empty()) {
            push_tag(format!(
                "source:{}",
                normalize_entity_topic(source).replace(' ', "-")
            ));
        }

        if let Some(device) = device.filter(|value| !value.trim().is_empty()) {
            let normalized = normalize_entity_topic(device).replace(' ', "-");
            if !normalized.is_empty() {
                push_tag(format!("device:{normalized}"));
            }
        }

        if intents.iter().any(|intent| {
            matches!(
                intent.kind,
                markdown::IntentKind::Commitment | markdown::IntentKind::ActionItem
            )
        }) {
            push_tag("has-actions".into());
        }
        if !decisions.is_empty() {
            push_tag("has-decisions".into());
        }

        for entity in entities.people.iter().take(3) {
            push_tag(format!("person:{}", entity.slug));
        }

        for decision in decisions.iter().take(3) {
            if let Some(topic) = decision
                .topic
                .as_ref()
                .filter(|value| !value.trim().is_empty())
            {
                push_tag(format!("topic:{}", slugify(topic)));
            }
        }

        for entity in entities.projects.iter().take(4) {
            push_tag(format!("project:{}", entity.slug));
        }
    }

    tags.into_iter().take(8).collect()
}

fn add_person_entity(entities: &mut BTreeMap<String, (String, BTreeSet<String>)>, raw: &str) {
    let Some(trimmed) = (match resolve_speaker_reference(raw, &[], false) {
        Some(name) => Some(name),
        None => {
            let trimmed = raw.trim().trim_start_matches('@').trim();
            if parse_speaker_reference(trimmed).is_some() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
    }) else {
        return;
    };
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unassigned") {
        return;
    }

    let canonical = strip_email_domain(strip_name_disambiguation(&trimmed)).trim();
    if canonical.is_empty() {
        return;
    }

    let label = canonical
        .split_whitespace()
        .map(capitalize_token)
        .collect::<Vec<_>>()
        .join(" ");
    let slug = slugify(&label);
    if slug.is_empty() {
        return;
    }

    let entry = entities
        .entry(slug)
        .or_insert_with(|| (label.clone(), BTreeSet::new()));
    entry.1.insert(canonical.to_lowercase());
    if trimmed != canonical {
        entry.1.insert(trimmed.to_lowercase());
    }
    let raw_trimmed = raw.trim();
    if raw_trimmed != trimmed && raw_trimmed != canonical {
        entry.1.insert(raw_trimmed.to_lowercase());
    }
}

/// Strip a " / Other" disambiguation suffix, returning the canonical head.
/// Example: `"Mat / Matthew"` → `"Mat"`. If no separator is present, returns
/// the input unchanged. The LLM sometimes produces hedged names during
/// speaker attribution; the head is always the best guess.
fn strip_name_disambiguation(s: &str) -> &str {
    match s.split_once(" / ") {
        Some((head, _)) => head.trim_end(),
        None => s,
    }
}

/// If the string is an email address (`local@domain.tld`), return just the
/// local part. Otherwise return the input unchanged. This prevents email
/// forms from spawning separate person entities when the same human also
/// appears by display name elsewhere in the meeting.
fn strip_email_domain(s: &str) -> &str {
    if let Some((local, domain)) = s.split_once('@') {
        if !local.is_empty() && domain.contains('.') {
            return local;
        }
    }
    s
}

fn add_project_entity(
    entities: &mut BTreeMap<String, (String, BTreeSet<String>)>,
    raw: &str,
    alias_source: Option<&str>,
) {
    let normalized = normalize_entity_topic(raw);
    if normalized.is_empty() {
        return;
    }

    if is_task_like_project_candidate(&normalized, alias_source.or(Some(raw))) {
        return;
    }

    let generic = [
        "untitled recording",
        "follow up",
        "another follow up",
        "voice memo",
        "meeting",
        "recording",
    ];
    if generic.contains(&normalized.as_str()) {
        return;
    }

    let label = normalized
        .split_whitespace()
        .map(capitalize_token)
        .collect::<Vec<_>>()
        .join(" ");
    let slug = slugify(&label);
    if slug.is_empty() {
        return;
    }

    let entry = entities
        .entry(slug)
        .or_insert_with(|| (label.clone(), BTreeSet::new()));
    entry.1.insert(normalized.clone());
    if let Some(alias) = alias_source {
        let cleaned = normalize_space(alias);
        if !cleaned.is_empty() {
            entry.1.insert(cleaned.to_lowercase());
        }
    }
}

fn capitalize_token(token: &str) -> String {
    let lower = token.to_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn slugify(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn normalize_entity_topic(text: &str) -> String {
    let stopwords = [
        "a", "an", "and", "as", "at", "by", "for", "from", "in", "of", "on", "or", "the", "to",
        "with", "we", "should", "will", "be", "is", "are", "use", "using",
    ];

    text.split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .filter(|word| !stopwords.contains(&word.to_lowercase().as_str()))
        .take(4)
        .map(|word| word.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_task_like_project_candidate(normalized: &str, source: Option<&str>) -> bool {
    const ACTION_VERBS: &[&str] = &[
        "add", "ask", "asked", "build", "call", "check", "confirm", "create", "deliver", "email",
        "follow", "provide", "reach", "review", "run", "schedule", "send", "share", "study",
        "update",
    ];
    const TASK_START_RED_FLAGS: &[&str] = &[
        "a", "an", "the", "to", "my", "our", "your", "his", "her", "their", "this", "that",
        "these", "those", "me", "us", "him", "them",
    ];

    if parse_speaker_reference(normalized).is_some() {
        return true;
    }

    let words: Vec<&str> = normalized.split_whitespace().collect();
    if words.is_empty() {
        return true;
    }

    let verb_hits = words
        .iter()
        .filter(|word| ACTION_VERBS.contains(word))
        .count();

    if normalized.contains("reach out") || normalized.contains("follow up") {
        return true;
    }

    let source_words: Vec<String> = source
        .unwrap_or(normalized)
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect();

    let starts_with_action = ACTION_VERBS.contains(&words[0]);
    if starts_with_action {
        if words.len() == 1 {
            return true;
        }

        let has_follow_on_signal = source_words.len() > words.len()
            || source_words
                .get(1)
                .is_some_and(|word| TASK_START_RED_FLAGS.contains(&word.as_str()));

        if has_follow_on_signal || verb_hits >= 2 {
            return true;
        }
    }

    verb_hits >= 2
}

/// Execute the post_record hook if configured.
/// Runs the command asynchronously in the background with the transcript path as argument.
pub fn run_post_record_hook(config: &Config, transcript_path: &Path) {
    if let Some(ref command) = config.hooks.post_record {
        let cmd = command.clone();
        let path = transcript_path.display().to_string();
        std::thread::spawn(move || {
            tracing::info!(command = %cmd, path = %path, "running post_record hook");
            match std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("{} \"$1\"", cmd))
                .arg("--")
                .arg(&path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
            {
                Ok(output) => {
                    if !output.status.success() {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        tracing::warn!(
                            command = %cmd,
                            exit_code = output.status.code(),
                            stderr = %stderr,
                            "post_record hook failed"
                        );
                    } else {
                        tracing::info!(command = %cmd, "post_record hook completed");
                    }
                }
                Err(error) => {
                    tracing::warn!(command = %cmd, error = %error, "post_record hook spawn failed");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_summary() -> summarize::Summary {
        summarize::Summary {
            text: "Discussed Command RX codebase walkthrough and next steps.".into(),
            key_points: vec![
                "Walked through the Command RX codebase".into(),
                "Aligned on next implementation tasks".into(),
            ],
            decisions: vec!["Use the new ingestion pipeline".into()],
            action_items: vec!["@mat: Send follow-up notes by Friday".into()],
            open_questions: vec!["@samantha: Which rollout order should we use?".into()],
            commitments: vec!["@samantha: Share the access details".into()],
            participants: vec!["Mat".into(), "Samantha".into()],
        }
    }

    fn write_test_meeting(title: &str) -> (tempfile::TempDir, WriteResult, Frontmatter) {
        let dir = tempfile::TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let frontmatter = Frontmatter {
            title: title.into(),
            r#type: ContentType::Meeting,
            date: Local::now(),
            duration: "12m 0s".into(),
            source: None,
            status: Some(OutputStatus::Complete),
            tags: vec![],
            attendees: vec!["Samantha".into()],
            attendees_raw: None,
            calendar_event: None,
            people: vec![],
            entities: markdown::EntityLinks::default(),
            device: None,
            captured_at: None,
            context: None,
            action_items: vec![],
            decisions: vec![],
            intents: vec![],
            recorded_by: Some("Mat".into()),
            visibility: None,
            speaker_map: vec![],
            filter_diagnosis: None,
        };

        let result = markdown::write_with_retry_path(
            &frontmatter,
            "Transcript body",
            Some("Summary body"),
            None,
            None,
            &config,
        )
        .unwrap();

        (dir, result, frontmatter)
    }

    #[test]
    fn generate_title_takes_first_words() {
        let transcript = "We need to discuss the new pricing strategy for Q2";
        let title = generate_title(transcript, None);
        assert_eq!(title, "The New Pricing Strategy for Q2");
    }

    #[test]
    fn generate_title_strips_timestamps_and_speaker_labels() {
        let transcript = "[SPEAKER_0 0:00] let's talk about API launch timeline for Q2";
        let title = generate_title(transcript, None);
        assert_eq!(title, "API Launch Timeline for Q2");
    }

    #[test]
    fn generate_title_strips_conversational_fillers_before_lead_in_phrase() {
        let transcript = "Okay, let's talk about API launch timeline for Q2";
        let title = generate_title(transcript, None);
        assert_eq!(title, "API Launch Timeline for Q2");
    }

    #[test]
    fn generate_title_prefers_context_when_available() {
        let transcript = "Okay so I just had an idea about onboarding";
        let title = generate_title(transcript, Some("Q2 pricing discussion with Alex"));
        assert_eq!(title, "Q2 Pricing Discussion with Alex");
    }

    #[test]
    fn generate_title_falls_back_when_only_timestamps_exist() {
        let transcript = "[0:00]";
        let title = generate_title(transcript, None);
        assert_eq!(title, "Untitled Recording");
    }

    #[test]
    fn generate_title_rejects_greeting_only_openers() {
        let transcript = "[UNKNOWN 0:08] >> Hey, Matt.";
        let title = generate_title(transcript, None);
        assert_eq!(title, "Untitled Recording");
    }

    #[test]
    fn generate_title_rejects_generic_meeting_openers() {
        let transcript = "Okay, this is a meeting that we're gonna be doing here";
        let title = generate_title(transcript, None);
        assert_eq!(title, "Untitled Recording");
    }

    #[test]
    fn llm_title_refinement_success_renames_written_meeting() {
        let (_dir, mut result, mut frontmatter) = write_test_meeting("Untitled Recording");
        let audio_path = Path::new("/tmp/input.wav");
        let summary = sample_summary();
        let decision = maybe_refine_title_with_llm(
            "Untitled Recording",
            None,
            Some("Command RX walkthrough with implementation planning."),
            Some(&summary),
            &markdown::EntityLinks::default(),
            &Config::default(),
            |_, _, _, _| {
                Ok(summarize::TitleRefinement {
                    title: "Command RX Codebase Walkthrough".into(),
                    model: "agent:codex".into(),
                    input_chars: 128,
                })
            },
        );

        apply_title_generation(
            audio_path,
            &mut result,
            &mut frontmatter,
            decision,
            |_, _| {},
        );

        assert_eq!(result.title, "Command RX Codebase Walkthrough");
        assert_eq!(frontmatter.title, "Command RX Codebase Walkthrough");
        assert!(result.path.exists());
        assert!(result
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .contains("command-rx-codebase-walkthrough"));
        let content = std::fs::read_to_string(&result.path).unwrap();
        assert!(content.contains("title: \"Command RX Codebase Walkthrough\""));
    }

    #[test]
    fn llm_title_refinement_failure_falls_back_to_algorithmic_title() {
        let summary = sample_summary();
        let mut config = Config::default();
        config.summarization.engine = "agent".into();
        config.summarization.agent_command = "claude".into();
        let decision = maybe_refine_title_with_llm(
            "Roadmap Review",
            None,
            Some("Roadmap discussion"),
            Some(&summary),
            &markdown::EntityLinks::default(),
            &config,
            |_, _, _, _| Err("rate limited".into()),
        );

        assert_eq!(decision.final_title, "Roadmap Review");
        assert_eq!(decision.refined_title, None);
        assert_eq!(decision.outcome, "error");
        assert_eq!(decision.model, Some("agent:claude".into()));
    }

    #[test]
    fn llm_title_quality_filter_rejects_bad_titles() {
        let summary = sample_summary();
        let mut config = Config::default();
        config.summarization.engine = "agent".into();
        config.summarization.agent_command = "claude".into();
        let decision = maybe_refine_title_with_llm(
            "Roadmap Review",
            None,
            Some("Roadmap discussion"),
            Some(&summary),
            &markdown::EntityLinks::default(),
            &config,
            |_, _, _, _| {
                Ok(summarize::TitleRefinement {
                    title: "Meeting".into(),
                    model: "agent:codex".into(),
                    input_chars: 64,
                })
            },
        );

        assert_eq!(decision.final_title, "Roadmap Review");
        assert_eq!(decision.refined_title, None);
        assert_eq!(decision.outcome, "fallback");
    }

    #[test]
    fn algorithmic_fallback_still_works_standalone() {
        let title = generate_title("Hello.", None);
        assert_eq!(title, "Untitled Recording");
    }

    #[test]
    fn estimate_duration_formats_correctly() {
        // 32000 bytes/sec * 90 sec + 44 header = 2_880_044 bytes
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.wav");
        let data = vec![0u8; 2_880_044];
        std::fs::write(&path, &data).unwrap();

        let duration = estimate_duration(&path);
        assert_eq!(duration, "1m 30s");
    }

    #[test]
    fn format_duration_secs_rounds_to_nearest_second() {
        assert_eq!(format_duration_secs(4313.6), "71m 54s");
        assert_eq!(format_duration_secs(59.6), "1m 0s");
        assert_eq!(format_duration_secs(0.4), "0s");
    }

    #[test]
    fn extract_action_items_parses_assignee_and_task() {
        let summary = summarize::Summary {
            text: String::new(),
            key_points: vec![],
            decisions: vec![],
            action_items: vec![
                "@user: Send pricing doc by Friday".into(),
                "@sarah: Review competitor grid (due March 21)".into(),
                "Unassigned task with no @".into(),
            ],
            open_questions: vec![],
            commitments: vec![],
            participants: vec![],
        };

        let items = extract_action_items(&summary);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].assignee, "user");
        assert!(items[0].task.contains("Send pricing doc"));
        assert_eq!(items[0].due, Some("Friday".into()));
        assert_eq!(items[0].status, "open");

        assert_eq!(items[1].assignee, "sarah");
        assert_eq!(items[1].due, Some("March 21".into()));

        assert_eq!(items[2].assignee, "unassigned");
    }

    #[test]
    fn extract_decisions_with_topic_inference() {
        let summary = summarize::Summary {
            text: String::new(),
            key_points: vec![],
            decisions: vec![
                "Price advisor platform at monthly billing/mo".into(),
                "Use REST over GraphQL for the new API".into(),
            ],
            action_items: vec![],
            open_questions: vec![],
            commitments: vec![],
            participants: vec![],
        };

        let decisions = extract_decisions(&summary);
        assert_eq!(decisions.len(), 2);
        assert!(decisions[0].topic.is_some());
        assert!(decisions[0].text.contains("monthly billing"));
    }

    #[test]
    fn extract_due_date_patterns() {
        assert_eq!(
            extract_due_date("Send doc by Friday"),
            Some("Friday".into())
        );
        assert_eq!(
            extract_due_date("Review (due March 21)"),
            Some("March 21".into())
        );
        assert_eq!(extract_due_date("Just do this thing"), None);
    }

    #[test]
    fn single_stem_speaker_self_attribution_maps_to_identity() {
        let mut config = Config::default();
        config.identity.name = Some("Mat".into());

        let voice_result = VoiceMatchResult {
            attributions: vec![],
            self_profile_exists: true,
        };
        let labels = vec!["SPEAKER_0".to_string()];
        let l2_labels = std::collections::HashSet::new();

        let outcome = single_stem_speaker_self_attribution(
            Path::new("/fake.wav"),
            &config,
            &voice_result,
            true,
            "[SPEAKER_0 0:00] hello\n",
            &labels,
            &l2_labels,
        );
        let attr = outcome
            .attribution
            .expect("single stem speaker should map to self");

        assert_eq!(attr.name, "Mat");
        assert_eq!(attr.speaker_label, "SPEAKER_0");
        assert_eq!(attr.confidence, diarize::Confidence::Medium);
        assert_eq!(attr.source, diarize::AttributionSource::Deterministic);
        assert_eq!(
            outcome.debug.applied_via,
            Some(SelfAttributionAppliedVia::FallbackIdentityOnly)
        );
        assert_eq!(
            outcome.debug.fallback_reason,
            Some(SelfAttributionSkippedReason::NoStems)
        );
    }

    #[test]
    fn single_stem_speaker_self_attribution_respects_guards() {
        let mut config = Config::default();
        config.identity.name = Some("Mat".into());
        let voice_result = VoiceMatchResult {
            attributions: vec![],
            self_profile_exists: false,
        };
        let labels = vec!["SPEAKER_2".to_string()];
        let l2_labels = std::collections::HashSet::new();

        let no_stable_label = single_stem_speaker_self_attribution(
            Path::new("/fake.wav"),
            &config,
            &voice_result,
            true,
            "[SPEAKER_2 0:00] hello\n",
            &labels,
            &l2_labels,
        );
        assert!(!no_stable_label.debug.returned_some);
        assert_eq!(
            no_stable_label.debug.skipped_reason,
            Some(SelfAttributionSkippedReason::NoStableLabel)
        );

        let not_from_stems = single_stem_speaker_self_attribution(
            Path::new("/fake.wav"),
            &config,
            &voice_result,
            false,
            "[SPEAKER_0 0:00] hello\n",
            &["SPEAKER_0".to_string()],
            &std::collections::HashSet::new(),
        );
        assert!(!not_from_stems.debug.returned_some);
        assert_eq!(
            not_from_stems.debug.skipped_reason,
            Some(SelfAttributionSkippedReason::DiarizationNotFromStems)
        );
        let mut mapped = std::collections::HashSet::new();
        mapped.insert("SPEAKER_0".to_string());
        let already_mapped = single_stem_speaker_self_attribution(
            Path::new("/fake.wav"),
            &config,
            &voice_result,
            true,
            "[SPEAKER_0 0:00] hello\n",
            &["SPEAKER_0".to_string()],
            &mapped,
        );
        assert!(!already_mapped.debug.returned_some);
        assert_eq!(
            already_mapped.debug.skipped_reason,
            Some(SelfAttributionSkippedReason::AlreadyMapped)
        );
    }

    #[test]
    fn single_stem_speaker_self_attribution_handles_unknown_label() {
        let mut config = Config::default();
        config.identity.name = Some("Mat".into());
        let voice_result = VoiceMatchResult {
            attributions: vec![],
            self_profile_exists: true,
        };

        let outcome = single_stem_speaker_self_attribution(
            Path::new("/fake.wav"),
            &config,
            &voice_result,
            true,
            "[UNKNOWN 0:00] Hello there\n",
            &[],
            &std::collections::HashSet::new(),
        );
        let attr = outcome
            .attribution
            .expect("single unknown label should still map to self");

        assert_eq!(attr.speaker_label, "UNKNOWN");
        assert_eq!(attr.name, "Mat");
        assert_eq!(attr.confidence, diarize::Confidence::Medium);
        assert_eq!(
            outcome.debug.applied_via,
            Some(SelfAttributionAppliedVia::FallbackIdentityOnly)
        );
    }

    #[test]
    fn single_stem_self_attribution_skips_remote_only_label_without_voice_match() {
        let mut config = Config::default();
        config.identity.name = Some("Mat".into());
        let voice_result = VoiceMatchResult {
            attributions: vec![],
            self_profile_exists: true,
        };

        let outcome = single_stem_speaker_self_attribution(
            Path::new("/fake.wav"),
            &config,
            &voice_result,
            true,
            "[SPEAKER_1 0:00] remote voice\n",
            &["SPEAKER_1".to_string()],
            &std::collections::HashSet::new(),
        );

        assert!(outcome.attribution.is_none());
        assert_eq!(
            outcome.debug.skipped_reason,
            Some(SelfAttributionSkippedReason::RemoteOnlyLabel)
        );
    }

    #[test]
    fn infer_capture_backend_prefers_native_call_path() {
        assert_eq!(
            infer_capture_backend(
                Path::new("/Users/test/.minutes/native-captures/2026-04-08-083713-call.mov"),
                None
            ),
            "native-call"
        );
        assert_eq!(
            infer_capture_backend(Path::new("/Users/test/.minutes/jobs/job-123.wav"), None),
            "cpal"
        );
    }

    #[test]
    fn extract_effective_transcript_speaker_labels_keeps_unknowns() {
        let labels = extract_effective_transcript_speaker_labels(
            "[UNKNOWN 0:00] hello\n[SPEAKER_0 0:02] hi\n[Mat 0:05] done\n",
        );
        assert_eq!(labels, vec!["UNKNOWN", "SPEAKER_0", "Mat"]);
    }

    #[test]
    fn deterministic_two_person_mapping_stays_medium_confidence() {
        let mut config = Config::default();
        config.identity.name = Some("Mat".into());

        let result = attribute_meeting_speakers(
            Path::new("/fake.wav"),
            ContentType::Meeting,
            None,
            &config,
            &["Mat".into(), "Alex".into()],
            &["Mat".into(), "Alex".into()],
            2,
            true,
            &std::collections::HashMap::new(),
            "[SPEAKER_0 0:00] hello\n[SPEAKER_1 0:01] hi\n".into(),
        );

        assert_eq!(result.speaker_map.len(), 2);
        assert!(result
            .speaker_map
            .iter()
            .all(|entry| entry.confidence == diarize::Confidence::Medium));
    }

    #[test]
    fn merge_attendees_adds_summary_participants_case_insensitively() {
        let merged = merge_attendees(
            &["Mat".into(), "Alex".into()],
            &["alex".into(), "Casey".into()],
        );
        assert_eq!(merged, vec!["Mat", "Alex", "Casey"]);
    }

    #[test]
    fn merge_attendees_collapses_compound_speaker_labels_to_names() {
        let merged = merge_attendees(
            &["Andrea".into(), "Dan".into()],
            &[
                "Speaker 1 / Samantha".into(),
                "Speaker_2 (Mat)".into(),
                "Samantha".into(),
            ],
        );
        assert_eq!(merged, vec!["Andrea", "Dan", "Samantha", "Mat"]);
    }

    #[test]
    fn select_calendar_event_prefers_closest_candidate() {
        let selected = select_calendar_event(
            &[
                crate::calendar::CalendarEvent {
                    title: "Far Event".into(),
                    start: "2026-04-14 09:00".into(),
                    minutes_until: 45,
                    attendees: vec![],
                    url: None,
                },
                crate::calendar::CalendarEvent {
                    title: "Closest Event".into(),
                    start: "2026-04-14 10:00".into(),
                    minutes_until: 5,
                    attendees: vec![],
                    url: None,
                },
            ],
            None,
        )
        .expect("expected a match");

        assert_eq!(selected.title, "Closest Event");
    }

    #[test]
    fn select_calendar_event_requires_overlap_with_explicit_title() {
        let selected = select_calendar_event(
            &[crate::calendar::CalendarEvent {
                title: "Mat & Supernal Coding Meeting".into(),
                start: "2026-04-14 12:30".into(),
                minutes_until: 4,
                attendees: vec!["mat@example.com".into()],
                url: None,
            }],
            Some("Wesley prep session recovery"),
        );

        assert!(selected.is_none());
    }

    #[test]
    fn select_calendar_event_allows_explicit_title_when_names_overlap() {
        let selected = select_calendar_event(
            &[crate::calendar::CalendarEvent {
                title: "Wesley Young Prep Session".into(),
                start: "2026-04-14 12:00".into(),
                minutes_until: 2,
                attendees: vec!["wesley@example.com".into()],
                url: None,
            }],
            Some("Wesley prep session recovery"),
        )
        .expect("expected overlapping explicit title to keep match");

        assert_eq!(selected.title, "Wesley Young Prep Session");
    }

    #[test]
    fn native_call_without_trusted_attendees_maps_only_local_voice_source() {
        let mut config = Config::default();
        config.identity.name = Some("Mat".into());

        let result = attribute_meeting_speakers(
            Path::new("/Users/test/.minutes/native-captures/fake-call.mov"),
            ContentType::Meeting,
            Some("native-call"),
            &config,
            &[],
            &[],
            2,
            true,
            &std::collections::HashMap::new(),
            "[SPEAKER_1 0:00] hi\n[SPEAKER_0 0:01] hello\n".into(),
        );

        assert_eq!(result.speaker_map.len(), 1);
        assert!(result
            .speaker_map
            .iter()
            .any(|entry| entry.speaker_label == "SPEAKER_0"
                && entry.name == "Mat"
                && entry.confidence == diarize::Confidence::Medium));
        assert!(
            result
                .speaker_map
                .iter()
                .all(|entry| entry.speaker_label != "SPEAKER_1"),
            "native-call clips without trusted attendees should not invent a remote identity"
        );
    }

    #[test]
    fn extract_intents_builds_typed_entries() {
        let summary = summarize::Summary {
            text: String::new(),
            key_points: vec![],
            decisions: vec!["Use REST over GraphQL for the new API".into()],
            action_items: vec!["@user: Send pricing doc by Friday".into()],
            open_questions: vec!["@case: Do we grandfather current customers?".into()],
            commitments: vec!["@sarah: Share revised pricing model by Tuesday".into()],
            participants: vec![],
        };

        let intents = extract_intents(&summary);
        assert_eq!(intents.len(), 4);
        assert_eq!(intents[0].kind, markdown::IntentKind::ActionItem);
        assert_eq!(intents[0].who.as_deref(), Some("user"));
        assert_eq!(intents[0].by_date.as_deref(), Some("Friday"));
        assert_eq!(intents[1].kind, markdown::IntentKind::Decision);
        assert_eq!(intents[1].status, "decided");
        assert_eq!(intents[2].kind, markdown::IntentKind::OpenQuestion);
        assert_eq!(intents[2].who.as_deref(), Some("case"));
        assert_eq!(intents[3].kind, markdown::IntentKind::Commitment);
        assert_eq!(intents[3].who.as_deref(), Some("sarah"));
        assert_eq!(intents[3].by_date.as_deref(), Some("Tuesday"));
    }

    #[test]
    fn action_item_assignee_uses_name_for_high_confidence_speaker_map() {
        let items = normalize_action_items_with_speaker_map(
            vec![markdown::ActionItem {
                assignee: "Speaker_1 (Samantha)".into(),
                task: "Provide the quarterly file".into(),
                due: None,
                status: "open".into(),
            }],
            &[diarize::SpeakerAttribution {
                speaker_label: "SPEAKER_1".into(),
                name: "Samantha".into(),
                confidence: diarize::Confidence::High,
                source: diarize::AttributionSource::Enrollment,
            }],
        );

        assert_eq!(items[0].assignee, "Samantha");
    }

    #[test]
    fn action_item_and_intent_keep_speaker_hint_for_medium_confidence() {
        let speaker_map = vec![diarize::SpeakerAttribution {
            speaker_label: "SPEAKER_1".into(),
            name: "Samantha".into(),
            confidence: diarize::Confidence::Medium,
            source: diarize::AttributionSource::Llm,
        }];

        let items = normalize_action_items_with_speaker_map(
            vec![markdown::ActionItem {
                assignee: "Speaker_1 (Samantha)".into(),
                task: "Provide the quarterly file".into(),
                due: None,
                status: "open".into(),
            }],
            &speaker_map,
        );
        assert_eq!(items[0].assignee, "Samantha (SPEAKER_1)");

        let intents = normalize_intents_with_speaker_map(
            vec![markdown::Intent {
                kind: markdown::IntentKind::Commitment,
                what: "Provide the quarterly file".into(),
                who: Some("Speaker_1 (Samantha)".into()),
                status: "open".into(),
                by_date: None,
            }],
            &speaker_map,
        );
        assert_eq!(intents[0].who.as_deref(), Some("Samantha (SPEAKER_1)"));
    }

    #[test]
    fn generate_title_rejects_hallucinated_cjk() {
        // Whisper hallucinates CJK text on silence — title_from_transcript
        // rejects non-ASCII-dominant candidates, so generate_title falls back
        // to "Untitled Recording".
        let transcript = "スパイシー";
        let title = generate_title(transcript, None);
        assert_eq!(title, "Untitled Recording");
    }

    #[test]
    fn generate_title_rejects_mixed_hallucination() {
        // Even with a timestamp prefix, the CJK content is rejected.
        let transcript = "[0:00] スパイシー\n[0:05] 東京タワー";
        let title = generate_title(transcript, None);
        assert_eq!(title, "Untitled Recording");
    }

    #[test]
    fn generate_title_allows_latin_with_accents() {
        // Accented Latin characters (French, Spanish, etc.) should be fine.
        let transcript = "café résumé naïve";
        let title = generate_title(transcript, None);
        assert_ne!(title, "Untitled Recording");
    }

    #[test]
    fn generate_title_allows_polish_with_extended_latin() {
        // Polish city name: Łódź has mostly non-ASCII but all Latin-extended chars.
        let transcript = "Meeting in Łódź about the project";
        let title = generate_title(transcript, None);
        assert_ne!(title, "Untitled Recording");
    }

    #[test]
    fn generate_title_allows_purely_accented_latin() {
        // All non-ASCII but entirely Latin-script — must NOT be rejected.
        // Łódź: Ł(\u{0141}) ó(\u{00F3}) d(ASCII) ź(\u{017A}) — 3/4 extended, 1/4 ASCII
        let transcript = "Łódź Gdańsk Wrocław";
        let title = generate_title(transcript, None);
        assert_ne!(title, "Untitled Recording");
    }

    #[test]
    fn generate_title_rejects_cyrillic() {
        let transcript = "Привет мир";
        let title = generate_title(transcript, None);
        assert_eq!(title, "Untitled Recording");
    }

    #[test]
    fn generate_title_below_threshold_seam() {
        // 60% Latin (below 70% strip_foreign_script threshold) but first line is CJK.
        // title_from_transcript must catch it via Latin-ratio check.
        let transcript = "[0:00] スパイシー\n[0:05] Hello world\n[0:10] Good morning\n[0:15] 東京\n[0:20] Testing";
        let title = generate_title(transcript, None);
        assert_eq!(title, "Untitled Recording");
    }

    #[test]
    fn build_entity_links_derives_people_and_projects() {
        let action_items = vec![markdown::ActionItem {
            assignee: "mat".into(),
            task: "Send pricing doc".into(),
            due: Some("Friday".into()),
            status: "open".into(),
        }];
        let decisions = vec![markdown::Decision {
            text: "Launch pricing at monthly billing per month".into(),
            topic: Some("pricing strategy".into()),
            authority: None,
            supersedes: None,
        }];
        let intents = vec![markdown::Intent {
            kind: markdown::IntentKind::Commitment,
            what: "Share revised pricing model".into(),
            who: Some("Alex Chen".into()),
            status: "open".into(),
            by_date: Some("Tuesday".into()),
        }];

        let entities = build_entity_links(
            "Q2 Pricing Discussion",
            Some("pricing review with Alex"),
            &["Case Wintermute".into()],
            &action_items,
            &decisions,
            &intents,
            &["advisor-platform".into()],
            None,
        );

        assert!(entities.people.iter().any(|entity| entity.slug == "mat"));
        assert!(entities
            .people
            .iter()
            .any(|entity| entity.slug == "alex-chen"));
        assert!(entities
            .people
            .iter()
            .any(|entity| entity.slug == "case-wintermute"));
        assert!(entities
            .projects
            .iter()
            .any(|entity| entity.slug == "pricing-strategy"));
        assert!(entities
            .projects
            .iter()
            .any(|entity| entity.slug == "advisor-platform"));
    }

    #[test]
    fn build_entity_links_rejects_task_like_or_speaker_labeled_projects() {
        let entities = build_entity_links(
            "CCRx Data Access",
            Some("Vantus Cardinal portal"),
            &["Samantha".into()],
            &[],
            &[
                markdown::Decision {
                    text: "Speaker_1 provide speaker roster and contact notes".into(),
                    topic: Some("speaker 1 provide speaker".into()),
                    authority: None,
                    supersedes: None,
                },
                markdown::Decision {
                    text: "Reach out to Cardinal about access".into(),
                    topic: Some("reach out".into()),
                    authority: None,
                    supersedes: None,
                },
                markdown::Decision {
                    text: "Pioneer asked build the custom report after review".into(),
                    topic: Some("pioneer asked build".into()),
                    authority: None,
                    supersedes: None,
                },
                markdown::Decision {
                    text: "LeaderNet 835 reconciliation remains the core workflow".into(),
                    topic: Some("leadernet 835 reconciliation".into()),
                    authority: None,
                    supersedes: None,
                },
            ],
            &[],
            &[],
            None,
        );

        let project_slugs: Vec<&str> = entities
            .projects
            .iter()
            .map(|entity| entity.slug.as_str())
            .collect();
        assert!(project_slugs.contains(&"leadernet-835-reconciliation"));
        assert!(!project_slugs.contains(&"speaker-1-provide-speaker"));
        assert!(!project_slugs.contains(&"reach-out"));
        assert!(!project_slugs.contains(&"pioneer-asked-build"));
    }

    #[test]
    fn build_entity_links_folds_email_and_slash_forms_onto_canonical_person() {
        let entities = build_entity_links(
            "Alex <> Casey",
            None,
            &[
                "alex@example.org".into(),
                "casey@example.com".into(),
                "Casey".into(),
                "Alex / Alexander".into(),
                "Dan".into(),
            ],
            &[],
            &[],
            &[],
            &[],
            None,
        );

        let slugs: Vec<&str> = entities.people.iter().map(|e| e.slug.as_str()).collect();
        assert!(slugs.contains(&"alex"), "email localpart kept: {:?}", slugs);
        assert!(slugs.contains(&"casey"), "casey present: {:?}", slugs);
        assert!(slugs.contains(&"dan"), "dan present: {:?}", slugs);
        // The email form and the bare name collapsed for Casey.
        assert_eq!(
            slugs.iter().filter(|s| **s == "casey").count(),
            1,
            "casey deduped: {:?}",
            slugs
        );
        // The slash-disambiguated form does not spawn its own slug.
        assert!(
            !slugs.contains(&"alex-alexander"),
            "slash-disambiguation stripped: {:?}",
            slugs
        );
        // The email form does not spawn a slug that includes the domain.
        assert!(
            !slugs.contains(&"alex-example-org"),
            "email domain stripped: {:?}",
            slugs
        );
        assert!(
            !slugs.contains(&"casey-example-com"),
            "email domain stripped for casey: {:?}",
            slugs
        );

        let casey = entities.people.iter().find(|e| e.slug == "casey").unwrap();
        assert!(
            casey.aliases.iter().any(|a| a == "casey@example.com"),
            "original email preserved as alias: {:?}",
            casey.aliases
        );

        let alex = entities.people.iter().find(|e| e.slug == "alex").unwrap();
        assert!(
            alex.aliases.iter().any(|a| a == "alex / alexander"),
            "original slash form preserved as alias: {:?}",
            alex.aliases
        );
    }

    #[test]
    fn build_entity_links_folds_user_identity_aliases_and_emails() {
        let identity = IdentityConfig {
            name: Some("Mat".into()),
            email: None,
            emails: vec![
                "mathieu@followthedata.co".into(),
                "matsilverstein@gmail.com".into(),
            ],
            aliases: vec!["Mathieu".into(), "Matthew".into()],
        };

        let entities = build_entity_links(
            "Weekly sync",
            None,
            &[
                "mathieu@followthedata.co".into(),
                "matsilverstein@gmail.com".into(),
                "Mat".into(),
                "Mathieu".into(),
                "Dan".into(),
                "Andrea".into(),
            ],
            &[],
            &[],
            &[],
            &[],
            Some(&identity),
        );

        let slugs: Vec<&str> = entities.people.iter().map(|e| e.slug.as_str()).collect();
        // Canonical Mat is present; all alias forms folded in.
        assert!(slugs.contains(&"mat"), "canonical mat present: {:?}", slugs);
        assert!(!slugs.contains(&"mathieu"), "mathieu folded: {:?}", slugs);
        assert!(
            !slugs.contains(&"matsilverstein"),
            "matsilverstein folded: {:?}",
            slugs
        );
        assert!(!slugs.contains(&"matthew"), "matthew folded: {:?}", slugs);
        // Non-user entities untouched.
        assert!(slugs.contains(&"dan"), "dan present: {:?}", slugs);
        assert!(slugs.contains(&"andrea"), "andrea present: {:?}", slugs);

        let mat = entities.people.iter().find(|e| e.slug == "mat").unwrap();
        assert!(
            mat.aliases.iter().any(|a| a == "mathieu"),
            "Mathieu folded as alias: {:?}",
            mat.aliases
        );
        assert!(
            mat.aliases.iter().any(|a| a == "mathieu@followthedata.co"),
            "work email folded as alias: {:?}",
            mat.aliases
        );
        assert!(
            mat.aliases.iter().any(|a| a == "matsilverstein@gmail.com"),
            "personal email folded as alias: {:?}",
            mat.aliases
        );
    }

    #[test]
    fn fold_user_identity_skips_meeting_without_user() {
        // If the user isn't a participant, don't invent an entity.
        let identity = IdentityConfig {
            name: Some("Mat".into()),
            email: Some("mathieu@followthedata.co".into()),
            emails: vec![],
            aliases: vec!["Mathieu".into()],
        };

        let entities = build_entity_links(
            "Third-party call",
            None,
            &["Dan".into(), "Andrea".into()],
            &[],
            &[],
            &[],
            &[],
            Some(&identity),
        );

        let slugs: Vec<&str> = entities.people.iter().map(|e| e.slug.as_str()).collect();
        assert!(!slugs.contains(&"mat"), "mat not invented: {:?}", slugs);
        assert_eq!(slugs.len(), 2);
    }

    #[test]
    fn identity_config_all_user_aliases_dedupes_and_preserves_order() {
        let identity = IdentityConfig {
            name: Some("Mat".into()),
            email: Some("mathieu@followthedata.co".into()),
            emails: vec![
                "mathieu@followthedata.co".into(), // dup of legacy email
                "matsilverstein@gmail.com".into(),
                "   ".into(), // blank
            ],
            aliases: vec!["Mathieu".into(), "mathieu".into()],
        };

        let aliases = identity.all_user_aliases();
        assert_eq!(
            aliases,
            vec![
                "mathieu@followthedata.co".to_string(),
                "matsilverstein@gmail.com".to_string(),
                "Mathieu".to_string(),
            ],
            "legacy email first, dedup case-insensitively, skip blanks"
        );
    }

    #[test]
    fn strip_name_disambiguation_handles_common_shapes() {
        assert_eq!(strip_name_disambiguation("Mat / Matthew"), "Mat");
        assert_eq!(strip_name_disambiguation("Mat"), "Mat");
        // No surrounding spaces around "/" means it's not a disambiguation hedge.
        assert_eq!(strip_name_disambiguation("A/B Testing"), "A/B Testing");
    }

    #[test]
    fn strip_email_domain_returns_localpart_only_for_valid_emails() {
        assert_eq!(strip_email_domain("alex@example.org"), "alex");
        assert_eq!(strip_email_domain("casey@example.com"), "casey");
        // Missing dot in domain → not treated as email.
        assert_eq!(strip_email_domain("user@localhost"), "user@localhost");
        // Missing local part → unchanged.
        assert_eq!(strip_email_domain("@bad.tld"), "@bad.tld");
        // No '@' at all.
        assert_eq!(strip_email_domain("Alex"), "Alex");
    }

    #[test]
    fn merge_attendees_strips_name_disambiguation_hedge() {
        let merged = merge_attendees(
            &["Andrea".into()],
            &["Alex / Alexander".into(), "Casey".into()],
        );
        assert!(
            merged.iter().any(|a| a == "Alex"),
            "slash suffix stripped in attendees: {:?}",
            merged
        );
        assert!(
            !merged.iter().any(|a| a == "Alex / Alexander"),
            "slash-hedge form not kept: {:?}",
            merged
        );
    }

    #[test]
    fn build_decode_hints_uses_identity_aliases_attendees_and_context() {
        let identity = IdentityConfig {
            name: Some("Mat".into()),
            email: Some("mat@example.com".into()),
            emails: vec!["mathieu@work.com".into()],
            aliases: vec!["Mathieu".into(), "Matthew".into()],
        };

        let hints = build_decode_hints(
            Some("X1 / Planning Review"),
            Some("Mat with Alex Chen"),
            Some("Asana migration with Box"),
            &[
                "mat@example.com".into(),
                "alex.chen@example.com".into(),
                "Casey / Casey Winters".into(),
            ],
            Some(&identity),
        );

        assert_eq!(
            hints.whisper_initial_prompt().as_deref(),
            Some(
                "Names and terms that may appear in this audio: Mat, Mathieu, Matthew, Alex Chen, Casey, X1, Planning Review, Asana migration. Preserve spelling exactly when heard."
            )
        );
    }

    #[test]
    fn build_decode_hints_skips_identity_when_user_not_in_attendees() {
        let identity = IdentityConfig {
            name: Some("Mat".into()),
            email: Some("mat@example.com".into()),
            emails: vec!["mathieu@work.com".into()],
            aliases: vec!["Mathieu".into(), "Matthew".into()],
        };

        let hints = build_decode_hints(
            Some("X1 / Planning Review"),
            Some("Alex Chen with Casey Winters"),
            Some("Asana migration with Box"),
            &[
                "alex.chen@example.com".into(),
                "Casey / Casey Winters".into(),
            ],
            Some(&identity),
        );

        let prompt = hints.whisper_initial_prompt().expect("prompt");
        assert!(!prompt.contains("Mathieu"));
        assert!(!prompt.contains("Matthew"));
        assert!(!prompt.contains("Mat,"));
        assert!(prompt.contains("Alex Chen"));
        assert!(prompt.contains("Casey"));
    }

    #[test]
    fn normalize_self_name_refs_in_transcript_rewrites_intro_patterns_only() {
        let transcript = "[SPEAKER_1 0:00] Hey, this is Matt testing one more time.\n[SPEAKER_1 0:04] Matt is testing the path repro.\n[SPEAKER_2 0:08] Another speaker said Matt Mullenweg messaged me.\n";
        let normalized =
            normalize_self_name_refs_in_transcript(transcript, "Mat", &["Matt".into()]);

        assert!(normalized.contains("Hey, this is Mat testing one more time."));
        assert!(normalized.contains("Mat is testing the path repro."));
        assert!(normalized.contains("Matt Mullenweg messaged me."));
        assert!(!normalized.contains("Hey, this is Matt testing one more time."));
    }

    #[test]
    fn normalize_self_name_refs_in_transcript_uses_fuzzy_intro_match_without_explicit_variant() {
        let transcript = "[SPEAKER_1 0:00] This is Matt and I'm testing.\n";
        let normalized = normalize_self_name_refs_in_transcript(transcript, "Mat", &[]);

        assert!(
            normalized.contains("This is Mat and I'm testing."),
            "{}",
            normalized
        );
    }

    #[test]
    fn collect_user_participant_variants_uses_attendee_forms_matching_identity() {
        let identity = IdentityConfig {
            name: Some("Mat".into()),
            email: Some("mat@example.com".into()),
            emails: vec![],
            aliases: vec!["Mathieu".into()],
        };

        let variants =
            collect_user_participant_variants(&["Matt".into(), "Alex Chen".into()], &identity);

        // "Matt" no longer needs to be treated as an explicit participant
        // variant here because the guarded intro matcher handles close
        // self-name fuzz like "This is Matt" at rewrite time.
        assert_eq!(variants, vec!["Mathieu".to_string()]);
    }

    #[test]
    fn write_transcript_artifact_normalizes_self_name_for_title_and_body() {
        let dir = tempfile::TempDir::new().unwrap();
        let audio_path = dir.path().join("memo.wav");
        std::fs::write(&audio_path, vec![0u8; 64_044]).unwrap();

        let mut config = Config::default();
        config.output_dir = dir.path().to_path_buf();
        config.identity.name = Some("Mat".into());
        config.identity.aliases = vec!["Matt".into()];

        let context = BackgroundPipelineContext {
            calendar_event: Some(crate::calendar::CalendarEvent {
                title: "meeting".into(),
                start: Local::now().to_rfc3339(),
                minutes_until: 0,
                attendees: vec!["Matt".into(), "Alex Chen".into()],
                url: None,
            }),
            ..BackgroundPipelineContext::default()
        };

        let artifact = write_transcript_artifact(
            &audio_path,
            ContentType::Meeting,
            None,
            &config,
            &context,
            None,
            "[SPEAKER_1 0:00] Matt is outlining onboarding follow up.\n".into(),
            crate::transcribe::FilterStats::default(),
            0,
        )
        .unwrap();

        assert!(
            artifact
                .transcript
                .contains("Mat is outlining onboarding follow up."),
            "{}",
            artifact.transcript
        );
        assert!(!artifact.transcript.contains("Matt is outlining"));
        assert_eq!(
            artifact.frontmatter.title,
            "Mat Is Outlining Onboarding Follow Up"
        );
    }

    #[test]
    fn is_task_like_project_candidate_requires_more_than_a_verb_like_start() {
        assert!(!is_task_like_project_candidate(
            "review board",
            Some("Review Board"),
        ));
        assert!(!is_task_like_project_candidate(
            "run club",
            Some("Run Club")
        ));
        assert!(!is_task_like_project_candidate(
            "study group",
            Some("Study Group"),
        ));
        assert!(is_task_like_project_candidate(
            "review q3 budget",
            Some("Review the Q3 budget"),
        ));
        assert!(is_task_like_project_candidate(
            "run tests",
            Some("Run the tests"),
        ));
        assert!(is_task_like_project_candidate(
            "speaker 1 provide quarterly report",
            Some("Speaker 1 provide quarterly report"),
        ));
        assert!(!is_task_like_project_candidate(
            "asana migration",
            Some("Asana migration"),
        ));
    }

    #[test]
    fn synthetic_frontmatter_cleanup_keeps_names_and_drops_bad_projects() {
        let attendees = normalize_attendees_with_speaker_map(
            &merge_attendees(
                &["Andrea".into(), "Dan".into()],
                &["Speaker 1 / Samantha".into(), "Speaker_2 (Mat)".into()],
            ),
            &[diarize::SpeakerAttribution {
                speaker_label: "SPEAKER_1".into(),
                name: "Samantha".into(),
                confidence: diarize::Confidence::Medium,
                source: diarize::AttributionSource::Llm,
            }],
        );
        let action_items = normalize_action_items_with_speaker_map(
            vec![markdown::ActionItem {
                assignee: "Speaker_1 (Samantha)".into(),
                task: "Provide the quarterly LeaderNet file".into(),
                due: None,
                status: "open".into(),
            }],
            &[diarize::SpeakerAttribution {
                speaker_label: "SPEAKER_1".into(),
                name: "Samantha".into(),
                confidence: diarize::Confidence::Medium,
                source: diarize::AttributionSource::Llm,
            }],
        );
        let intents = normalize_intents_with_speaker_map(
            vec![markdown::Intent {
                kind: markdown::IntentKind::Commitment,
                what: "Provide the quarterly LeaderNet file".into(),
                who: Some("Speaker_1 (Samantha)".into()),
                status: "open".into(),
                by_date: None,
            }],
            &[diarize::SpeakerAttribution {
                speaker_label: "SPEAKER_1".into(),
                name: "Samantha".into(),
                confidence: diarize::Confidence::Medium,
                source: diarize::AttributionSource::Llm,
            }],
        );
        let entities = build_entity_links(
            "CCRx Data Access",
            Some("LeaderNet 835 reconciliation"),
            &attendees,
            &action_items,
            &[markdown::Decision {
                text: "Speaker_1 provide speaker roster and contact notes".into(),
                topic: Some("speaker 1 provide speaker".into()),
                authority: None,
                supersedes: None,
            }],
            &intents,
            &[],
            None,
        );

        let frontmatter = markdown::Frontmatter {
            title: "CCRx Data Access".into(),
            r#type: ContentType::Meeting,
            date: Local::now(),
            duration: "21m".into(),
            source: None,
            status: Some(OutputStatus::Complete),
            tags: vec![],
            attendees,
            attendees_raw: None,
            calendar_event: None,
            people: entities
                .people
                .iter()
                .map(|entity| entity.label.clone())
                .collect(),
            entities,
            device: None,
            captured_at: None,
            context: None,
            action_items,
            decisions: vec![],
            intents,
            recorded_by: Some("Mat".into()),
            visibility: None,
            speaker_map: vec![diarize::SpeakerAttribution {
                speaker_label: "SPEAKER_1".into(),
                name: "Samantha".into(),
                confidence: diarize::Confidence::Medium,
                source: diarize::AttributionSource::Llm,
            }],
            filter_diagnosis: None,
        };

        assert_eq!(
            frontmatter.attendees,
            vec!["Andrea", "Dan", "Samantha", "Mat"]
        );
        assert_eq!(frontmatter.action_items[0].assignee, "Samantha (SPEAKER_1)");
        assert_eq!(
            frontmatter.intents[0].who.as_deref(),
            Some("Samantha (SPEAKER_1)")
        );
        assert!(frontmatter
            .entities
            .projects
            .iter()
            .all(|entity| entity.slug != "speaker-1-provide-speaker"));
    }

    #[test]
    fn derive_structured_tags_for_memo_includes_source_people_projects_and_guardrails() {
        let entities = build_entity_links(
            "Pricing Idea",
            Some("pricing review with Alex"),
            &["Alex Chen".into()],
            &[],
            &[markdown::Decision {
                text: "Use annual billing for premium users".into(),
                topic: Some("pricing strategy".into()),
                authority: None,
                supersedes: None,
            }],
            &[markdown::Intent {
                kind: markdown::IntentKind::Commitment,
                what: "Send the revised deck".into(),
                who: Some("Alex Chen".into()),
                status: "open".into(),
                by_date: Some("Friday".into()),
            }],
            &[],
            None,
        );

        let tags = derive_structured_tags(
            ContentType::Memo,
            Some("voice-memos"),
            Some("iPhone 16 Pro"),
            &entities,
            &[markdown::Decision {
                text: "Use annual billing for premium users".into(),
                topic: Some("pricing strategy".into()),
                authority: None,
                supersedes: None,
            }],
            &[markdown::Intent {
                kind: markdown::IntentKind::Commitment,
                what: "Send the revised deck".into(),
                who: Some("Alex Chen".into()),
                status: "open".into(),
                by_date: Some("Friday".into()),
            }],
        );

        assert!(tags.iter().any(|tag| tag == "memo"));
        assert!(tags.iter().any(|tag| tag == "source:voice-memos"));
        assert!(tags.iter().any(|tag| tag == "device:iphone-16-pro"));
        assert!(tags.iter().any(|tag| tag == "person:alex-chen"));
        assert!(tags.iter().any(|tag| tag == "project:pricing-idea"));
        assert!(tags.iter().any(|tag| tag == "topic:pricing-strategy"));
        assert!(tags.iter().any(|tag| tag == "has-actions"));
        assert!(tags.iter().any(|tag| tag == "has-decisions"));
        assert!(tags.len() <= 8);
    }

    #[test]
    #[cfg(unix)]
    fn run_post_record_hook_executes_and_receives_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let marker = dir.path().join("hook-ran.txt");
        let transcript = dir.path().join("test-meeting.md");
        std::fs::write(&transcript, "test content").unwrap();

        // The hook is invoked as: sh -c '{cmd} "$1"' -- /path/to/transcript.md
        // So the user's command receives the transcript path as $1.
        // Use a simple script that copies $1 to the marker location.
        let script = dir.path().join("hook.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ncp \"$1\" '{}'\n", marker.display()),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let config = Config {
            hooks: crate::config::HooksConfig {
                post_record: Some(script.display().to_string()),
            },
            ..Config::default()
        };

        // Replicate the exact invocation from run_post_record_hook
        let cmd = config.hooks.post_record.as_ref().unwrap();
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("{} \"$1\"", cmd))
            .arg("--")
            .arg(transcript.display().to_string())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "hook failed (stderr={})", stderr);
        assert!(marker.exists(), "hook should have created the marker file");
        let contents = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(contents, "test content");
    }

    #[test]
    #[ignore = "requires MINUTES_PROPER_NAME_EVAL_CORPUS pointing at a local corpus manifest"]
    fn proper_name_eval_corpus() {
        let corpus_path = match std::env::var("MINUTES_PROPER_NAME_EVAL_CORPUS") {
            Ok(path) => std::path::PathBuf::from(path),
            Err(_) => {
                eprintln!(
                    "proper-name-eval skipped: set MINUTES_PROPER_NAME_EVAL_CORPUS=/abs/path/to/corpus.json"
                );
                return;
            }
        };

        let report = crate::autoresearch::run_decode_hint_eval_corpus(
            &corpus_path,
            &crate::autoresearch::DecodeHintEvalOptions::default(),
        )
        .unwrap_or_else(|error| panic!("proper-name eval failed: {}", error));

        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("serialize eval results")
        );
        if !report.failure_messages.is_empty() {
            panic!(
                "proper-name eval failures:\n{}",
                report.failure_messages.join("\n")
            );
        }
    }
}
