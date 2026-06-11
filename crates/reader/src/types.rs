use chrono::{DateTime, Local};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ──────────────────────────────────────────────────────────────
// Standalone types for parsing Minutes meeting files.
// Duplicated from minutes-core to avoid pulling in audio deps.
// ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    Meeting,
    Memo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OutputStatus {
    Complete,
    NoSpeech,
    TranscriptOnly,
    /// Transcription ran but one or more post-transcript steps fell back
    /// to empty output. Details in `processing_warnings`. See issue #243.
    Degraded,
}

/// How a meeting artifact was captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CapturePolicy {
    /// No audio was captured for this meeting artifact.
    None,
}

/// Sensitivity designation for read-only consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    /// Standard meeting artifact.
    Normal,
    /// Restricted artifact; agent enforcement is layered by higher-level callers.
    Restricted,
}

/// Human debrief state for no-capture meeting artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DebriefStatus {
    /// The meeting was stopped without an interactive debrief.
    Pending,
}

/// A non-fatal failure of a post-transcript pipeline step. Mirrors the
/// core crate's `markdown::ProcessingWarning`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessingWarning {
    pub step: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Frontmatter {
    pub title: String,
    pub r#type: ContentType,
    pub date: DateTime<Local>,
    pub duration: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<OutputStatus>,
    /// Per-step failure context when `status: degraded` applies (#243).
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaker_map: Vec<SpeakerAttribution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_health: Option<RecordingHealth>,
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
    pub kind: FailureKind,
    pub source: CaptureSource,
    pub message: String,
    pub diagnostic_confidence: DiagnosticConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FailureKind {
    Silent,
    Sparse,
    Missing,
    BackendUnavailable,
    StreamError,
    SourceStarved,
    UnsupportedFormat,
    MisconfiguredRoute,
    PermissionDenied,
    RouteUnavailable,
    Other { code: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CaptureSource {
    Voice,
    System,
    Both,
    Backend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticConfidence {
    High,
    Inferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AttributionSource {
    Deterministic,
    Llm,
    Enrollment,
    Manual,
    #[serde(rename = "ml-bleed-degraded")]
    MlBleedDegraded,
    #[serde(rename = "stem-recovery")]
    StemRecovery,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SpeakerAttribution {
    pub speaker_label: String,
    pub name: String,
    pub confidence: Confidence,
    pub source: AttributionSource,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ActionItem {
    pub assignee: String,
    pub task: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Decision {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
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

#[derive(Debug, Clone)]
pub struct ParsedMeeting {
    pub frontmatter: Frontmatter,
    pub body: String,
    pub path: PathBuf,
}
