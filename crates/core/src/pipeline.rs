use crate::config::Config;
use crate::diarize;
use crate::error::MinutesError;
use crate::logging;
use crate::markdown::{self, ContentType, Frontmatter, OutputStatus, WriteResult};
use crate::notes;
use crate::summarize;
use crate::transcribe;
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

fn single_stem_speaker_self_attribution(
    config: &Config,
    voice_result: &VoiceMatchResult,
    diarization_from_stems: bool,
    diarization_num_speakers: usize,
    transcript: &str,
    transcript_labels: &[String],
    l2_labels: &std::collections::HashSet<String>,
) -> Option<diarize::SpeakerAttribution> {
    if !diarization_from_stems || diarization_num_speakers != 1 || !l2_labels.is_empty() {
        return None;
    }

    let speaker_label = if transcript_labels.len() == 1 && transcript_labels[0] == "SPEAKER_0" {
        "SPEAKER_0".to_string()
    } else if transcript.contains("[UNKNOWN ") {
        "UNKNOWN".to_string()
    } else {
        return None;
    };

    let my_name = config.identity.name.as_ref()?;
    let my_confidence = if voice_result.self_profile_exists {
        diarize::Confidence::High
    } else {
        diarize::Confidence::Medium
    };
    let my_source = if voice_result.self_profile_exists {
        diarize::AttributionSource::Enrollment
    } else {
        diarize::AttributionSource::Deterministic
    };

    Some(diarize::SpeakerAttribution {
        speaker_label,
        name: my_name.clone(),
        confidence: my_confidence,
        source: my_source,
    })
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

    let step_start = std::time::Instant::now();
    let result = transcribe::transcribe(audio_path, config)?;
    let transcribe_ms = step_start.elapsed().as_millis() as u64;
    let transcript = result.text;
    let filter_stats = result.stats;
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

    let matched_event = if content_type == ContentType::Meeting {
        context
            .calendar_event
            .clone()
            .or_else(|| crate::calendar::events_overlapping_now().first().cloned())
    } else {
        None
    };
    let calendar_event_title = matched_event.as_ref().map(|event| event.title.clone());
    let attendees = matched_event
        .as_ref()
        .map(|event| event.attendees.clone())
        .unwrap_or_default();

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

    let frontmatter = Frontmatter {
        title: auto_title,
        r#type: content_type,
        date: context.recorded_at.unwrap_or_else(Local::now),
        duration: estimate_duration(audio_path),
        source,
        status,
        tags: vec![],
        attendees,
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
            diarization_from_stems = result.from_stems;
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

        summarize::summarize_with_screens(&transcript_with_notes, &screen_files, config).map(
            |summary| {
                structured_actions = extract_action_items(&summary);
                structured_decisions = extract_decisions(&summary);
                structured_intents = extract_intents(&summary);
                summary_participants = summary.participants.clone();
                let formatted = summarize::format_summary(&summary);
                raw_summary = Some(summary);
                formatted
            },
        )
    } else {
        None
    };

    if !screen_files.is_empty()
        && !config.screen_context.keep_after_summary
        && std::fs::remove_dir_all(&screen_dir).is_ok()
    {
        tracing::info!(dir = %screen_dir.display(), "screen captures cleaned up");
    }

    let mut attendees = artifact.frontmatter.attendees.clone();
    let mut seen_lower: std::collections::HashSet<String> =
        attendees.iter().map(|name| name.to_lowercase()).collect();
    for participant in &summary_participants {
        let lower = participant.to_lowercase();
        if !lower.is_empty() && seen_lower.insert(lower) {
            attendees.push(participant.clone());
        }
    }

    let mut speaker_map: Vec<diarize::SpeakerAttribution> = Vec::new();
    if diarization_num_speakers > 0 && artifact.frontmatter.r#type == ContentType::Meeting {
        // Level 2: Voice enrollment matching via embedding cosine similarity.
        let voice_result = match_speakers_by_voice(config, &diarization_embeddings);
        speaker_map.extend(voice_result.attributions.clone());

        // Level 0: deterministic 1-on-1 mapping (skip if Level 2 already matched)
        let transcript_labels = crate::summarize::extract_speaker_labels_pub(&transcript);
        let l2_labels: std::collections::HashSet<String> = speaker_map
            .iter()
            .map(|a| a.speaker_label.clone())
            .collect();

        if !attendees.is_empty()
            && diarization_num_speakers == attendees.len()
            && diarization_num_speakers == 2
            && transcript_labels.len() == 2
            && l2_labels.is_empty()
        {
            if let Some(my_name) = config.identity.name.as_ref() {
                let my_slug = slugify(my_name);
                let other = attendees
                    .iter()
                    .find(|attendee| slugify(attendee) != my_slug);
                if let Some(other_name) = other {
                    let my_confidence = if voice_result.self_profile_exists {
                        diarize::Confidence::High
                    } else {
                        diarize::Confidence::Medium
                    };
                    let my_source = if voice_result.self_profile_exists {
                        diarize::AttributionSource::Enrollment
                    } else {
                        diarize::AttributionSource::Deterministic
                    };
                    speaker_map.push(diarize::SpeakerAttribution {
                        speaker_label: transcript_labels[0].clone(),
                        name: my_name.clone(),
                        confidence: my_confidence,
                        source: my_source,
                    });
                    speaker_map.push(diarize::SpeakerAttribution {
                        speaker_label: transcript_labels[1].clone(),
                        name: other_name.clone(),
                        confidence: diarize::Confidence::Medium,
                        source: diarize::AttributionSource::Deterministic,
                    });
                }
            }
        }

        if let Some(attr) = single_stem_speaker_self_attribution(
            config,
            &voice_result,
            diarization_from_stems,
            diarization_num_speakers,
            &transcript,
            &transcript_labels,
            &l2_labels,
        ) {
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
            for attribution in summarize::map_speakers(&transcript, &attendees, config) {
                if !mapped_labels.contains(&attribution.speaker_label) {
                    speaker_map.push(attribution);
                }
            }
        }

        if speaker_map
            .iter()
            .any(|attribution| attribution.confidence == diarize::Confidence::High)
        {
            transcript = diarize::apply_confirmed_names(&transcript, &speaker_map);
        }
    }

    let entities = build_entity_links(
        &artifact.frontmatter.title,
        context.pre_context.as_deref(),
        &attendees,
        &structured_actions,
        &structured_decisions,
        &structured_intents,
        &artifact.frontmatter.tags,
    );
    let people = entities
        .people
        .iter()
        .map(|entity| entity.label.clone())
        .collect();

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
    let result = markdown::rewrite_with_retry_path(
        &artifact.write_result.path,
        &frontmatter,
        &transcript,
        summary.as_deref(),
        context.user_notes.as_deref(),
        Some(audio_path),
    )?;

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

    // Step 1: Transcribe (always)
    on_progress(PipelineStage::Transcribing);
    tracing::info!(step = "transcribe", file = %audio_path.display(), "transcribing audio");
    let step_start = std::time::Instant::now();
    let result = transcribe::transcribe(audio_path, config)?;
    let transcribe_ms = step_start.elapsed().as_millis() as u64;
    let transcript = result.text;
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
            diarization_from_stems = result.from_stems;
            diarization_embeddings = result.speaker_embeddings.clone();
            diarize::apply_speakers(&transcript, &result)
        } else {
            transcript
        }
    } else {
        transcript
    };

    // Read user notes and pre-meeting context (if any)
    let user_notes = notes::read_notes();
    let pre_context = notes::read_context();

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
        summarize::summarize_with_screens(&transcript_with_notes, &screen_files, config).map(|s| {
            structured_actions = extract_action_items(&s);
            structured_decisions = extract_decisions(&s);
            structured_intents = extract_intents(&s);
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

    // Clean up screen captures (runs regardless of summarization setting — fixes race)
    if !screen_files.is_empty()
        && !config.screen_context.keep_after_summary
        && std::fs::remove_dir_all(&screen_dir).is_ok()
    {
        tracing::info!(dir = %screen_dir.display(), "screen captures cleaned up");
    }

    // Step 4: Match calendar event + merge attendees
    on_progress(PipelineStage::Saving);

    // Query calendar for events overlapping the recording window
    let calendar_events = if content_type == ContentType::Meeting {
        crate::calendar::events_overlapping_now()
    } else {
        Vec::new()
    };

    // Pick the best matching calendar event (closest to now, or the one currently happening)
    let matched_event = calendar_events.first();
    let calendar_event_title = matched_event.map(|e| e.title.clone());
    let calendar_attendees: Vec<String> = matched_event
        .map(|e| e.attendees.clone())
        .unwrap_or_default();

    if let Some(ref title) = calendar_event_title {
        tracing::info!(event = %title, attendees = calendar_attendees.len(), "matched calendar event");
    }

    // Merge attendees: calendar + transcript participants (deduplicate, case-insensitive)
    let mut attendees: Vec<String> = Vec::new();
    let mut seen_lower: std::collections::HashSet<String> = std::collections::HashSet::new();

    for name in calendar_attendees.iter().chain(summary_participants.iter()) {
        let lower = name.to_lowercase();
        if !lower.is_empty() && seen_lower.insert(lower) {
            attendees.push(name.clone());
        }
    }

    if !attendees.is_empty() {
        tracing::info!(attendees = ?attendees, "merged attendee list");
    }

    // Step 4b: Speaker attribution
    // Level 2 → Level 0 → Level 1 (voice enrollment → deterministic → LLM)
    let mut speaker_map: Vec<diarize::SpeakerAttribution> = Vec::new();
    let mut transcript = transcript;

    if diarization_num_speakers > 0 && content_type == ContentType::Meeting {
        // Level 2: Voice enrollment matching via embedding cosine similarity.
        let voice_result = match_speakers_by_voice(config, &diarization_embeddings);
        speaker_map.extend(voice_result.attributions.clone());

        // Level 0: deterministic 1-on-1 mapping (skip if Level 2 already matched)
        let transcript_labels = crate::summarize::extract_speaker_labels_pub(&transcript);
        let l2_labels: std::collections::HashSet<String> = speaker_map
            .iter()
            .map(|a| a.speaker_label.clone())
            .collect();

        if !attendees.is_empty()
            && diarization_num_speakers == attendees.len()
            && diarization_num_speakers == 2
            && transcript_labels.len() == 2
            && l2_labels.is_empty()
        {
            if let Some(my_name) = config.identity.name.as_ref() {
                let my_slug = slugify(my_name);
                let other = attendees.iter().find(|a| slugify(a) != my_slug);
                if let Some(other_name) = other {
                    let my_confidence = if voice_result.self_profile_exists {
                        diarize::Confidence::High
                    } else {
                        diarize::Confidence::Medium
                    };
                    let my_source = if voice_result.self_profile_exists {
                        diarize::AttributionSource::Enrollment
                    } else {
                        diarize::AttributionSource::Deterministic
                    };

                    speaker_map.push(diarize::SpeakerAttribution {
                        speaker_label: transcript_labels[0].clone(),
                        name: my_name.clone(),
                        confidence: my_confidence,
                        source: my_source,
                    });
                    speaker_map.push(diarize::SpeakerAttribution {
                        speaker_label: transcript_labels[1].clone(),
                        name: other_name.clone(),
                        confidence: diarize::Confidence::Medium,
                        source: diarize::AttributionSource::Deterministic,
                    });
                    tracing::info!(
                        my_name = %my_name,
                        my_confidence = ?my_confidence,
                        other_name = %other_name,
                        labels = ?transcript_labels,
                        "Level 0: deterministic 1-on-1 speaker attribution"
                    );
                }
            }
        }

        if let Some(attr) = single_stem_speaker_self_attribution(
            config,
            &voice_result,
            diarization_from_stems,
            diarization_num_speakers,
            &transcript,
            &transcript_labels,
            &l2_labels,
        ) {
            speaker_map.push(attr);
        }

        // Level 1: LLM suggestions for unmapped speakers
        let mapped_labels: std::collections::HashSet<String> = speaker_map
            .iter()
            .map(|a| a.speaker_label.clone())
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
            let llm_attributions = summarize::map_speakers(&transcript, &attendees, config);
            for attr in llm_attributions {
                if !mapped_labels.contains(&attr.speaker_label) {
                    speaker_map.push(attr);
                }
            }
        }

        // Apply high-confidence attributions to transcript
        if speaker_map
            .iter()
            .any(|a| a.confidence == diarize::Confidence::High)
        {
            transcript = diarize::apply_confirmed_names(&transcript, &speaker_map);
        }
    }

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
    let entities = build_entity_links(
        &auto_title,
        pre_context.as_deref(),
        &attendees,
        &structured_actions,
        &structured_decisions,
        &structured_intents,
        &[],
    );
    let people = entities
        .people
        .iter()
        .map(|entity| entity.label.clone())
        .collect();

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

    // Use sidecar captured_at, then audio file creation time, then now() as last resort
    let recording_date = sidecar
        .and_then(|s| s.captured_at)
        .or_else(|| metadata.created().ok().map(DateTime::<Local>::from))
        .unwrap_or_else(Local::now);

    let frontmatter = Frontmatter {
        title: auto_title,
        r#type: content_type,
        date: recording_date,
        duration,
        source,
        status,
        tags: vec![],
        attendees,
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
    let result = markdown::write_with_retry_path(
        &frontmatter,
        &transcript,
        summary.as_deref(),
        user_notes.as_deref(),
        Some(audio_path),
        config,
    )?;
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
    let bytes = std::fs::metadata(audio_path).map(|m| m.len()).unwrap_or(0);

    // WAV header is 44 bytes, then raw PCM at 32000 bytes/sec (16kHz 16-bit mono)
    let secs = if bytes > 44 { (bytes - 44) / 32_000 } else { 0 };

    let mins = secs / 60;
    let remaining_secs = secs % 60;
    if mins > 0 {
        format!("{}m {}s", mins, remaining_secs)
    } else {
        format!("{}s", remaining_secs)
    }
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
    let stripped = strip_lead_in_phrase(&first_line);
    let candidate = normalize_space(&stripped);

    if candidate.is_empty() {
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

fn clean_transcript_line(line: &str) -> Option<String> {
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

fn normalize_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
        None
    } else {
        Some(words.join(" ").to_lowercase())
    }
}

fn build_entity_links(
    title: &str,
    pre_context: Option<&str>,
    attendees: &[String],
    action_items: &[markdown::ActionItem],
    decisions: &[markdown::Decision],
    intents: &[markdown::Intent],
    tags: &[String],
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

fn add_person_entity(entities: &mut BTreeMap<String, (String, BTreeSet<String>)>, raw: &str) {
    let trimmed = raw.trim().trim_start_matches('@').trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unassigned") {
        return;
    }

    let label = trimmed
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
    entry.1.insert(trimmed.to_lowercase());
    if raw.trim() != trimmed {
        entry.1.insert(raw.trim().to_lowercase());
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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

        let attr = single_stem_speaker_self_attribution(
            &config,
            &voice_result,
            true,
            1,
            "[SPEAKER_0 0:00] hello\n",
            &labels,
            &l2_labels,
        )
        .expect("single stem speaker should map to self");

        assert_eq!(attr.name, "Mat");
        assert_eq!(attr.speaker_label, "SPEAKER_0");
        assert_eq!(attr.confidence, diarize::Confidence::High);
        assert_eq!(attr.source, diarize::AttributionSource::Enrollment);
    }

    #[test]
    fn single_stem_speaker_self_attribution_respects_guards() {
        let mut config = Config::default();
        config.identity.name = Some("Mat".into());
        let voice_result = VoiceMatchResult {
            attributions: vec![],
            self_profile_exists: false,
        };
        let labels = vec!["SPEAKER_1".to_string()];
        let l2_labels = std::collections::HashSet::new();

        assert!(single_stem_speaker_self_attribution(
            &config,
            &voice_result,
            true,
            1,
            "[SPEAKER_1 0:00] hello\n",
            &labels,
            &l2_labels,
        )
        .is_none());
        assert!(single_stem_speaker_self_attribution(
            &config,
            &voice_result,
            false,
            1,
            "[SPEAKER_0 0:00] hello\n",
            &["SPEAKER_0".to_string()],
            &std::collections::HashSet::new(),
        )
        .is_none());
    }

    #[test]
    fn single_stem_speaker_self_attribution_handles_unknown_label() {
        let mut config = Config::default();
        config.identity.name = Some("Mat".into());
        let voice_result = VoiceMatchResult {
            attributions: vec![],
            self_profile_exists: true,
        };

        let attr = single_stem_speaker_self_attribution(
            &config,
            &voice_result,
            true,
            1,
            "[UNKNOWN 0:00] Hello there\n",
            &[],
            &std::collections::HashSet::new(),
        )
        .expect("single unknown label should still map to self");

        assert_eq!(attr.speaker_label, "UNKNOWN");
        assert_eq!(attr.name, "Mat");
        assert_eq!(attr.confidence, diarize::Confidence::High);
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
