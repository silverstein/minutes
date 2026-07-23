use crate::config::Config;
use crate::error::MarkdownError;
use chrono::{DateTime, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone};
use schemars::JsonSchema;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

// ──────────────────────────────────────────────────────────────
// Meeting/memo markdown output.
// All files written with 0600 permissions (owner read/write only)
// because transcripts contain sensitive conversation content.
// ──────────────────────────────────────────────────────────────

/// Content types for output files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    Meeting,
    Memo,
    Dictation,
}

/// Output status markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OutputStatus {
    Complete,
    NoSpeech,
    TranscriptOnly,
    /// Transcription completed but one or more summarization-side steps
    /// fell back to empty output (e.g. agent timeout, empty summary).
    /// Per-step failures are recorded in [`Frontmatter::processing_warnings`].
    Degraded,
}

/// Attested basis for capturing a conversation.
///
/// This is privacy metadata only, not a determination about requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConsentBasis {
    VerbalAllParties,
    NoticeInInvite,
    RecordedDisclosed,
    #[serde(rename = "na")]
    NotApplicable,
    Unattested,
}

impl ConsentBasis {
    /// Stable serialized string used in frontmatter and CLI flags.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VerbalAllParties => "verbal_all_parties",
            Self::NoticeInInvite => "notice_in_invite",
            Self::RecordedDisclosed => "recorded_disclosed",
            Self::NotApplicable => "na",
            Self::Unattested => "unattested",
        }
    }
}

impl FromStr for ConsentBasis {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim() {
            "verbal_all_parties" => Ok(Self::VerbalAllParties),
            "notice_in_invite" => Ok(Self::NoticeInInvite),
            "recorded_disclosed" => Ok(Self::RecordedDisclosed),
            "na" => Ok(Self::NotApplicable),
            "unattested" => Ok(Self::Unattested),
            other => Err(format!(
                "unknown consent basis: {other}. Use verbal_all_parties, notice_in_invite, recorded_disclosed, na, or unattested."
            )),
        }
    }
}

/// How a meeting artifact was captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CapturePolicy {
    /// No audio was captured for this meeting artifact.
    None,
}

/// Sensitivity designation for agent-facing policy layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    /// Standard meeting artifact.
    Normal,
    /// Restricted artifact; agent surfaces enforce this in a later wave.
    Restricted,
}

/// Human debrief state for no-capture meeting artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DebriefStatus {
    /// The meeting was stopped without an interactive debrief.
    Pending,
    /// A human supplied the debrief (at stop time or later via the
    /// assistant flow); the artifact is the finished record.
    Complete,
    /// The human marked the debrief unnecessary (test run, accidental
    /// trigger); nothing further is expected.
    NotApplicable,
}

/// A non-fatal failure of a post-transcript pipeline step.
///
/// When any step degrades, the meeting's [`OutputStatus`] is promoted to
/// [`OutputStatus::Degraded`] and the failure context is appended here so
/// the markdown is honest about what is missing. Files are then greppable
/// for "what needs re-running" (`status: degraded` in frontmatter).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessingWarning {
    /// The pipeline step that produced the warning.
    pub step: String,
    /// Machine-readable failure reason (e.g. `agent_timeout`, `empty_output`).
    pub reason: String,
    /// For timeout reasons, the budget that was exceeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Optional human-readable detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Health of the Level-1 speaker-naming (`speaker_mapping`) step, surfaced in
/// frontmatter so a meeting that shipped anonymous is greppable and re-runnable
/// (#382/#384). Counts are recorded separately from confidence so "a map exists"
/// is never confused with "every speaker was named".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpeakerMappingHealth {
    /// `ok` (some labels named), `empty` (ran, no confident matches), `skipped`
    /// (no anonymous labels or no attendees to map against). Mirrors the
    /// `speaker_mapping` JSONL `outcome` vocabulary.
    pub status: String,
    /// Engine/model hint used (e.g. `agent:claude`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Distinct diarization speaker labels present (proxy for the raw diarization
    /// speaker count, which needs the audio and so isn't recoverable on a redo).
    pub diarized_speakers: usize,
    /// How many of those labels received a name.
    pub mapped_speakers: usize,
    /// Size of the attendee pool offered to the mapper.
    pub attendees: usize,
    /// Wall-clock of the mapping call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Machine-readable reason when `status` is not `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// RFC3339 timestamp of the most recent mapping run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
}

/// Health of the summarization stage, surfaced in frontmatter so a re-run of
/// the AI pass (`minutes resummarize`, #523) is greppable and auditable.
/// Shape mirrors [`SpeakerMappingHealth`] per maintainer guidance on #523.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SummarizationHealth {
    /// `ok` — resummarize refuses to write anything else (failed runs never
    /// mutate the file, so a failure can never appear here).
    pub status: String,
    /// Engine/model hint used (e.g. `agent:claude`, `apple-fm`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Template slug applied to this run, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Wall-clock of the summarize stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Machine-readable context (reserved; `None` for successful runs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// RFC3339 timestamp of the most recent resummarize run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RecordingHealth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_stem_active_ratio: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_stem_active_ratio: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_dominant_ratio: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capture_warnings: Vec<CaptureWarning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diarization_path: Option<DiarizationPath>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DiarizationPath {
    StemEnergy,
    Ml,
    MlBleedDegraded,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CaptureWarning {
    pub kind: crate::diarize::FailureKind,
    pub source: crate::diarize::CaptureSource,
    pub message: String,
    pub diagnostic_confidence: crate::diarize::DiagnosticConfidence,
}

impl From<crate::diarize::DegradedCapture> for RecordingHealth {
    fn from(reason: crate::diarize::DegradedCapture) -> Self {
        RecordingHealth::from_degraded_capture(reason, DiarizationPath::None)
    }
}

impl RecordingHealth {
    pub fn from_degraded_capture(
        reason: crate::diarize::DegradedCapture,
        diarization_path: DiarizationPath,
    ) -> Self {
        let message = match &reason.failure_kind {
            crate::diarize::FailureKind::Silent => {
                if diarization_path == DiarizationPath::MlBleedDegraded {
                    "System audio was silent during capture; speaker labels were recovered from degraded mic bleed with low confidence.".to_string()
                } else {
                    "System audio was silent during capture; transcript was left unlabeled."
                        .to_string()
                }
            }
            crate::diarize::FailureKind::Sparse => {
                if diarization_path == DiarizationPath::MlBleedDegraded {
                    "System audio did not contain sustained transcript-aligned remote speech; speaker labels were recovered from degraded mic bleed with low confidence.".to_string()
                } else {
                    "System audio did not contain sustained transcript-aligned remote speech; transcript was left unlabeled.".to_string()
                }
            }
            _ => {
                if diarization_path == DiarizationPath::MlBleedDegraded {
                    "Capture health degraded diarization; speaker labels were recovered from degraded mic bleed with low confidence.".to_string()
                } else {
                    "Capture health degraded diarization; transcript was left unlabeled."
                        .to_string()
                }
            }
        };

        RecordingHealth {
            voice_stem_active_ratio: reason.voice_active_ratio,
            system_stem_active_ratio: reason.system_active_ratio,
            system_dominant_ratio: None,
            capture_warnings: vec![CaptureWarning {
                kind: reason.failure_kind,
                source: reason.capture_source,
                message,
                diagnostic_confidence: reason.diagnostic_confidence,
            }],
            diarization_path: Some(diarization_path),
        }
    }
}

/// Frontmatter for a meeting/memo markdown file.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Frontmatter {
    pub title: String,
    pub r#type: ContentType,
    #[serde(deserialize_with = "deserialize_local_datetime")]
    pub date: DateTime<Local>,
    #[serde(default = "default_duration")]
    pub duration: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<OutputStatus>,
    /// Per-step failure context when [`OutputStatus::Degraded`] applies.
    /// Skipped from serialization when empty so successful runs do not
    /// emit extra frontmatter noise. See issue #243.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processing_warnings: Vec<ProcessingWarning>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attendees_raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_event: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<String>,
    #[serde(default, skip_serializing_if = "EntityLinks::is_empty")]
    pub entities: EntityLinks,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default, deserialize_with = "deserialize_optional_local_datetime")]
    pub captured_at: Option<DateTime<Local>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub action_items: Vec<ActionItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Decision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intents: Vec<Intent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recorded_by: Option<String>,
    /// Capture mode for the artifact. Absent means a normal captured meeting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<CapturePolicy>,
    /// Sensitivity designation. Absent means the normal sensitivity policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitivity: Option<Sensitivity>,
    /// Debrief completion state for no-capture meetings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debrief: Option<DebriefStatus>,
    /// How consent to capture was obtained, if attested.
    ///
    /// Privacy metadata only, not a determination about requirements. See
    /// [`crate::config::ConsentConfig`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<ConsentBasis>,
    /// The exact disclosure the user gave or used, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_notice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaker_map: Vec<crate::diarize::SpeakerAttribution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub name_corrections: Vec<crate::name_correction::NameCorrection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_health: Option<RecordingHealth>,
    /// Health of the Level-1 speaker-naming step. Lets a meeting be greppable for
    /// "naming failed / incomplete" instead of the failure living only in the JSON
    /// log (#384). `None` for meetings processed before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_mapping: Option<SpeakerMappingHealth>,
    /// Health of the most recent re-run of the AI pass (`minutes resummarize`).
    /// `None` for artifacts that have only been through the original pipeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summarization: Option<SummarizationHealth>,
    /// Slug of the template applied to this recording, if any.
    /// Recorded so a Phase 2 reprocessor knows which template produced the
    /// summary. `None` means no template was passed (legacy / default flow).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Diagnostic string from the transcription filter pipeline.
    /// Not serialized to YAML — only used for the NoSpeech hint in rendered markdown.
    #[serde(skip)]
    pub filter_diagnosis: Option<String>,
}

fn default_duration() -> String {
    "0s".into()
}

fn deserialize_local_datetime<'de, D>(deserializer: D) -> Result<DateTime<Local>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(LocalDateTimeVisitor)
}

fn deserialize_optional_local_datetime<'de, D>(
    deserializer: D,
) -> Result<Option<DateTime<Local>>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    value
        .as_deref()
        .map(parse_frontmatter_local_datetime)
        .transpose()
        .map_err(de::Error::custom)
}

struct LocalDateTimeVisitor;

impl Visitor<'_> for LocalDateTimeVisitor {
    type Value = DateTime<Local>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an RFC3339 timestamp, local timestamp, or YYYY-MM-DD date")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        parse_frontmatter_local_datetime(value).map_err(E::custom)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(&value)
    }
}

fn parse_frontmatter_local_datetime(raw: &str) -> Result<DateTime<Local>, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("empty date".into());
    }

    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&Local));
    }

    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
            return local_datetime_from_naive(naive);
        }
    }

    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        if let Some(naive) = date.and_hms_opt(0, 0, 0) {
            return local_datetime_from_naive(naive);
        }
    }

    Err(format!(
        "invalid date `{}` (expected YYYY-MM-DD, local timestamp, or RFC3339 timestamp)",
        value
    ))
}

fn local_datetime_from_naive(naive: NaiveDateTime) -> Result<DateTime<Local>, String> {
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => Ok(dt),
        LocalResult::None => Err(format!("local datetime `{}` does not exist", naive)),
    }
}

impl Frontmatter {
    /// Return structured attendees plus any names parsed from legacy raw imports.
    pub fn normalized_attendees(&self) -> Vec<String> {
        let mut attendees = self.attendees.clone();
        if let Some(raw) = &self.attendees_raw {
            for attendee in parse_attendees_raw(raw) {
                if !attendees
                    .iter()
                    .any(|existing| attendee_key(existing) == attendee_key(&attendee))
                {
                    attendees.push(attendee);
                }
            }
        }
        attendees
    }
}

fn attendee_key(value: &str) -> String {
    value.trim().to_lowercase()
}

/// Parse legacy Granola-style attendee strings like
/// `Alice Smith (alice@example.com), bob@example.com`.
pub fn parse_attendees_raw(raw: &str) -> Vec<String> {
    raw.split(',')
        .filter_map(|token| {
            let trimmed = token.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
                return None;
            }

            if let Some(name) = trimmed
                .strip_suffix(')')
                .and_then(|value| value.rsplit_once('(').map(|(name, _)| name.trim()))
                .filter(|name| !name.is_empty())
            {
                return Some(name.to_string());
            }

            if let Some(name) = trimmed
                .strip_suffix('>')
                .and_then(|value| value.rsplit_once('<').map(|(name, _)| name.trim()))
                .filter(|name| !name.is_empty())
            {
                return Some(name.to_string());
            }

            Some(trimmed.to_string())
        })
        .fold(Vec::new(), |mut acc, attendee| {
            if !acc
                .iter()
                .any(|existing| attendee_key(existing) == attendee_key(&attendee))
            {
                acc.push(attendee);
            }
            acc
        })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct EntityLinks {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<EntityRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<EntityRef>,
}

impl EntityLinks {
    pub fn is_empty(&self) -> bool {
        self.people.is_empty() && self.projects.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EntityRef {
    pub slug: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// A structured action item extracted from a meeting.
/// Queryable via MCP tools: filter by assignee, status, due date.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ActionItem {
    pub assignee: String,
    pub task: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    pub status: String, // "open" or "done"
}

/// A structured decision extracted from a meeting.
/// Queryable via MCP tools: search across all meetings for decision history.
///
/// Frontmatter v2 fields (optional, backward compatible):
/// - `authority`: "high" | "medium" | "low" — the decision's weight. A CEO
///   commitment is high; a drive-by aside is low. Consumers can use this to
///   rank conflicting decisions or surface the authoritative one.
/// - `supersedes`: free-text reference to the earlier decision this one
///   replaces. When set, the consistency report treats the topic conflict as
///   a documented supersession rather than an unresolved contradiction.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Decision {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum IntentKind {
    ActionItem,
    Decision,
    OpenQuestion,
    Commitment,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Intent {
    pub kind: IntentKind,
    pub what: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_date: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Private,
    Team,
}

/// Result of writing a meeting/memo to disk.
#[derive(Debug, Clone, Serialize)]
pub struct WriteResult {
    pub path: PathBuf,
    pub title: String,
    pub word_count: usize,
    pub content_type: ContentType,
}

fn render_markdown(
    frontmatter: &Frontmatter,
    transcript: &str,
    summary: Option<&str>,
    user_notes: Option<&str>,
    retry_audio_path: &Path,
) -> Result<String, MarkdownError> {
    let yaml = serde_yaml::to_string(frontmatter)
        .map_err(|e| MarkdownError::SerializationError(e.to_string()))?;

    let mut content = format!("---\n{}---\n\n", yaml);

    if let Some(summary_text) = summary {
        content.push_str("## Summary\n\n");
        content.push_str(summary_text);
        content.push_str("\n\n");
    }

    if frontmatter.status == Some(OutputStatus::NoSpeech) {
        content.push_str("*No speech detected in this recording.*\n\n");
        if let Some(diagnosis) = &frontmatter.filter_diagnosis {
            content.push_str(&format!("**Diagnosis**: {}\n\n", diagnosis));
        }
        content.push_str(&format!(
            "**Retry audio**: `{}`\n\n",
            retry_audio_path.display()
        ));
        content.push_str(&format!(
            "To retry after adjusting your transcription settings:\n`minutes process {}`\n\n",
            retry_audio_path.display()
        ));
    }

    if let Some(notes) = user_notes {
        content.push_str("## Notes\n\n");
        for line in notes.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                content.push_str(&format!("- {}\n", trimmed));
            }
        }
        content.push('\n');
    }

    content.push_str("## Transcript\n\n");
    content.push_str(transcript);
    content.push('\n');

    Ok(content)
}

/// Write a meeting/memo to markdown with YAML frontmatter.
pub fn write(
    frontmatter: &Frontmatter,
    transcript: &str,
    summary: Option<&str>,
    user_notes: Option<&str>,
    config: &Config,
) -> Result<WriteResult, MarkdownError> {
    write_with_retry_path(frontmatter, transcript, summary, user_notes, None, config)
}

/// Write markdown while pointing no-speech retry guidance at the original audio path.
pub fn write_with_retry_path(
    frontmatter: &Frontmatter,
    transcript: &str,
    summary: Option<&str>,
    user_notes: Option<&str>,
    retry_audio_path: Option<&Path>,
    config: &Config,
) -> Result<WriteResult, MarkdownError> {
    let output_dir = match frontmatter.r#type {
        ContentType::Memo => config.output_dir.join("memos"),
        ContentType::Meeting => config.output_dir.clone(),
        ContentType::Dictation => config.output_dir.join("dictations"),
    };

    // Ensure output directory exists
    fs::create_dir_all(&output_dir)
        .map_err(|e| MarkdownError::OutputDirError(format!("{}: {}", output_dir.display(), e)))?;

    // Generate filename slug
    let slug = generate_slug(
        &frontmatter.title,
        frontmatter.date,
        frontmatter.recorded_by.as_deref(),
    );
    let path = resolve_collision(&output_dir, &slug);
    let content = render_markdown(
        frontmatter,
        transcript,
        summary,
        user_notes,
        retry_audio_path.unwrap_or(&path),
    )?;

    // Write file with appropriate permissions
    fs::write(&path, &content)?;
    let mode = match frontmatter.visibility {
        Some(Visibility::Team) => 0o640,
        _ => 0o600,
    };
    set_permissions(&path, mode)?;

    let word_count = transcript.split_whitespace().count();
    tracing::info!(
        path = %path.display(),
        words = word_count,
        content_type = ?frontmatter.r#type,
        "wrote meeting markdown"
    );

    Ok(WriteResult {
        path,
        title: frontmatter.title.clone(),
        word_count,
        content_type: frontmatter.r#type,
    })
}

pub fn rewrite(
    path: &Path,
    frontmatter: &Frontmatter,
    transcript: &str,
    summary: Option<&str>,
    user_notes: Option<&str>,
) -> Result<WriteResult, MarkdownError> {
    rewrite_with_retry_path(path, frontmatter, transcript, summary, user_notes, None)
}

pub fn rewrite_with_retry_path(
    path: &Path,
    frontmatter: &Frontmatter,
    transcript: &str,
    summary: Option<&str>,
    user_notes: Option<&str>,
    retry_audio_path: Option<&Path>,
) -> Result<WriteResult, MarkdownError> {
    let content = render_markdown(
        frontmatter,
        transcript,
        summary,
        user_notes,
        retry_audio_path.unwrap_or(path),
    )?;
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, content)?;
    let mode = match frontmatter.visibility {
        Some(Visibility::Team) => 0o640,
        _ => 0o600,
    };
    set_permissions(&tmp, mode)?;
    fs::rename(&tmp, path)?;

    let word_count = transcript.split_whitespace().count();
    Ok(WriteResult {
        path: path.to_path_buf(),
        title: frontmatter.title.clone(),
        word_count,
        content_type: frontmatter.r#type,
    })
}

/// Rename an existing meeting markdown file in place.
///
/// This is the safe path used by the command palette's
/// `RenameCurrentMeeting` action. It is **fail-closed**: any
/// frontmatter that is not boring-and-plain refuses the rename
/// instead of attempting a string replace that could corrupt YAML
/// anchors, folded scalars, literal blocks, or aliases.
///
/// Steps (described in `docs/plans/command-palette-slice-2.md` D8):
/// 1. Read the file.
/// 2. Split frontmatter via `split_frontmatter`. Empty frontmatter
///    means "not a Minutes meeting" → refuse.
/// 3. Parse the frontmatter via `serde_yaml::from_str::<Frontmatter>`.
///    A failure means the file is malformed → refuse.
/// 4. Re-parse the same frontmatter as `serde_yaml::Value` to check
///    that the `title` field is a **plain string scalar**. If it is a
///    folded scalar (`title: >`), literal block (`title: |`), tagged
///    scalar, mapping, sequence, or carries an anchor/alias, refuse.
///    These are real YAML constructs that the line-replace strategy
///    cannot handle safely.
/// 5. Find the exact line matching `^title:\s*<original-quoted-or-bare>$`
///    in the frontmatter text. If zero matches or more than one,
///    refuse.
/// 6. Replace that single line with `title: "<escaped-new-title>"`.
/// 7. Write the result to a tmp sibling and rename atomically over
///    the original path.
/// 8. **Parse the written file** to confirm the resulting frontmatter
///    is still valid YAML. If parse fails, restore the backup that
///    was written before the change and return an error.
/// 9. If the new title produces a different slug, rename the file
///    using `resolve_collision`. Returns the final path.
///
/// Errors are returned as `MarkdownError::RenameRefused` for the
/// safety-policy refusals and as `MarkdownError::Io` for filesystem
/// failures.
pub fn rename_meeting(path: &Path, new_title: &str) -> Result<PathBuf, MarkdownError> {
    let new_title = new_title.trim();
    if new_title.is_empty() {
        return Err(MarkdownError::RenameRefused("new title is empty".into()));
    }
    if new_title.contains('\n') || new_title.contains('\r') {
        return Err(MarkdownError::RenameRefused(
            "new title contains newlines".into(),
        ));
    }

    let original = fs::read_to_string(path)?;
    let (fm_str, _body) = split_frontmatter(&original);
    if fm_str.is_empty() {
        return Err(MarkdownError::RenameRefused(
            "file has no YAML frontmatter — not a Minutes meeting".into(),
        ));
    }

    // Step 3: parse via serde_yaml::Frontmatter to confirm the file is
    // structurally a meeting.
    let parsed: Frontmatter = serde_yaml::from_str(fm_str).map_err(|e| {
        MarkdownError::RenameRefused(format!("frontmatter does not parse as YAML: {}", e))
    })?;

    let original_title = parsed.title.trim().to_string();
    if original_title.is_empty() {
        return Err(MarkdownError::RenameRefused(
            "current frontmatter title is empty".into(),
        ));
    }

    // Step 4: confirm the on-disk title is a plain-string scalar with
    // no anchors/aliases/tags/folded/literal blocks. We do this by
    // parsing the frontmatter as a generic serde_yaml::Value and
    // walking the title node.
    let value: serde_yaml::Value = serde_yaml::from_str(fm_str).map_err(|e| {
        MarkdownError::RenameRefused(format!("frontmatter generic parse failed: {}", e))
    })?;
    let title_value = value
        .get("title")
        .ok_or_else(|| MarkdownError::RenameRefused("no `title` field in frontmatter".into()))?;
    if !title_value.is_string() {
        return Err(MarkdownError::RenameRefused(
            "title is not a plain scalar — rename via your text editor".into(),
        ));
    }

    // No-op rename: title unchanged.
    if original_title == new_title {
        return Ok(path.to_path_buf());
    }

    // Step 5: find the EXACT title line in fm_str. We refuse to touch
    // files with `title:` appearing on more than one line in the
    // frontmatter — that's a sign of an unusual file we don't want to
    // mutate blindly.
    let title_lines: Vec<(usize, &str)> = fm_str
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let trimmed = line.trim_start();
            trimmed.starts_with("title:") && !trimmed.starts_with("title::")
        })
        .collect();
    if title_lines.is_empty() {
        return Err(MarkdownError::RenameRefused(
            "could not locate `title:` line in frontmatter".into(),
        ));
    }
    if title_lines.len() > 1 {
        return Err(MarkdownError::RenameRefused(
            "multiple `title:` lines in frontmatter — refusing to rename".into(),
        ));
    }
    let (title_line_index, original_title_line) = title_lines[0];

    // Reject anchors / folded / literal block markers on the title line.
    let after_colon = original_title_line
        .trim_start()
        .trim_start_matches("title:")
        .trim_start();
    if after_colon.starts_with('&') || after_colon.starts_with('*') || after_colon.starts_with('!')
    {
        return Err(MarkdownError::RenameRefused(
            "title line uses YAML anchor/alias/tag — rename via your text editor".into(),
        ));
    }
    // Folded scalar `>` and literal block `|` markers (with optional
    // chomping indicator) on the title line mean the value spans
    // multiple lines, which the line replace cannot handle safely.
    let leading_marker = after_colon.chars().next();
    if matches!(leading_marker, Some('>') | Some('|')) {
        return Err(MarkdownError::RenameRefused(
            "title is a folded or literal block scalar — rename via your text editor".into(),
        ));
    }

    // Step 6: rebuild the frontmatter with the title line replaced.
    let new_title_line = format!("title: {}", yaml_quote(new_title));
    let mut new_fm_lines: Vec<String> = fm_str.lines().map(String::from).collect();
    new_fm_lines[title_line_index] = new_title_line;
    let new_fm_text = new_fm_lines.join("\n");

    // Reassemble the file. `split_frontmatter` strips the leading
    // `---\n` and trailing `\n---\n`; we have to put them back.
    // Find the body slice the same way `split_frontmatter` does, then
    // splice in the new frontmatter text.
    let body_start = original
        .find("\n---")
        .map(|idx| {
            // Move past the trailing `\n---` and the next newline.
            let after = idx + 4;
            original[after..]
                .find('\n')
                .map(|n| after + n + 1)
                .unwrap_or(original.len())
        })
        .unwrap_or(original.len());
    let new_content = format!("---\n{}\n---\n{}", new_fm_text, &original[body_start..]);

    // Step 7: atomic write through a tmp sibling. Preserve the
    // ORIGINAL file's permissions instead of forcing 0o600 — the
    // user may have chmod'd the file to 0o644 for Obsidian sync, a
    // local webserver preview, or any other workflow that needs
    // group-readable. Forcing 0o600 on every rename would silently
    // break those setups (claude pass 3 P3).
    let tmp_path = path.with_extension("md.rename.tmp");
    fs::write(&tmp_path, &new_content)?;
    let original_mode = preserved_file_mode(path);
    if let Err(e) = set_permissions(&tmp_path, original_mode) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    // Step 8: parse-after-write validation. Read back what we just
    // wrote and confirm the frontmatter still parses. If it doesn't,
    // delete the tmp and refuse the rename — the original file is
    // unchanged.
    let written = match fs::read_to_string(&tmp_path) {
        Ok(s) => s,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(MarkdownError::Io(e));
        }
    };
    let (written_fm, _) = split_frontmatter(&written);
    if let Err(e) = serde_yaml::from_str::<Frontmatter>(written_fm) {
        let _ = fs::remove_file(&tmp_path);
        return Err(MarkdownError::RenameRefused(format!(
            "post-write validation failed; original file unchanged: {}",
            e
        )));
    }

    // Commit: atomically replace the original file with the new
    // content. After this point the meeting markdown reflects the new
    // title; only the file *name* may still need to change.
    fs::rename(&tmp_path, path)?;

    // Step 9: rename the file itself if the slug changes. We use the
    // parsed frontmatter (parsed before the title edit) for the date
    // and recorded_by fields — the title edit doesn't touch those.
    let new_slug = generate_slug(new_title, parsed.date, parsed.recorded_by.as_deref());
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let final_path = if path.file_name().and_then(|n| n.to_str()) == Some(new_slug.as_str()) {
        path.to_path_buf()
    } else {
        let target = resolve_collision(parent, &new_slug);
        fs::rename(path, &target)?;
        target
    };

    Ok(final_path)
}

/// Quote a string as a YAML double-quoted scalar. Escapes the
/// characters that double-quoted scalars require: backslash, double
/// quote, and the C0 control set. Used by `rename_meeting` to write a
/// safe `title:` line.
fn yaml_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                write!(out, "\\x{:02x}", c as u32).expect("write to string");
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Generate a URL-safe filename slug from title, date, and optional recorder name.
fn generate_slug(title: &str, date: DateTime<Local>, recorded_by: Option<&str>) -> String {
    let date_prefix = date.format("%Y-%m-%d").to_string();
    let title_slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    let name_suffix = recorded_by
        .map(|name| {
            let short: String = name
                .split_whitespace()
                .next()
                .unwrap_or(name)
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .take(10)
                .collect();
            if short.is_empty() {
                String::new()
            } else {
                format!("-{}", short)
            }
        })
        .unwrap_or_default();

    let slug = if title_slug.is_empty() {
        format!("{}-untitled{}", date_prefix, name_suffix)
    } else {
        // Truncate long titles
        let truncated: String = title_slug.chars().take(60).collect();
        format!("{}-{}{}", date_prefix, truncated, name_suffix)
    };

    format!("{}.md", slug)
}

/// Resolve filename collisions by appending -2, -3, etc.
fn resolve_collision(dir: &Path, filename: &str) -> PathBuf {
    let path = dir.join(filename);
    if !path.exists() {
        return path;
    }

    let stem = filename.trim_end_matches(".md");
    for i in 2..=999 {
        let candidate = dir.join(format!("{}-{}.md", stem, i));
        if !candidate.exists() {
            return candidate;
        }
    }

    // Fallback: use timestamp suffix
    let ts = chrono::Local::now().timestamp();
    dir.join(format!("{}-{}.md", stem, ts))
}

/// Surgically update only a meeting's frontmatter, preserving the body
/// (summary / notes / transcript) byte-for-byte.
///
/// Unlike [`rewrite`], which regenerates the whole file from passed-in sections,
/// this parses the existing frontmatter, applies `update`, re-serializes only the
/// YAML block, and splices it back in front of the original body. Fail-closed:
/// refuses files without parseable frontmatter, validates the result parses
/// before swapping, and writes atomically via a tmp sibling while preserving the
/// original file mode (#384).
///
/// This is a generic frontmatter updater: it does NOT enforce `type: meeting`.
/// Callers that should only touch meetings must check `frontmatter.r#type`
/// themselves (see `cmd_redo_speaker_mapping`).
pub fn update_frontmatter<F>(path: &Path, update: F) -> Result<(), MarkdownError>
where
    F: FnOnce(&mut Frontmatter),
{
    let original = fs::read_to_string(path)?;
    // Take the body slice straight from `split_frontmatter` so we share its exact
    // boundary semantics. `body` is a suffix slice of `original`, so splicing it
    // back verbatim preserves every body byte even for oddly-fenced inputs (no
    // second, subtly-different offset computation that could drop bytes; #384).
    let (fm_str, body) = split_frontmatter(&original);
    if fm_str.is_empty() {
        return Err(MarkdownError::RenameRefused(
            "not a meeting file (no frontmatter)".into(),
        ));
    }
    let mut frontmatter: Frontmatter = serde_yaml::from_str(fm_str)
        .map_err(|e| MarkdownError::RenameRefused(format!("frontmatter does not parse: {e}")))?;

    update(&mut frontmatter);

    let serialized = serde_yaml::to_string(&frontmatter)
        .map_err(|e| MarkdownError::SerializationError(e.to_string()))?;
    let new_fm_text = serialized.trim_end_matches('\n');

    let new_content = format!("---\n{}\n---\n{}", new_fm_text, body);

    // Atomic write through a tmp sibling, preserving the original file mode.
    let tmp_path = path.with_extension("md.fmupdate.tmp");
    if let Err(e) = fs::write(&tmp_path, &new_content) {
        let _ = fs::remove_file(&tmp_path);
        return Err(MarkdownError::Io(e));
    }
    let original_mode = preserved_file_mode(path);
    if let Err(e) = set_permissions(&tmp_path, original_mode) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    // Parse-after-write validation: confirm the result still parses before swapping.
    match fs::read_to_string(&tmp_path) {
        Ok(written) => {
            let (wfm, _) = split_frontmatter(&written);
            if wfm.is_empty() || serde_yaml::from_str::<Frontmatter>(wfm).is_err() {
                let _ = fs::remove_file(&tmp_path);
                return Err(MarkdownError::SerializationError(
                    "post-write frontmatter validation failed".into(),
                ));
            }
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(MarkdownError::Io(e));
        }
    }

    if let Err(e) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(MarkdownError::Io(e));
    }
    Ok(())
}

/// Atomically replace an artifact's full content, preserving its file mode
/// (modes are not uniformly 0600 — `Visibility::Team` files are 0640).
///
/// Same write discipline as [`update_frontmatter`] (#384): tmp-sibling write,
/// mode copy, parse-after-write validation of the frontmatter, then rename.
/// On any failure the original file is untouched.
///
/// When `expected_current` is given, the target file is re-read and compared
/// twice: once before the backup copy and once **immediately before the
/// rename**. A concurrent editor save that lands during the backup copy still
/// aborts the swap with
/// [`MarkdownError::ConcurrentModification`] with the smallest
/// copy-free check-to-rename window a plain filesystem allows (it cannot be
/// zero without OS-level file locking).
///
/// When `backup_to` is given, the current file is copied there (via
/// [`fs::copy`], which carries the source mode) **after** the guard passes
/// and before the rename — so a conflicted or failed run never disturbs a
/// backup left by an earlier successful one.
pub fn atomic_rewrite_preserving_mode_guarded(
    path: &Path,
    new_content: &str,
    expected_current: Option<&str>,
    backup_to: Option<&Path>,
) -> Result<(), MarkdownError> {
    // Unique tmp name (pid + clock nanos): two concurrent runs must not
    // stage into — or rename — each other's file.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp_path = path.with_extension(format!(
        "md.resummarize.{}.{}.tmp",
        std::process::id(),
        nanos
    ));
    // Create with the artifact's final mode from the first byte. OpenOptions'
    // mode is umask-masked (so it may under-permission, never over-permission),
    // then the exact mode is enforced on the open handle before any content
    // byte is written.
    let original_mode = preserved_file_mode(path);
    let mut open_opts = fs::OpenOptions::new();
    open_opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_opts.mode(original_mode);
    }
    #[cfg(not(unix))]
    let _ = original_mode;
    let write_result = open_opts.open(&tmp_path).and_then(|mut file| {
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(original_mode))?;
        use std::io::Write;
        file.write_all(new_content.as_bytes())
    });
    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(MarkdownError::Io(e));
    }

    match fs::read_to_string(&tmp_path) {
        Ok(written) => {
            let (wfm, _) = split_frontmatter(&written);
            if wfm.is_empty() || serde_yaml::from_str::<Frontmatter>(wfm).is_err() {
                let _ = fs::remove_file(&tmp_path);
                return Err(MarkdownError::SerializationError(
                    "post-write frontmatter validation failed".into(),
                ));
            }
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(MarkdownError::Io(e));
        }
    }

    let compare_current = || -> Result<(), MarkdownError> {
        if let Some(expected) = expected_current {
            match fs::read_to_string(path) {
                Ok(current) if current != expected => {
                    return Err(MarkdownError::ConcurrentModification);
                }
                Ok(_) => {}
                Err(e) => return Err(MarkdownError::Io(e)),
            }
        }
        Ok(())
    };

    // Cheap early guard: do not create a backup for a stale snapshot.
    if let Err(e) = compare_current() {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    // Backup only once the guard has passed: a conflicted run must never
    // clobber the backup of an earlier successful run. `fs::copy` carries
    // the source file's mode, so the backup is never umask-readable.
    let mut created_backup = None;
    if let Some(backup) = backup_to {
        if let Err(e) = fs::copy(path, backup) {
            let _ = fs::remove_file(&tmp_path);
            return Err(MarkdownError::Io(e));
        }
        created_backup = Some(backup);
    }

    // Guard compare again immediately before the swap. If the file changed
    // during backup creation, that backup belongs only to this failed run.
    if let Err(e) = compare_current() {
        let _ = fs::remove_file(&tmp_path);
        if let Some(backup) = created_backup {
            let _ = fs::remove_file(backup);
        }
        return Err(e);
    }

    if let Err(e) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(MarkdownError::Io(e));
    }
    Ok(())
}

/// Set file permissions to the given mode (Unix only; no-op on Windows).
fn set_permissions(path: &Path, _mode: u32) -> Result<(), MarkdownError> {
    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(_mode);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

/// Read the existing file's mode bits so a rewrite can preserve
/// permissions the user may have set deliberately. Returns `0o600`
/// (the Minutes default) on Windows or if the metadata read fails.
/// Used by `rename_meeting` to avoid clobbering user-chosen modes.
fn preserved_file_mode(_path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(_path) {
            // Mask off the file-type bits, keep only the permission
            // bits (rwxrwxrwx + setuid/setgid/sticky).
            return meta.permissions().mode() & 0o7777;
        }
    }
    0o600
}

// ── Frontmatter parsing utilities (shared across modules) ────

/// Split markdown content into frontmatter string and body string.
/// Returns `("", content)` if no frontmatter is found.
pub fn split_frontmatter(content: &str) -> (&str, &str) {
    if !content.starts_with("---") {
        return ("", content);
    }

    if let Some(end) = content[3..].find("\n---") {
        let fm_end = end + 3;
        let body_start = fm_end + 4; // skip \n---
        let body_start = content[body_start..]
            .find('\n')
            .map(|i| body_start + i + 1)
            .unwrap_or(body_start);
        (&content[3..fm_end], &content[body_start..])
    } else {
        ("", content)
    }
}

/// Extract a simple `key: value` field from YAML frontmatter text.
/// Handles quoted values. Returns None if key not found.
pub fn extract_field(frontmatter: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix(&prefix) {
            return Some(
                value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            );
        }
    }
    None
}

/// Byte-offset bounds of one `## <heading>` section inside a markdown body
/// (the body as returned by [`split_frontmatter`], not the full file).
///
/// All offsets lie on line boundaries of the original text, so a caller can
/// splice `body[..content_start] + new_content + body[end..]` without
/// disturbing a single byte outside the section — including CRLF line
/// endings elsewhere in the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionRange {
    /// Byte offset of the start of the `## ` heading line.
    pub heading_start: usize,
    /// Byte offset of the first content byte after the heading line
    /// (equal to `end` when the section is empty).
    pub content_start: usize,
    /// Byte offset one past the section content: the start of the next H2
    /// heading line, or `body.len()` when the section runs to the end.
    pub end: usize,
}

/// Scan `body` for H2 (`## `) heading lines outside fenced code blocks,
/// returning `(line_start, after_line, heading_text)` per heading.
///
/// Fence tracking matches CommonMark's relevant closing rules: the matching
/// fence character must run for at least the opener's length, and the rest of
/// a closing line must be whitespace-only. Different, shorter, or
/// info-string-bearing fence lines are content. Headings must start at column
/// 0.
fn h2_headings(body: &str) -> Vec<(usize, usize, &str)> {
    let mut fence: Option<(char, usize)> = None;
    let mut headings = Vec::new();
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next().and_then(|marker| {
            (matches!(marker, '`' | '~')).then(|| {
                let run_len = trimmed.chars().take_while(|c| *c == marker).count();
                (marker, run_len)
            })
        });
        if let Some((marker, run_len)) = marker.filter(|(_, run_len)| *run_len >= 3) {
            match fence {
                None => fence = Some((marker, run_len)),
                Some((open_marker, open_len))
                    if open_marker == marker
                        && run_len >= open_len
                        && trimmed[run_len..].trim().is_empty() =>
                {
                    fence = None;
                }
                Some(_) => {} // non-closing marker inside a fence: content
            }
            continue;
        }
        if fence.is_none() {
            if let Some(rest) = line.strip_prefix("## ") {
                headings.push((start, offset, rest.trim()));
            }
        }
    }
    headings
}

/// Find every `## <name>` section in `body` (fence-aware; the heading text
/// must equal `name` after trimming surrounding whitespace).
///
/// A section ends at the next H2 heading of *any* name outside a fence, or at
/// the end of the body. Most callers want [`find_unique_section`]; this
/// exists so previews can report *where* duplicate headings live.
pub fn find_sections(body: &str, name: &str) -> Vec<SectionRange> {
    let headings = h2_headings(body);
    let mut sections = Vec::new();
    for (i, (heading_start, content_start, text)) in headings.iter().enumerate() {
        if *text == name {
            let end = headings.get(i + 1).map_or(body.len(), |next| next.0);
            sections.push(SectionRange {
                heading_start: *heading_start,
                content_start: *content_start,
                end,
            });
        }
    }
    sections
}

/// Find the single `## <name>` section, failing closed on ambiguity.
///
/// Returns `Ok(None)` when the section is absent, and
/// [`MarkdownError::AmbiguousSection`] when more than one heading matches — a
/// splice against an ambiguous document could rewrite the wrong section, so
/// writers must treat the error as a hard stop and surface it to the user.
pub fn find_unique_section(body: &str, name: &str) -> Result<Option<SectionRange>, MarkdownError> {
    let mut sections = find_sections(body, name);
    match sections.len() {
        0 => Ok(None),
        1 => Ok(Some(sections.remove(0))),
        count => Err(MarkdownError::AmbiguousSection {
            name: name.to_string(),
            count,
        }),
    }
}

/// Extract a section's text content given its resolved range.
///
/// Line endings are normalized to `\n` and leading blank lines are dropped,
/// matching the historical CLI `transcript_section` behavior. Use the raw
/// `body[range.content_start..range.end]` slice when exact bytes matter.
pub fn section_text(body: &str, range: SectionRange) -> String {
    body[range.content_start..range.end]
        .lines()
        .collect::<Vec<_>>()
        .join("\n")
        .trim_start_matches('\n')
        .to_string()
}

/// Text of the first `## <name>` section, or `None` when the body has none.
///
/// Lenient variant for read-only callers: duplicate headings resolve to the
/// first occurrence, exactly as the CLI transcript extractor always did.
/// Anything that writes back must go through [`find_unique_section`] instead.
pub fn first_section_text(body: &str, name: &str) -> Option<String> {
    find_sections(body, name)
        .first()
        .map(|range| section_text(body, *range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_frontmatter() -> Frontmatter {
        Frontmatter {
            title: "Test Meeting".into(),
            r#type: ContentType::Meeting,
            date: Local::now(),
            duration: "5m 30s".into(),
            source: None,
            status: Some(OutputStatus::Complete),
            tags: vec![],
            attendees: vec![],
            attendees_raw: None,
            calendar_event: None,
            people: vec![],
            entities: EntityLinks::default(),
            device: None,
            captured_at: None,
            context: None,
            action_items: vec![],
            decisions: vec![],
            intents: vec![],
            recorded_by: None,
            capture: None,
            sensitivity: None,
            debrief: None,
            consent: None,
            consent_notice: None,
            visibility: None,
            speaker_map: vec![],
            name_corrections: Vec::new(),
            recording_health: None,
            speaker_mapping: None,
            summarization: None,
            processing_warnings: Vec::new(),
            template: None,
            filter_diagnosis: None,
        }
    }

    #[test]
    fn frontmatter_accepts_manual_date_only_values() {
        use chrono::Datelike;

        let input = "title: Test\ntype: meeting\ndate: 2024-05-14\n";
        let parsed: Frontmatter = serde_yaml::from_str(input).unwrap();

        assert_eq!(parsed.date.year(), 2024);
        assert_eq!(parsed.date.month(), 5);
        assert_eq!(parsed.date.day(), 14);
        assert_eq!(parsed.duration, "0s");
    }

    #[test]
    fn consent_basis_serializes_expected_strings() {
        assert_eq!(
            serde_yaml::to_string(&ConsentBasis::VerbalAllParties).unwrap(),
            "verbal_all_parties\n"
        );
        assert_eq!(
            ConsentBasis::RecordedDisclosed.as_str(),
            "recorded_disclosed"
        );
        assert_eq!(
            "na".parse::<ConsentBasis>().unwrap(),
            ConsentBasis::NotApplicable
        );
        assert!("mystery".parse::<ConsentBasis>().is_err());
    }

    #[test]
    fn frontmatter_consent_fields_are_optional_and_serialize_when_present() {
        let legacy: Frontmatter =
            serde_yaml::from_str("title: Test\ntype: meeting\ndate: 2026-06-04T10:00:00-07:00\n")
                .unwrap();
        assert_eq!(legacy.consent, None);
        assert_eq!(legacy.consent_notice, None);

        let mut fm = test_frontmatter();
        let without_consent = serde_yaml::to_string(&fm).unwrap();
        assert!(!without_consent.contains("consent:"));
        assert!(!without_consent.contains("consent_notice:"));

        fm.consent = Some(ConsentBasis::NoticeInInvite);
        fm.consent_notice = Some("Shared in the calendar invite.".into());
        let with_consent = serde_yaml::to_string(&fm).unwrap();
        assert!(with_consent.contains("consent: notice_in_invite"));
        assert!(with_consent.contains("consent_notice: Shared in the calendar invite."));
    }

    #[test]
    fn frontmatter_sensitive_fields_are_optional_and_serialize_when_present() {
        let legacy: Frontmatter =
            serde_yaml::from_str("title: Test\ntype: meeting\ndate: 2026-06-10T10:00:00-07:00\n")
                .unwrap();
        assert_eq!(legacy.capture, None);
        assert_eq!(legacy.sensitivity, None);
        assert_eq!(legacy.debrief, None);

        let mut fm = test_frontmatter();
        let without_sensitive = serde_yaml::to_string(&fm).unwrap();
        assert!(!without_sensitive.contains("capture:"));
        assert!(!without_sensitive.contains("sensitivity:"));
        assert!(!without_sensitive.contains("debrief:"));

        fm.capture = Some(CapturePolicy::None);
        fm.sensitivity = Some(Sensitivity::Restricted);
        fm.debrief = Some(DebriefStatus::Pending);
        let with_sensitive = serde_yaml::to_string(&fm).unwrap();
        assert!(with_sensitive.contains("capture: none"));
        assert!(with_sensitive.contains("sensitivity: restricted"));
        assert!(with_sensitive.contains("debrief: pending"));
    }

    #[test]
    fn frontmatter_accepts_local_timestamps_without_offset() {
        use chrono::{Datelike, Timelike};

        let input = "title: Test\ntype: meeting\ndate: \"2026-05-14T10:30:45\"\n";
        let parsed: Frontmatter = serde_yaml::from_str(input).unwrap();

        assert_eq!(parsed.date.year(), 2026);
        assert_eq!(parsed.date.month(), 5);
        assert_eq!(parsed.date.day(), 14);
        assert_eq!(parsed.date.hour(), 10);
        assert_eq!(parsed.date.minute(), 30);
        assert_eq!(parsed.date.second(), 45);
    }

    #[test]
    fn frontmatter_keeps_rfc3339_dates_working() {
        let input = "title: Test\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 5m\n";
        let parsed: Frontmatter = serde_yaml::from_str(input).unwrap();

        assert_eq!(
            parsed.date.with_timezone(&chrono::Utc).to_rfc3339(),
            "2026-03-17T19:00:00+00:00"
        );
        assert_eq!(parsed.duration, "5m");
    }

    #[test]
    fn generates_correct_slug() {
        let date = Local::now();
        let slug = generate_slug("Q2 Planning Discussion", date, None);
        let prefix = date.format("%Y-%m-%d").to_string();
        assert!(slug.starts_with(&prefix));
        assert!(slug.contains("q2-planning-discussion"));
        assert!(slug.ends_with(".md"));
    }

    #[test]
    fn generates_untitled_slug_for_empty_title() {
        let date = Local::now();
        let slug = generate_slug("", date, None);
        assert!(slug.contains("untitled"));
    }

    #[test]
    fn generates_slug_with_recorder_name() {
        let date = Local::now();
        let slug = generate_slug("Q2 Planning", date, Some("Mat Silverstein"));
        assert!(slug.contains("-mat"));
        assert!(slug.ends_with(".md"));
    }

    #[test]
    #[cfg(unix)]
    fn visibility_team_sets_0640_permissions() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let mut fm = test_frontmatter();
        fm.visibility = Some(Visibility::Team);
        let result = write(&fm, "Hello world", None, None, &config).unwrap();

        let metadata = fs::metadata(&result.path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "team visibility should set 0640 permissions");
    }

    #[test]
    fn frontmatter_with_recorded_by_roundtrips() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let mut fm = test_frontmatter();
        fm.recorded_by = Some("Mat".into());
        let result = write(&fm, "Transcript", None, None, &config).unwrap();
        let content = fs::read_to_string(&result.path).unwrap();
        assert!(content.contains("recorded_by: Mat"));
    }

    #[test]
    fn json_schema_generates_valid_schema() {
        let schema = schemars::schema_for!(Frontmatter);
        insta::assert_json_snapshot!(schema);
    }

    #[test]
    fn frontmatter_with_speaker_map_roundtrips() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let mut fm = test_frontmatter();
        fm.speaker_map = vec![crate::diarize::SpeakerAttribution {
            speaker_label: "SPEAKER_1".into(),
            name: "Mat".into(),
            confidence: crate::diarize::Confidence::Medium,
            source: crate::diarize::AttributionSource::Deterministic,
        }];
        let result = write(&fm, "transcript", None, None, &config).unwrap();
        let content = std::fs::read_to_string(&result.path).unwrap();
        assert!(
            content.contains("speaker_map:"),
            "speaker_map should appear in YAML"
        );
        assert!(content.contains("SPEAKER_1"), "speaker label should appear");
        assert!(content.contains("medium"), "confidence should be lowercase");
        assert!(
            content.contains("deterministic"),
            "source should be lowercase"
        );
    }

    #[test]
    fn recording_health_absent_roundtrips_as_omitted() {
        let input = "---\ntitle: Test Meeting\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 5m\nstatus: complete\n---\n\n## Transcript\n\nHello.\n";
        let (fm, body) = split_frontmatter(input);
        let frontmatter: Frontmatter = serde_yaml::from_str(fm).unwrap();
        assert!(frontmatter.recording_health.is_none());

        let yaml = serde_yaml::to_string(&frontmatter).unwrap();
        let output = format!("---\n{}---\n{}", yaml, body);

        assert!(!yaml.contains("recording_health"));
        assert_eq!(split_frontmatter(&output).1.as_bytes(), body.as_bytes());
    }

    #[test]
    fn recording_health_populated_roundtrips_structurally() {
        let input = "---\ntitle: Test Meeting\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 5m\nrecording_health:\n  voice_stem_active_ratio: 0.31\n  system_stem_active_ratio: 0.0\n  system_dominant_ratio: 0.12\n  capture_warnings:\n    - kind: silent\n      source: system\n      message: System audio was silent during capture.\n      diagnostic_confidence: inferred\n  diarization_path: ml-bleed-degraded\n---\n\n## Transcript\n\nHello.\n";
        let (fm, body) = split_frontmatter(input);
        let frontmatter: Frontmatter = serde_yaml::from_str(fm).unwrap();
        let health = frontmatter.recording_health.as_ref().unwrap();

        assert_eq!(health.voice_stem_active_ratio, Some(0.31));
        assert_eq!(health.system_stem_active_ratio, Some(0.0));
        assert_eq!(health.system_dominant_ratio, Some(0.12));
        assert_eq!(
            health.diarization_path,
            Some(DiarizationPath::MlBleedDegraded)
        );
        assert_eq!(health.capture_warnings.len(), 1);
        assert_eq!(
            health.capture_warnings[0].kind,
            crate::diarize::FailureKind::Silent
        );
        assert_eq!(
            health.capture_warnings[0].source,
            crate::diarize::CaptureSource::System
        );
        assert_eq!(
            health.capture_warnings[0].diagnostic_confidence,
            crate::diarize::DiagnosticConfidence::Inferred
        );

        let yaml = serde_yaml::to_string(&frontmatter).unwrap();
        let output = format!("---\n{}---\n{}", yaml, body);
        let reparsed: Frontmatter = serde_yaml::from_str(split_frontmatter(&output).0).unwrap();

        assert_eq!(reparsed.recording_health, frontmatter.recording_health);
        assert_eq!(split_frontmatter(&output).1.as_bytes(), body.as_bytes());
    }

    #[test]
    fn processing_warnings_roundtrip_through_yaml() {
        // Issue #243: degraded status + processing_warnings must serialize
        // to YAML and round-trip back through deserialization without loss.
        // Codex review of PR #249 v1 flagged missing end-to-end coverage.
        let input = "---\ntitle: Failed Summary Meeting\ntype: meeting\ndate: 2026-04-01T10:00:00-07:00\nduration: 45m\nstatus: degraded\nprocessing_warnings:\n  - step: summarize\n    reason: summarize_failed\n    timeout_secs: 300\n    message: Summarization via agent `opencode` produced no output.\n---\n\n## Transcript\n\nHello.\n";
        let (fm, body) = split_frontmatter(input);
        let frontmatter: Frontmatter = serde_yaml::from_str(fm).unwrap();

        assert_eq!(frontmatter.status, Some(OutputStatus::Degraded));
        assert_eq!(frontmatter.processing_warnings.len(), 1);
        let w = &frontmatter.processing_warnings[0];
        assert_eq!(w.step, "summarize");
        assert_eq!(w.reason, "summarize_failed");
        assert_eq!(w.timeout_secs, Some(300));
        assert!(w.message.as_ref().unwrap().contains("opencode"));

        // Round-trip the structure through serde -> string -> serde and
        // assert the deserialized form is identical.
        let yaml = serde_yaml::to_string(&frontmatter).unwrap();
        let output = format!("---\n{}---\n{}", yaml, body);
        let (reparsed_fm, reparsed_body) = split_frontmatter(&output);
        let reparsed: Frontmatter = serde_yaml::from_str(reparsed_fm).unwrap();
        assert_eq!(reparsed.status, frontmatter.status);
        assert_eq!(
            reparsed.processing_warnings,
            frontmatter.processing_warnings
        );
        assert_eq!(reparsed_body.as_bytes(), body.as_bytes());

        // Verify the serialized YAML actually contains the kebab-case
        // discriminant and the array (rather than skipping due to empty).
        assert!(yaml.contains("status: degraded"));
        assert!(yaml.contains("processing_warnings:"));
        assert!(yaml.contains("step: summarize"));
    }

    #[test]
    fn processing_warnings_omitted_when_empty() {
        // Empty processing_warnings must not appear in the serialized
        // YAML so successful runs don't pick up extra frontmatter noise.
        let input = "---\ntitle: Normal Meeting\ntype: meeting\ndate: 2026-04-01T10:00:00-07:00\nduration: 5m\nstatus: complete\n---\n\n## Transcript\n\nHello.\n";
        let (fm, _) = split_frontmatter(input);
        let frontmatter: Frontmatter = serde_yaml::from_str(fm).unwrap();
        assert!(frontmatter.processing_warnings.is_empty());

        let yaml = serde_yaml::to_string(&frontmatter).unwrap();
        assert!(!yaml.contains("processing_warnings"));
    }

    #[test]
    fn frontmatter_without_speaker_map_omits_field() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let fm = test_frontmatter(); // speaker_map: vec![]
        let result = write(&fm, "transcript", None, None, &config).unwrap();
        let content = std::fs::read_to_string(&result.path).unwrap();
        assert!(
            !content.contains("speaker_map"),
            "empty speaker_map should be omitted"
        );
    }

    #[test]
    fn resolves_filename_collisions() {
        let dir = TempDir::new().unwrap();
        let filename = "2026-03-17-test.md";

        // First file: no collision
        let path1 = resolve_collision(dir.path(), filename);
        assert_eq!(path1.file_name().unwrap(), filename);
        fs::write(&path1, "first").unwrap();

        // Second file: gets -2 suffix
        let path2 = resolve_collision(dir.path(), filename);
        assert_eq!(
            path2.file_name().unwrap().to_str().unwrap(),
            "2026-03-17-test-2.md"
        );
    }

    #[test]
    #[cfg(unix)]
    fn writes_markdown_with_correct_permissions() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let fm = test_frontmatter();
        let result = write(&fm, "Hello world transcript", None, None, &config).unwrap();

        assert!(result.path.exists());
        assert_eq!(result.word_count, 3);

        // Check permissions are 0600
        let metadata = fs::metadata(&result.path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "file should have 0600 permissions");
    }

    #[test]
    fn writes_memo_to_memos_subdirectory() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let fm = Frontmatter {
            r#type: ContentType::Memo,
            source: Some("voice-memos".into()),
            ..test_frontmatter()
        };

        let result = write(&fm, "Voice memo text", None, None, &config).unwrap();
        assert!(result.path.to_str().unwrap().contains("memos"));
    }

    #[test]
    fn frontmatter_serializes_intents_when_present() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let mut fm = test_frontmatter();
        fm.intents = vec![Intent {
            kind: IntentKind::Commitment,
            what: "Share revised pricing model".into(),
            who: Some("sarah".into()),
            status: "open".into(),
            by_date: Some("Tuesday".into()),
        }];

        let result = write(&fm, "Transcript", None, None, &config).unwrap();
        let content = fs::read_to_string(&result.path).unwrap();
        assert!(content.contains("intents:"));
        assert!(content.contains("kind: commitment"));
        assert!(content.contains("who: sarah"));
        assert!(content.contains("by_date: Tuesday"));
    }

    #[test]
    fn parses_attendees_raw_names_and_fallbacks() {
        let attendees = parse_attendees_raw(
            "Alice Smith (alice@example.com), bob@example.com, Carol Jones <carol@example.com>, Alice Smith (alice@example.com)",
        );

        assert_eq!(
            attendees,
            vec![
                "Alice Smith".to_string(),
                "bob@example.com".to_string(),
                "Carol Jones".to_string()
            ]
        );
    }

    #[test]
    fn normalized_attendees_merges_structured_and_raw_values() {
        let mut fm = test_frontmatter();
        fm.attendees = vec!["Alice Smith".into()];
        fm.attendees_raw =
            Some("Alice Smith (alice@example.com), Bob Brown (bob@example.com)".into());

        assert_eq!(
            fm.normalized_attendees(),
            vec!["Alice Smith".to_string(), "Bob Brown".to_string()]
        );
    }

    #[test]
    fn frontmatter_serializes_entities_when_present() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let mut fm = test_frontmatter();
        fm.people = vec!["Alex Chen".into()];
        fm.entities = EntityLinks {
            people: vec![EntityRef {
                slug: "sarah-chen".into(),
                label: "Alex Chen".into(),
                aliases: vec!["sarah".into()],
            }],
            projects: vec![EntityRef {
                slug: "pricing-review".into(),
                label: "Pricing Review".into(),
                aliases: vec!["pricing".into()],
            }],
        };

        let result = write(&fm, "Transcript", None, None, &config).unwrap();
        let content = fs::read_to_string(&result.path).unwrap();
        assert!(content.contains("entities:"));
        assert!(content.contains("slug: sarah-chen"));
        assert!(content.contains("label: Alex Chen"));
        assert!(content.contains("slug: pricing-review"));
    }

    #[test]
    fn frontmatter_serializes_tags_when_present() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let mut fm = test_frontmatter();
        fm.r#type = ContentType::Memo;
        fm.tags = vec![
            "memo".into(),
            "source:voice-memos".into(),
            "project:pricing-idea".into(),
        ];

        let result = write(&fm, "Transcript", None, None, &config).unwrap();
        let content = fs::read_to_string(&result.path).unwrap();
        assert!(content.contains("tags:"));
        assert!(content.contains("- memo"));
        assert!(content.contains("- source:voice-memos"));
        assert!(content.contains("- project:pricing-idea"));
    }

    // ── rename_meeting fail-closed tests ─────────────────────

    fn write_meeting(dir: &TempDir, slug: &str, frontmatter_yaml: &str, body: &str) -> PathBuf {
        let path = dir.path().join(slug);
        let content = format!("---\n{}---\n{}", frontmatter_yaml, body);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn rename_meeting_renames_plain_title_in_place() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-pricing-review.md",
            "title: \"Pricing Review\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\n[00:00] Hello\n",
        );

        let new_path = rename_meeting(&path, "Quarterly Pricing").expect("rename should succeed");
        let content = std::fs::read_to_string(&new_path).unwrap();
        assert!(content.contains("title: \"Quarterly Pricing\""));
        // Body must be preserved untouched.
        assert!(content.contains("[00:00] Hello"));
        // The post-write parse must round-trip.
        let (fm, _) = split_frontmatter(&content);
        let parsed: Frontmatter = serde_yaml::from_str(fm).unwrap();
        assert_eq!(parsed.title, "Quarterly Pricing");
        // The file name should reflect the new slug.
        assert!(
            new_path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .contains("quarterly-pricing"),
            "expected slug rename, got {}",
            new_path.display()
        );
        // The original path should no longer exist.
        assert!(!path.exists());
    }

    #[test]
    fn rename_meeting_handles_unquoted_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-team-sync.md",
            "title: Team Sync\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHello\n",
        );

        let new_path = rename_meeting(&path, "Team Standup").unwrap();
        let content = std::fs::read_to_string(&new_path).unwrap();
        assert!(content.contains("title: \"Team Standup\""));
    }

    #[test]
    fn rename_meeting_preserves_user_added_sections() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-call.md",
            "title: \"Call\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Summary\n\nWent well\n\n## Custom Section From User\n\nHand-edited stuff\n\n## Transcript\n\n[00:00] Hi\n",
        );

        let new_path = rename_meeting(&path, "Important Call").unwrap();
        let content = std::fs::read_to_string(&new_path).unwrap();
        // Hand-edited section must survive.
        assert!(content.contains("## Custom Section From User"));
        assert!(content.contains("Hand-edited stuff"));
    }

    #[test]
    fn update_frontmatter_preserves_body_byte_for_byte() {
        let dir = TempDir::new().unwrap();
        // Body with unicode, emoji, trailing spaces, and a markdown rule that
        // contains "---" to make sure we never confuse it with the fence.
        let body = "## Summary\n\nWent well café 🎤\n\n---\n\n## Custom Notes\n\n- trailing spaces   \n\n## Transcript\n\n[SPEAKER_00 00:00] Hi\n[SPEAKER_01 00:05] Hello\n";
        let path = write_meeting(
            &dir,
            "2026-04-07-redo.md",
            "title: \"Redo Test\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            body,
        );
        let original = std::fs::read_to_string(&path).unwrap();
        let (_, orig_body) = split_frontmatter(&original);
        let orig_body = orig_body.to_string();

        update_frontmatter(&path, |fm| {
            fm.speaker_mapping = Some(SpeakerMappingHealth {
                status: "ok".into(),
                model: "agent:claude".into(),
                diarized_speakers: 2,
                mapped_speakers: 2,
                attendees: 2,
                duration_ms: Some(1234),
                reason: None,
                last_run: Some("2026-06-30T12:00:00-07:00".into()),
            });
        })
        .unwrap();

        let updated = std::fs::read_to_string(&path).unwrap();
        let (updated_fm, updated_body) = split_frontmatter(&updated);

        // The body must be preserved exactly.
        assert_eq!(orig_body, updated_body);
        // The new field must have landed and round-trip through serde.
        assert!(updated_fm.contains("speaker_mapping:"));
        let parsed: Frontmatter = serde_yaml::from_str(updated_fm).unwrap();
        let health = parsed.speaker_mapping.expect("speaker_mapping written");
        assert_eq!(health.status, "ok");
        assert_eq!(health.mapped_speakers, 2);
        assert_eq!(health.duration_ms, Some(1234));
    }

    #[test]
    fn update_frontmatter_preserves_body_with_glued_closing_fence() {
        // Regression (#384): a closing fence glued to body text with no newline
        // anywhere after it. `split_frontmatter` keeps the trailing bytes; the
        // earlier bespoke offset math in `update_frontmatter` dropped them. Now
        // both share `split_frontmatter`'s body, so nothing is lost.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("glued.md");
        let original = "---\ntitle: \"Glued\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n---BODYBYTES";
        std::fs::write(&path, original).unwrap();

        let (_, orig_body) = split_frontmatter(original);
        assert_eq!(orig_body, "BODYBYTES"); // sanity: the bytes we must not lose

        update_frontmatter(&path, |fm| {
            fm.speaker_mapping = Some(SpeakerMappingHealth {
                status: "skipped".into(),
                model: "none".into(),
                diarized_speakers: 0,
                mapped_speakers: 0,
                attendees: 0,
                duration_ms: None,
                reason: Some("test".into()),
                last_run: None,
            });
        })
        .unwrap();

        let updated = std::fs::read_to_string(&path).unwrap();
        let (_, updated_body) = split_frontmatter(&updated);
        assert_eq!(updated_body, "BODYBYTES");
    }

    #[test]
    fn update_frontmatter_refuses_non_meeting_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("not-a-meeting.md");
        let original = "# Just markdown\n\nNo frontmatter here.\n";
        std::fs::write(&path, original).unwrap();

        let err = update_frontmatter(&path, |fm| {
            fm.title = "Hijacked".into();
        })
        .unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));

        // File must be untouched.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn rename_meeting_refuses_folded_scalar_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-folded.md",
            "title: >\n  Pricing\n  Review\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        let original = std::fs::read_to_string(&path).unwrap();

        let err = rename_meeting(&path, "Q4 Pricing").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));

        // Original file MUST be unchanged.
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn rename_meeting_refuses_literal_block_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-literal.md",
            "title: |\n  Multi\n  line\n  title\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        let original = std::fs::read_to_string(&path).unwrap();

        let err = rename_meeting(&path, "Single Line").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn rename_meeting_refuses_anchored_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-anchored.md",
            "title: &meeting_title \"Pricing Review\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        let original = std::fs::read_to_string(&path).unwrap();

        let err = rename_meeting(&path, "Q4 Pricing").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));
        // The original file is untouched even though our serde parse
        // would happily accept the anchor.
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn rename_meeting_refuses_empty_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-empty.md",
            "title: \"Pricing\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );

        let err = rename_meeting(&path, "   ").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));
    }

    #[test]
    fn rename_meeting_refuses_newline_in_new_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-nl.md",
            "title: \"Pricing\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );

        let err = rename_meeting(&path, "First\nSecond").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));
    }

    #[test]
    fn rename_meeting_refuses_file_without_frontmatter() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("plain.md");
        std::fs::write(&path, "no frontmatter here\n").unwrap();

        let err = rename_meeting(&path, "Anything").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));
    }

    #[test]
    fn rename_meeting_quotes_special_chars_in_new_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-call.md",
            "title: \"Call\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );

        let new_path = rename_meeting(&path, "Quote \"this\" and \\that").unwrap();
        let content = std::fs::read_to_string(&new_path).unwrap();
        // Round-trip via serde_yaml — the special chars must survive.
        let (fm, _) = split_frontmatter(&content);
        let parsed: Frontmatter = serde_yaml::from_str(fm).unwrap();
        assert_eq!(parsed.title, "Quote \"this\" and \\that");
    }

    #[test]
    fn rename_meeting_resolves_slug_collision() {
        let dir = TempDir::new().unwrap();
        let frontmatter =
            "title: \"Call\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n";
        let path = write_meeting(
            &dir,
            "2026-04-07-call.md",
            frontmatter,
            "## Transcript\n\nHi\n",
        );
        // Pre-create a sibling that the new slug would collide with.
        let parsed: Frontmatter = serde_yaml::from_str(frontmatter).unwrap();
        let collision_slug = generate_slug("Pricing Review", parsed.date, None);
        std::fs::write(
            dir.path().join(&collision_slug),
            "---\ntitle: existing\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n---\n",
        )
        .unwrap();

        let new_path = rename_meeting(&path, "Pricing Review").unwrap();
        let name = new_path.file_name().unwrap().to_str().unwrap();
        let collision_stem = collision_slug.trim_end_matches(".md");
        assert!(
            name.starts_with(&format!("{collision_stem}-")) && name.ends_with(".md"),
            "expected collision-resolved slug, got {}",
            name
        );
    }

    #[test]
    fn rename_meeting_refuses_aliased_title() {
        // YAML alias `*meeting_title` references an anchor defined
        // elsewhere. The naive line replace would drop the alias
        // reference and silently break frontmatter that depends on it.
        // Codex pass 2 P2 #4.
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-aliased.md",
            "title: *meeting_title\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        let original = std::fs::read_to_string(&path).unwrap();

        let err = rename_meeting(&path, "Q4 Pricing").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));

        // Original file MUST be unchanged.
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn rename_meeting_handles_crlf_line_endings() {
        // Files saved on Windows or copied through email may have
        // CRLF endings in the frontmatter. Rename must succeed and
        // produce a parseable result. We do not promise CRLF
        // preservation in the body — only that the rename is not
        // corrupted by it. Codex pass 2 P2 #4.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("2026-04-07-crlf.md");
        let content = "---\r\n\
            title: \"Pricing\"\r\n\
            type: meeting\r\n\
            date: 2026-04-07T10:00:00-07:00\r\n\
            duration: 0\r\n\
            ---\r\n\
            ## Transcript\r\n\
            \r\n\
            Hi\r\n";
        std::fs::write(&path, content).unwrap();

        let new_path = rename_meeting(&path, "Quarterly Pricing").unwrap();
        let after = std::fs::read_to_string(&new_path).unwrap();
        let (fm, body) = split_frontmatter(&after);
        let parsed: Frontmatter = serde_yaml::from_str(fm).unwrap();
        assert_eq!(parsed.title, "Quarterly Pricing");
        assert!(body.contains("## Transcript"));
        assert!(body.contains("Hi"));
    }

    #[test]
    fn rename_meeting_post_write_validation_rolls_back_on_corruption() {
        // We can't easily force a real serde_yaml parse failure on a
        // properly-quoted title, so this test verifies the rollback
        // PATH by exercising it with a known-good rename and confirming
        // there's no leftover .md.rename.tmp sibling. The path is
        // exercised end-to-end; the assertion is "no temp files
        // remain after a successful rename, and the original was
        // replaced atomically."
        // Codex pass 2 P2 #4.
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-validate.md",
            "title: \"Old\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );

        let _ = rename_meeting(&path, "New").unwrap();

        // No leftover tmp files anywhere in the dir.
        let entries: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        for name in &entries {
            assert!(
                !name.ends_with(".md.rename.tmp"),
                "leftover tmp file: {} (entries: {:?})",
                name,
                entries
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rename_meeting_preserves_user_chosen_file_mode() {
        // The Minutes default is 0o600, but a user may have chmod'd
        // their meetings to 0o644 for an Obsidian sync, a local
        // webserver preview, or any other workflow. The rename must
        // preserve those bits — codex pass 3 / claude pass 3 P3.
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-mode.md",
            "title: \"Old\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let new_path = rename_meeting(&path, "New").unwrap();
        let after_meta = std::fs::metadata(&new_path).unwrap();
        let after_mode = after_meta.permissions().mode() & 0o777;
        assert_eq!(
            after_mode, 0o644,
            "rename should preserve the original file mode (0o644), got 0o{:o}",
            after_mode
        );
    }

    #[test]
    fn rename_meeting_no_op_when_title_unchanged() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-pricing-review.md",
            "title: \"Pricing Review\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        let original = std::fs::read_to_string(&path).unwrap();
        let result = rename_meeting(&path, "Pricing Review").unwrap();
        assert_eq!(result, path);
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn yaml_quote_escapes_required_chars() {
        assert_eq!(yaml_quote("plain"), r#""plain""#);
        assert_eq!(yaml_quote("with \"quotes\""), r#""with \"quotes\"""#);
        assert_eq!(yaml_quote("back\\slash"), r#""back\\slash""#);
        assert_eq!(yaml_quote("tab\there"), r#""tab\there""#);
    }

    #[test]
    fn no_speech_output_includes_retry_instructions() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let audio = dir.path().join("capture.wav");

        let fm = Frontmatter {
            status: Some(OutputStatus::NoSpeech),
            filter_diagnosis: Some("audio: 5.0s, whisper produced 3 segments, no_speech filter: -3 → 0, final: 0 words".into()),
            ..test_frontmatter()
        };

        let result = write_with_retry_path(&fm, "", None, None, Some(&audio), &config).unwrap();
        let content = fs::read_to_string(&result.path).unwrap();
        assert!(content.contains("No speech detected"));
        assert!(content.contains("**Diagnosis**:"));
        assert!(content.contains("no_speech filter"));
        assert!(content.contains(audio.display().to_string().as_str()));
        assert!(content.contains("minutes process"));
    }

    // --- section-range parser (migrated from the CLI transcript extractor) ---

    #[test]
    fn section_requires_exact_heading() {
        // A look-alike heading must not be treated as the transcript.
        let body = "## Transcript cleanup notes\n\n[SPEAKER_00 0:00] not the transcript\n";
        assert!(first_section_text(body, "Transcript").is_none());
    }

    #[test]
    fn section_ignores_fenced_code_block() {
        // A `## Transcript` line inside a code fence must be ignored; the real
        // section after the fence is the one that wins.
        let body = "## Summary\n\n```\n## Transcript\n[SPEAKER_99 0:00] fake\n```\n\n## Transcript\n\n[SPEAKER_00 0:00] real line\n";
        let t = first_section_text(body, "Transcript").expect("real transcript section found");
        assert!(t.contains("real line"));
        assert!(!t.contains("fake"));
    }

    #[test]
    fn section_handles_mixed_fence_markers() {
        // A backtick fence whose body contains a `~~~` line must NOT be treated
        // as closed by that tilde line; the fake `## Transcript` inside stays
        // ignored and the real section after the fence wins.
        let body = "## Summary\n\n```\n~~~\n## Transcript\n[SPEAKER_99 0:00] fake\n```\n\n## Transcript\n\n[SPEAKER_00 0:00] real line\n";
        let t = first_section_text(body, "Transcript").expect("real transcript section found");
        assert!(t.contains("real line"));
        assert!(!t.contains("fake"));
    }

    #[test]
    fn section_keeps_shorter_backtick_run_inside_longer_fence() {
        let body = "## Summary\n\n````\n```\n## Transcript\n[SPEAKER_99 0:00] fake\n````\n\n## Transcript\n\n[SPEAKER_00 0:00] real line\n";
        let t = first_section_text(body, "Transcript").expect("real transcript section found");
        assert!(t.contains("real line"));
        assert!(!t.contains("fake"));
    }

    #[test]
    fn section_keeps_info_string_fence_line_inside_open_fence() {
        let body = "## Summary\n\n```\n```rust\n## Transcript\n[SPEAKER_99 0:00] fake\n```\n\n## Transcript\n\n[SPEAKER_00 0:00] real line\n";
        let t = first_section_text(body, "Transcript").expect("real transcript section found");
        assert!(t.contains("real line"));
        assert!(!t.contains("fake"));
    }

    #[test]
    fn guarded_rewrite_conflict_leaves_no_backup() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meeting.md");
        let original = "---\ntitle: Test\ntype: meeting\ndate: 2026-07-23\nduration: 1m\n---\n\n## Transcript\n\nold\n";
        let replacement = original.replace("old", "new");
        fs::write(&path, original).unwrap();
        let expected = fs::read_to_string(&path).unwrap();
        fs::write(&path, original.replace("old", "edited by user")).unwrap();
        let backup = dir.path().join(".meeting.md.pre-resummarize.1.bak");

        let err = atomic_rewrite_preserving_mode_guarded(
            &path,
            &replacement,
            Some(&expected),
            Some(&backup),
        )
        .unwrap_err();

        assert!(matches!(err, MarkdownError::ConcurrentModification));
        assert!(!backup.exists());
    }

    #[cfg(unix)]
    #[test]
    fn guarded_rewrite_preserves_team_file_mode_despite_umask() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meeting.md");
        let original = "---\ntitle: Test\ntype: meeting\ndate: 2026-07-23\nduration: 1m\n---\n\n## Transcript\n\nold\n";
        let replacement = original.replace("old", "new");
        fs::write(&path, original).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        atomic_rewrite_preserving_mode_guarded(&path, &replacement, None, None).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o640);
    }

    #[test]
    fn section_extracts_first_and_stops_at_next_h2() {
        let body = "## Transcript\n\n[SPEAKER_00 0:00] hello\n\n## Action Items\n\n- do thing\n";
        let t = first_section_text(body, "Transcript").unwrap();
        assert!(t.contains("hello"));
        assert!(!t.contains("Action Items"));
    }

    // --- new coverage beyond the migrated CLI tests ---

    #[test]
    fn find_unique_section_absent_is_ok_none() {
        let body = "## Summary\n\ntext\n";
        assert_eq!(find_unique_section(body, "Transcript").unwrap(), None);
    }

    #[test]
    fn find_unique_section_rejects_duplicates() {
        let body = "## Transcript\na\n## Notes\nn\n## Transcript\nb\n";
        let err = find_unique_section(body, "Transcript").unwrap_err();
        match err {
            MarkdownError::AmbiguousSection { name, count } => {
                assert_eq!(name, "Transcript");
                assert_eq!(count, 2);
            }
            other => panic!("expected AmbiguousSection, got {other:?}"),
        }
        // The lenient reader still resolves to the first occurrence.
        assert_eq!(first_section_text(body, "Transcript").unwrap(), "a");
    }

    #[test]
    fn section_range_splices_without_touching_neighbors() {
        let body = "## Summary\n\nold summary\n\n## Transcript\n\n[SPEAKER_00 0:00] hi\n";
        let range = find_unique_section(body, "Summary").unwrap().unwrap();
        let spliced = format!(
            "{}\nnew summary\n\n{}",
            &body[..range.content_start],
            &body[range.end..]
        );
        assert_eq!(
            spliced,
            "## Summary\n\nnew summary\n\n## Transcript\n\n[SPEAKER_00 0:00] hi\n"
        );
    }

    #[test]
    fn section_range_handles_crlf_bodies() {
        let body = "## Summary\r\nold\r\n## Transcript\r\n[SPEAKER_00 0:00] hi\r\n";
        let summary = find_unique_section(body, "Summary").unwrap().unwrap();
        // Extracted text is newline-normalized...
        assert_eq!(section_text(body, summary), "old");
        // ...but the byte range preserves the CRLF world around a splice.
        let untouched = &body[summary.end..];
        assert!(untouched.starts_with("## Transcript\r\n"));
    }

    #[test]
    fn section_at_end_of_body_runs_to_len() {
        let body = "## Notes\n\nkeep me\n\n## Transcript\n\n[SPEAKER_00 0:00] tail";
        let range = find_unique_section(body, "Transcript").unwrap().unwrap();
        assert_eq!(range.end, body.len());
        assert_eq!(section_text(body, range), "[SPEAKER_00 0:00] tail");
    }

    #[test]
    fn empty_section_yields_empty_text() {
        // Heading as the final line, no trailing newline: content is empty,
        // but the section still exists (Some(""), matching the old extractor).
        let body = "## Summary\n\ns\n\n## Transcript";
        let range = find_unique_section(body, "Transcript").unwrap().unwrap();
        assert_eq!(range.content_start, range.end);
        assert_eq!(first_section_text(body, "Transcript").unwrap(), "");
    }

    #[test]
    fn indented_heading_is_not_a_section_boundary() {
        // Headings must start at column 0 (same rule as the old extractor);
        // an indented `## ` line is content and must not end the section.
        let body = "## Transcript\n\nline one\n  ## Action Items\nline two\n";
        let t = first_section_text(body, "Transcript").unwrap();
        assert!(t.contains("line one"));
        assert!(t.contains("line two"));
        assert!(t.contains("## Action Items"));
    }

    #[test]
    fn find_sections_reports_every_duplicate_location() {
        let body = "## Transcript\na\n## Transcript\nb\n";
        let all = find_sections(body, "Transcript");
        assert_eq!(all.len(), 2);
        assert_eq!(section_text(body, all[0]), "a");
        assert_eq!(section_text(body, all[1]), "b");
    }
}
