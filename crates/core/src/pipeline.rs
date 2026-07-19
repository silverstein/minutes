use crate::config::{Config, IdentityConfig, NameCorrectionMode};
use crate::diarize;
use crate::error::MinutesError;
use crate::logging;
use crate::markdown::{
    self, ContentType, Frontmatter, OutputStatus, ProcessingWarning, WriteResult,
};
use crate::notes;
use crate::person_identity::{is_plausible_person_name, strip_contamination};
use crate::summarize;
use chrono::{DateTime, Local};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
#[cfg(not(any(target_os = "macos", windows)))]
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use whisper_guard::segments as wg_segments;
use zeroize::Zeroizing;

/// Stem active-ratio threshold below which a capture source is considered
/// "sparse" (almost no audible energy).
///
/// Keep in sync with the silence detector in `diarize::active_ratio` callers
/// (see `diarize.rs` around line 861, where the same `0.02` value classifies
/// a stem as `FailureKind::Sparse`). Pulled into a named constant so the
/// suppression gate below can read it without re-deriving the number.
const SPARSE_STEM_ACTIVE_RATIO: f32 = 0.02;

/// Body text we render in place of the transcript when the all-noise
/// suppression gate fires. Kept short and pointed - the surrounding markdown
/// already shows the diagnosis and `minutes process` retry hint, so this
/// just labels the gap.
const ALL_NOISE_SUPPRESSED_BODY: &str =
    "*No audible content was captured. See capture diagnostics.*";

/// Decide whether the transcript body should be suppressed because it is
/// almost certainly fabricated.
///
/// Returns `Some(diagnosis)` (a short human-readable string to store in
/// `Frontmatter::filter_diagnosis`) when **both** of these are true:
///
/// 1. Every non-empty line in `transcript` is a noise marker (bracketed
///    `[music]` / `[Growling]` or parenthetical `(crying)` / `(applause)`)
///    according to [`wg_segments::is_all_noise`].
/// 2. Both stem active ratios in `recording_health` are below
///    [`SPARSE_STEM_ACTIVE_RATIO`]. **Both ratios must be present**: if
///    either `voice_stem_active_ratio` or `system_stem_active_ratio` is
///    `None` (e.g. dictation captures with no system stem, or any recording
///    where stem-active health was not computed) the gate does NOT fire.
///    Missing health is treated as insufficient evidence to override the
///    transcript, not as confirmation that the stem was silent.
///
/// Otherwise returns `None` and the transcript flows through unchanged.
fn suppress_if_all_noise(
    transcript: &str,
    recording_health: Option<&markdown::RecordingHealth>,
) -> Option<String> {
    let lines: Vec<String> = transcript.lines().map(str::to_string).collect();
    if !wg_segments::is_all_noise(&lines) {
        return None;
    }

    let health = recording_health?;
    let voice = health.voice_stem_active_ratio?;
    let system = health.system_stem_active_ratio?;
    if voice >= SPARSE_STEM_ACTIVE_RATIO || system >= SPARSE_STEM_ACTIVE_RATIO {
        return None;
    }

    Some(format!(
        "all-noise transcript on sparse stems (voice active {:.3}, system active {:.3}, threshold {:.2}); whisper produced only non-speech markers - body suppressed",
        voice, system, SPARSE_STEM_ACTIVE_RATIO
    ))
}

/// Outcome of the shared suppression decision used by BOTH the
/// `write_transcript_artifact` background-recording path and the
/// `minutes process <wav>` reprocess path.
///
/// Returning a strongly typed struct (rather than a tuple) keeps the two call
/// sites mechanically identical and makes drift obvious in code review: if a
/// new field is added here the compiler forces an update at every call site.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SuppressionOutcome {
    /// Replacement body to write in place of the hallucinated transcript.
    body: String,
    /// Human-readable explanation, stored in `Frontmatter::filter_diagnosis`.
    diagnosis: String,
}

/// Shared decision: should the transcript body be suppressed because it is
/// almost certainly fabricated noise on near-silent audio?
///
/// This is the **single source of truth** for the suppression rule. Both
/// `write_transcript_artifact` (background recording finalizer) and
/// `process_with_progress_and_sidecar` (`minutes process <wav>` reprocess
/// path) call this helper so users see identical behavior regardless of
/// which entry point produced the artifact - the codex review on PR #246
/// flagged this drift as blocker #2.
///
/// Returns `Some(outcome)` when [`suppress_if_all_noise`] confirms the
/// transcript is all noise markers AND both stems are sparse; `None`
/// otherwise.
fn should_suppress_transcript(
    transcript: &str,
    recording_health: Option<&markdown::RecordingHealth>,
) -> Option<SuppressionOutcome> {
    let diagnosis = suppress_if_all_noise(transcript, recording_health)?;
    Some(SuppressionOutcome {
        body: ALL_NOISE_SUPPRESSED_BODY.to_string(),
        diagnosis,
    })
}

/// Detect post-transcript pipeline degradation and produce one
/// [`ProcessingWarning`] per failed step. See issue #243.
///
/// Returns an empty `Vec` when nothing degraded. Callers should promote
/// [`OutputStatus::Complete`] to [`OutputStatus::Degraded`] when the
/// result is non-empty and store the warnings on
/// [`Frontmatter::processing_warnings`] so the file itself is honest
/// about which sections are missing or fell back to defaults.
///
/// Today this detects the most user-visible failure mode: the
/// summarization engine returned `None` despite being configured to run
/// (typically an agent-CLI timeout or unexpected error). Follow-up
/// PRs can plumb richer per-step warnings through the LLM call sites
/// to populate the `reason`, `timeout_secs`, and `message` fields with
/// precise context.
///
/// **Single source of truth** for the suppression rule shared by both
/// `write_transcript_artifact` and `process_with_progress_and_sidecar`.
/// Keeping the detection here (rather than at each call site) prevents
/// the two paths from drifting and emitting different status values
/// for the same underlying failure.
fn detect_summarization_warnings(
    summary: Option<&str>,
    engine: &str,
    agent_command: &str,
    agent_timeout_secs: u64,
    summarization_attempted: bool,
) -> Vec<ProcessingWarning> {
    let mut warnings = Vec::new();
    if engine == "none" {
        return warnings;
    }
    // If summarization was deliberately not attempted (e.g. no-speech path
    // or all-noise suppression), an absent summary is expected behavior,
    // not a degradation. Without this guard the helper would emit a bogus
    // `summarize_failed` warning on every no-speech recording.
    if !summarization_attempted {
        return warnings;
    }
    if summary.is_none() {
        // We know the engine ran and produced nothing. The specific reason
        // (timeout vs error vs network failure) lives in audio.log; this is
        // a coarser file-level signal so users see something is missing
        // without grepping logs. Follow-up: plumb the precise reason
        // through summarize::run_summarization's return type.
        let (reason, timeout_secs, message) = match engine {
            "agent" => (
                "summarize_failed".to_string(),
                Some(agent_timeout_secs),
                Some(format!(
                    "Summarization via agent `{}` produced no output (timeout budget {}s, or agent error); see audio.log for the precise reason.",
                    agent_command, agent_timeout_secs
                )),
            ),
            "auto" => (
                "summarize_failed".to_string(),
                Some(agent_timeout_secs),
                Some(format!(
                    "Summarization with `engine = \"auto\"` produced no output (auto-detect picks the first available agent CLI then runs under the {}s budget); see audio.log for which agent was selected and the precise failure.",
                    agent_timeout_secs
                )),
            ),
            other => (
                "summarize_failed".to_string(),
                None,
                Some(format!(
                    "Summarization via engine `{}` produced no output; see audio.log for the precise reason.",
                    other
                )),
            ),
        };
        warnings.push(ProcessingWarning {
            step: "summarize".to_string(),
            reason,
            timeout_secs,
            message,
        });
    }
    warnings
}

const SILENT_REMOTE_WARNING_MESSAGE: &str =
    "Call/remote audio was not captured (system stem silent); transcript reflects only your microphone";
const SILENT_MICROPHONE_WARNING_MESSAGE: &str =
    "Microphone audio was not captured (mic stem silent or missing); transcript reflects only call/remote audio";

fn warning_is_native_call_system_stem_recovery(warning: &markdown::CaptureWarning) -> bool {
    match &warning.kind {
        diarize::FailureKind::Other { code } => {
            code == crate::health::NATIVE_CALL_SYSTEM_RECOVERY_CODE
                || (code == "native-call-stem-recovery"
                    && warning
                        .message
                        .to_ascii_lowercase()
                        .contains("system-audio stem"))
        }
        _ => false,
    }
}

fn warning_is_native_call_microphone_stem_recovery(warning: &markdown::CaptureWarning) -> bool {
    matches!(
        &warning.kind,
        diarize::FailureKind::Other { code }
            if code == crate::health::NATIVE_CALL_MICROPHONE_RECOVERY_CODE
    )
}

fn detect_silent_remote_stem_warning(
    content_type: ContentType,
    _audio_path: &Path,
    _source: Option<&str>,
    recording_health: Option<&markdown::RecordingHealth>,
) -> Option<ProcessingWarning> {
    if content_type != ContentType::Meeting {
        return None;
    }

    let health = recording_health?;
    if health
        .capture_warnings
        .iter()
        .any(warning_is_native_call_microphone_stem_recovery)
    {
        return Some(ProcessingWarning {
            step: "capture".to_string(),
            reason: "microphone_audio_not_captured".to_string(),
            timeout_secs: None,
            message: Some(SILENT_MICROPHONE_WARNING_MESSAGE.to_string()),
        });
    }

    health
        .capture_warnings
        .iter()
        .any(warning_is_native_call_system_stem_recovery)
        .then(|| ProcessingWarning {
            step: "capture".to_string(),
            reason: "remote_audio_not_captured".to_string(),
            timeout_secs: None,
            message: Some(SILENT_REMOTE_WARNING_MESSAGE.to_string()),
        })
}

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
        diarize::AttributionSource::MlBleedDegraded => "ml-bleed-degraded".into(),
        diarize::AttributionSource::StemRecovery => "stem-recovery".into(),
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

fn is_degraded_ml_fallback_result(result: &diarize::DiarizationResult) -> bool {
    result.degraded_capture.is_some() && !result.from_stems && !result.source_aware
}

fn degraded_ml_recording_health(reason: diarize::DegradedCapture) -> markdown::RecordingHealth {
    markdown::RecordingHealth::from_degraded_capture(
        reason,
        markdown::DiarizationPath::MlBleedDegraded,
    )
}

fn merge_recording_health(
    primary: Option<markdown::RecordingHealth>,
    existing: Option<markdown::RecordingHealth>,
) -> Option<markdown::RecordingHealth> {
    match (primary, existing) {
        (Some(mut primary), Some(existing)) => {
            primary.voice_stem_active_ratio = primary
                .voice_stem_active_ratio
                .or(existing.voice_stem_active_ratio);
            primary.system_stem_active_ratio = primary
                .system_stem_active_ratio
                .or(existing.system_stem_active_ratio);
            primary.system_dominant_ratio = primary
                .system_dominant_ratio
                .or(existing.system_dominant_ratio);
            if primary.diarization_path.is_none() {
                primary.diarization_path = existing.diarization_path;
            }
            let mut warnings = existing.capture_warnings;
            warnings.extend(primary.capture_warnings);
            primary.capture_warnings = warnings;
            Some(primary)
        }
        (Some(primary), None) => Some(primary),
        (None, Some(existing)) => Some(existing),
        (None, None) => None,
    }
}

fn mark_degraded_ml_attributions(speaker_map: &mut [diarize::SpeakerAttribution]) {
    for attribution in speaker_map {
        attribution.confidence = diarize::Confidence::Low;
        attribution.source = diarize::AttributionSource::MlBleedDegraded;
    }
}

fn log_rendered_label_collapse_diagnostic(
    audio_path: &Path,
    result: &diarize::DiarizationResult,
    transcript: &str,
) {
    if result.num_speakers <= 1 {
        return;
    }

    let rendered_labels = extract_effective_transcript_speaker_labels(transcript);
    let rendered_speaker_labels = rendered_labels
        .iter()
        .filter(|label| label.as_str() != "UNKNOWN")
        .count();
    if rendered_speaker_labels > 1 {
        return;
    }

    tracing::warn!(
        diarization_speakers = result.num_speakers,
        rendered_speaker_labels,
        degraded_capture = result.degraded_capture.is_some(),
        audio = %private_audio_diagnostic_label(audio_path),
        "diarization found multiple speakers but transcript rendered one or fewer speaker labels"
    );
    logging::log_step(
        "diarize_rendered_label_collapse",
        &private_audio_diagnostic_label(audio_path),
        0,
        serde_json::json!({
            "diagnostic": true,
            "diarization_speakers": result.num_speakers,
            "rendered_speaker_labels": rendered_speaker_labels,
            "degraded_capture": result.degraded_capture.is_some(),
        }),
    );
}

fn expected_voice_stem_path(audio_path: &Path) -> Option<std::path::PathBuf> {
    let stem = audio_path.file_stem()?.to_str()?;
    let dir = audio_path.parent()?;
    Some(dir.join(format!("{}.voice.wav", stem)))
}

#[cfg(target_os = "linux")]
fn ensure_private_audio_process_barrier() -> std::io::Result<()> {
    static BARRIER: OnceLock<Result<(), String>> = OnceLock::new();
    let result = BARRIER.get_or_init(|| {
        // Linux exposes anonymous descriptors through /proc/<pid>/fd to any
        // process allowed to ptrace this one. Owner-only modes, O_TMPFILE and
        // CLOEXEC do not close that same-UID route. Make this process and its
        // ordinary children non-dumpable before the first audio descriptor is
        // created; the Parakeet child inherits the barrier while receiving
        // only its explicitly leased descriptor.
        if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
            return Err("could not install the Linux private-audio process barrier".into());
        }
        if unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) } != 0 {
            return Err("Linux private-audio process barrier did not remain active".into());
        }
        Ok(())
    });
    result
        .as_ref()
        .map_err(|message| {
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, message.clone())
        })
        .copied()
}

#[cfg(target_os = "linux")]
fn create_anonymous_audio_file(root: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    ensure_private_audio_process_barrier()?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_TMPFILE | libc::O_CLOEXEC)
        .open(root)?;
    verify_anonymous_audio_file(&file)?;
    Ok(file)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn create_anonymous_audio_file(_root: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "anonymous private audio capabilities are unavailable on this Unix platform",
    ))
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn verify_anonymous_audio_file(_file: &File) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "anonymous private audio capabilities are unavailable on this Unix platform",
    ))
}

#[cfg(target_os = "linux")]
fn verify_anonymous_audio_file(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 0 || metadata.permissions().mode() & 0o077 != 0 {
        return Err(std::io::Error::other(
            "private audio capability is not anonymous and owner-private",
        ));
    }
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
    if flags < 0 || flags & libc::FD_CLOEXEC == 0 {
        return Err(std::io::Error::other(
            "private audio capability is unexpectedly inheritable",
        ));
    }
    Ok(())
}

/// Process-local name for an exact private-audio handle.
///
/// The returned path is deliberately not a filesystem pathname. It is an
/// opaque key into this process's registry, allowing existing pipeline APIs to
/// carry format information without exposing a plaintext filesystem path.
struct PrivateAudioRegistration {
    path: PathBuf,
}

enum RegisteredPrivateAudio {
    #[cfg(not(any(target_os = "macos", windows)))]
    File(Arc<RegisteredFileAudio>),
    #[cfg(any(target_os = "macos", windows))]
    Sealed(crate::sealed_audio::WeakSealedAudio),
}

#[cfg(not(any(target_os = "macos", windows)))]
const MAX_ACTIVE_REGISTERED_PRIVATE_AUDIO_READERS: usize = 8;

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) struct RegisteredFileAudio {
    file: File,
    state: Mutex<RegisteredFileGenerationState>,
}

#[cfg(not(any(target_os = "macos", windows)))]
struct RegisteredFileGenerationState {
    generation: u64,
    sealed: bool,
    writer_issued: bool,
    writer_active: bool,
    active_readers: usize,
    retired: bool,
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) struct RegisteredFileReaderLease {
    audio: Arc<RegisteredFileAudio>,
    generation: u64,
}

#[cfg(not(any(target_os = "macos", windows)))]
impl Drop for RegisteredFileReaderLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.audio.state.lock() {
            state.active_readers = state.active_readers.saturating_sub(1);
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
impl RegisteredFileReaderLease {
    fn verify(&self) -> std::io::Result<()> {
        let state = self
            .audio
            .state
            .lock()
            .map_err(|_| std::io::Error::other("private audio generation lock poisoned"))?;
        if state.retired || !state.sealed || state.generation != self.generation {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "private audio reader generation was retired",
            ));
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) struct RegisteredFileWriterLease {
    audio: Arc<RegisteredFileAudio>,
    generation: u64,
}

#[cfg(not(any(target_os = "macos", windows)))]
impl Drop for RegisteredFileWriterLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.audio.state.lock() {
            if state.generation == self.generation {
                state.writer_active = false;
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
impl RegisteredFileAudio {
    fn prepare_writer(self: &Arc<Self>) -> std::io::Result<PrivateAudioWriter> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| std::io::Error::other("private audio generation lock poisoned"))?;
        if state.retired || state.writer_active || state.active_readers != 0 {
            return Err(std::io::Error::other("private audio is still in use"));
        }

        state.generation = state
            .generation
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("private audio generation counter overflowed"))?;
        state.sealed = false;
        state.writer_issued = true;
        state.writer_active = true;
        let generation = state.generation;

        let prepared = self.file.try_clone().and_then(|mut file| {
            file.set_len(0)?;
            file.rewind()?;
            Ok(file)
        });
        let file = match prepared {
            Ok(file) => file,
            Err(error) => {
                // Once destructive preparation starts, no prior generation may
                // remain readable. Retire even when the failure happened before
                // truncation; fail-closed is preferable to guessing which
                // descriptor operation completed.
                state.writer_active = false;
                state.retired = true;
                return Err(error);
            }
        };
        drop(state);

        Ok(PrivateAudioWriter::File {
            file,
            _registered_lease: Some(RegisteredFileWriterLease {
                audio: Arc::clone(self),
                generation,
            }),
        })
    }

    fn finish(&self) -> std::io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| std::io::Error::other("private audio generation lock poisoned"))?;
        if state.retired || state.writer_active || !state.writer_issued {
            return Err(std::io::Error::other(
                "private audio generation is incomplete or retired",
            ));
        }
        self.file.sync_all()?;
        state.sealed = true;
        Ok(())
    }
}

fn private_audio_registry() -> &'static Mutex<HashMap<PathBuf, RegisteredPrivateAudio>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, RegisteredPrivateAudio>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn is_reserved_private_audio_path(path: &Path) -> bool {
    path.starts_with(Path::new("/minutes-private-audio"))
}

pub(crate) fn private_audio_diagnostic_label(path: &Path) -> String {
    if is_reserved_private_audio_path(path) {
        "private-audio".into()
    } else {
        path.display().to_string()
    }
}

fn allocate_private_audio_registration(
    backing: RegisteredPrivateAudio,
    format_extension: &str,
) -> std::io::Result<PrivateAudioRegistration> {
    let extension = format_extension.trim_start_matches('.');
    for _ in 0..16 {
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|error| {
            std::io::Error::other(format!("private audio registry nonce failed: {error}"))
        })?;
        let token = nonce
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = PathBuf::from("/minutes-private-audio").join(format!("{token}.{extension}"));
        let mut registry = private_audio_registry()
            .lock()
            .map_err(|_| std::io::Error::other("private audio registry lock poisoned"))?;
        if registry.contains_key(&path) {
            continue;
        }
        registry.insert(path.clone(), backing);
        return Ok(PrivateAudioRegistration { path });
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique private audio registry capability",
    ))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn register_private_audio_file(
    file: &File,
    format_extension: &str,
) -> std::io::Result<PrivateAudioRegistration> {
    let retained = file.try_clone()?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let flags = unsafe { libc::fcntl(retained.as_raw_fd(), libc::F_GETFD) };
        if flags < 0 || flags & libc::FD_CLOEXEC == 0 {
            return Err(std::io::Error::other(
                "private audio registry descriptor is unexpectedly inheritable",
            ));
        }
    }
    allocate_private_audio_registration(
        RegisteredPrivateAudio::File(Arc::new(RegisteredFileAudio {
            file: retained,
            state: Mutex::new(RegisteredFileGenerationState {
                generation: 0,
                sealed: false,
                writer_issued: false,
                writer_active: false,
                active_readers: 0,
                retired: false,
            }),
        })),
        format_extension,
    )
}

#[cfg(any(target_os = "macos", windows))]
fn register_sealed_private_audio(
    audio: &crate::sealed_audio::SealedAudio,
    format_extension: &str,
) -> std::io::Result<PrivateAudioRegistration> {
    allocate_private_audio_registration(
        RegisteredPrivateAudio::Sealed(audio.downgrade()),
        format_extension,
    )
}

fn registered_private_audio_reader(path: &Path) -> std::io::Result<Option<PrivateAudioReader>> {
    let registry = private_audio_registry()
        .lock()
        .map_err(|_| std::io::Error::other("private audio registry lock poisoned"))?;
    match registry.get(path) {
        #[cfg(not(any(target_os = "macos", windows)))]
        Some(RegisteredPrivateAudio::File(audio)) => Ok(Some(
            PrivateAudioReader::from_registered_file(Arc::clone(audio))?,
        )),
        #[cfg(any(target_os = "macos", windows))]
        Some(RegisteredPrivateAudio::Sealed(audio)) => {
            let audio = audio.upgrade().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "private audio owner retired")
            })?;
            Ok(Some(PrivateAudioReader::Sealed(audio.reader()?)))
        }
        None if is_reserved_private_audio_path(path) => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "private audio capability is stale or forged",
        )),
        None => Ok(None),
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn registered_file_audio(path: &Path) -> std::io::Result<Arc<RegisteredFileAudio>> {
    let registry = private_audio_registry()
        .lock()
        .map_err(|_| std::io::Error::other("private audio registry lock poisoned"))?;
    match registry.get(path) {
        Some(RegisteredPrivateAudio::File(audio)) => Ok(Arc::clone(audio)),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "private audio capability is stale or forged",
        )),
    }
}

fn retire_private_audio_registration(path: &Path) {
    let removed = private_audio_registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(path);
    #[cfg(not(any(target_os = "macos", windows)))]
    if let Some(RegisteredPrivateAudio::File(audio)) = removed {
        if let Ok(mut state) = audio.state.lock() {
            state.retired = true;
            state.sealed = false;
        }
    }
    #[cfg(any(target_os = "macos", windows))]
    let _ = removed;
}

/// Independent cursor over an exact private-audio handle.
///
/// Unix `dup` descriptors share one kernel file offset. Using positional reads
/// keeps concurrent transcription, probes, and diarization from advancing one
/// another even though they refer to the same anonymous object.
pub(crate) enum PrivateAudioReader {
    #[cfg(not(any(target_os = "macos", windows)))]
    File {
        file: File,
        position: u64,
        len: u64,
        _registered_lease: Option<RegisteredFileReaderLease>,
    },
    #[cfg(any(target_os = "macos", windows))]
    Sealed(crate::sealed_audio::SealedAudioReader),
}

impl PrivateAudioReader {
    #[cfg(all(not(unix), not(windows)))]
    fn from_file(file: File) -> std::io::Result<Self> {
        let len = file.metadata()?.len();
        Ok(Self::File {
            file,
            position: 0,
            len,
            _registered_lease: None,
        })
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    fn from_registered_file(audio: Arc<RegisteredFileAudio>) -> std::io::Result<Self> {
        let generation = {
            let mut state = audio
                .state
                .lock()
                .map_err(|_| std::io::Error::other("private audio generation lock poisoned"))?;
            if state.retired || !state.sealed || state.writer_active {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "private audio generation is not sealed for reading",
                ));
            }
            if state.active_readers >= MAX_ACTIVE_REGISTERED_PRIVATE_AUDIO_READERS {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "private audio reader count is exhausted",
                ));
            }
            state.active_readers += 1;
            state.generation
        };
        let lease = RegisteredFileReaderLease { audio, generation };
        let file = match lease.audio.file.try_clone() {
            Ok(file) => file,
            Err(error) => {
                drop(lease);
                return Err(error);
            }
        };
        let len = file.metadata()?.len();
        Ok(Self::File {
            file,
            position: 0,
            len,
            _registered_lease: Some(lease),
        })
    }
}

impl Read for PrivateAudioReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            #[cfg(not(any(target_os = "macos", windows)))]
            Self::File {
                file,
                position,
                _registered_lease,
                ..
            } => {
                if let Some(lease) = _registered_lease {
                    lease.verify()?;
                }
                #[cfg(unix)]
                let read = {
                    use std::os::unix::fs::FileExt;
                    file.read_at(buffer, *position)?
                };
                #[cfg(not(unix))]
                let read = {
                    file.seek(std::io::SeekFrom::Start(*position))?;
                    file.read(buffer)?
                };
                *position = position.saturating_add(read as u64);
                Ok(read)
            }
            #[cfg(any(target_os = "macos", windows))]
            Self::Sealed(reader) => reader.read(buffer),
        }
    }
}

impl Seek for PrivateAudioReader {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            #[cfg(not(any(target_os = "macos", windows)))]
            Self::File {
                position: current,
                len,
                ..
            } => {
                let next = match position {
                    std::io::SeekFrom::Start(offset) => i128::from(offset),
                    std::io::SeekFrom::End(offset) => i128::from(*len) + i128::from(offset),
                    std::io::SeekFrom::Current(offset) => i128::from(*current) + i128::from(offset),
                };
                if !(0..=i128::from(u64::MAX)).contains(&next) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "private audio seek is outside the capability",
                    ));
                }
                *current = next as u64;
                Ok(*current)
            }
            #[cfg(any(target_os = "macos", windows))]
            Self::Sealed(reader) => reader.seek(position),
        }
    }
}

#[cfg(test)]
fn read_registered_private_audio(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut reader = registered_private_audio_reader(path)?
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "capability missing"))?;
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(crate) fn private_audio_metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    let registry = private_audio_registry()
        .lock()
        .map_err(|_| std::io::Error::other("private audio registry lock poisoned"))?;
    match registry.get(path) {
        #[cfg(not(any(target_os = "macos", windows)))]
        Some(RegisteredPrivateAudio::File(audio)) => audio.file.metadata(),
        #[cfg(any(target_os = "macos", windows))]
        Some(RegisteredPrivateAudio::Sealed(audio)) => audio
            .upgrade()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "private audio owner retired")
            })?
            .metadata(),
        None if is_reserved_private_audio_path(path) => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "private audio capability is stale or forged",
        )),
        None => std::fs::metadata(path),
    }
}

pub(crate) fn private_audio_len(path: &Path) -> std::io::Result<u64> {
    let registry = private_audio_registry()
        .lock()
        .map_err(|_| std::io::Error::other("private audio registry lock poisoned"))?;
    match registry.get(path) {
        #[cfg(not(any(target_os = "macos", windows)))]
        Some(RegisteredPrivateAudio::File(audio)) => Ok(audio.file.metadata()?.len()),
        #[cfg(any(target_os = "macos", windows))]
        Some(RegisteredPrivateAudio::Sealed(audio)) => audio
            .upgrade()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "private audio owner retired")
            })?
            .len(),
        None if is_reserved_private_audio_path(path) => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "private audio capability is stale or forged",
        )),
        None => Ok(std::fs::metadata(path)?.len()),
    }
}

impl Drop for PrivateAudioRegistration {
    fn drop(&mut self) {
        retire_private_audio_registration(&self.path);
    }
}

/// Retained capability for a raw-audio temporary file.
///
/// Linux uses an anonymous file with no cleanup pathname. macOS and Windows
/// retain encrypted bytes behind an exact, non-inheritable parent-side handle.
/// Other platforms may use an owner-private temporary leaf when available.
pub(crate) enum PrivateAudioWriter {
    #[cfg(not(any(target_os = "macos", windows)))]
    File {
        file: File,
        _registered_lease: Option<RegisteredFileWriterLease>,
    },
    #[cfg(any(target_os = "macos", windows))]
    Sealed(crate::sealed_audio::SealedAudioWriter),
}

impl Write for PrivateAudioWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self {
            #[cfg(not(any(target_os = "macos", windows)))]
            Self::File {
                file,
                _registered_lease: Some(lease),
            } => {
                let state =
                    lease.audio.state.lock().map_err(|_| {
                        std::io::Error::other("private audio generation lock poisoned")
                    })?;
                if state.retired || !state.writer_active || state.generation != lease.generation {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "private audio writer generation was retired",
                    ));
                }
                file.write(bytes)
            }
            #[cfg(not(any(target_os = "macos", windows)))]
            Self::File {
                file,
                _registered_lease: None,
            } => file.write(bytes),
            #[cfg(any(target_os = "macos", windows))]
            Self::Sealed(writer) => writer.write(bytes),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            #[cfg(not(any(target_os = "macos", windows)))]
            Self::File {
                file,
                _registered_lease: Some(lease),
            } => {
                let state =
                    lease.audio.state.lock().map_err(|_| {
                        std::io::Error::other("private audio generation lock poisoned")
                    })?;
                if state.retired || !state.writer_active || state.generation != lease.generation {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "private audio writer generation was retired",
                    ));
                }
                file.flush()
            }
            #[cfg(not(any(target_os = "macos", windows)))]
            Self::File {
                file,
                _registered_lease: None,
            } => file.flush(),
            #[cfg(any(target_os = "macos", windows))]
            Self::Sealed(writer) => writer.flush(),
        }
    }
}

pub(crate) struct PrivateAudioTempFile {
    #[cfg(all(unix, not(target_os = "macos")))]
    file: File,
    #[cfg(any(target_os = "macos", windows))]
    sealed: crate::sealed_audio::SealedAudio,
    #[cfg(any(unix, windows))]
    processing_path: PathBuf,
    #[cfg(any(unix, windows))]
    _registration: PrivateAudioRegistration,
    #[cfg(all(not(unix), not(windows)))]
    file: tempfile::NamedTempFile,
    #[cfg(all(not(unix), not(windows)))]
    _directory: tempfile::TempDir,
}

/// Whether this platform has an exact private-audio capability that can be
/// handed to an out-of-process decoder without reopening a mutable leaf.
///
/// Linux uses an anonymous descriptor; macOS and Windows use an authenticated
/// ciphertext spool retained behind a non-inheritable parent-side handle.
pub const fn private_audio_processing_supported() -> bool {
    cfg!(any(target_os = "linux", target_os = "macos", windows))
}

/// Whether the configured pathname-only Parakeet CLI can consume an exact
/// anonymous private-audio capability on this platform.
///
/// The pathname-only CLI is deliberately disabled on every supported
/// platform. Linux can inherit an O_TMPFILE descriptor, but an ordinary
/// `execve` resets the child to dumpable and makes that descriptor race-openable
/// through `/proc/<pid>/fd` by a hostile same-UID process. macOS likewise must
/// not expose the sealed ciphertext backing or key. Parakeet remains on the
/// in-process Whisper fallback until it accepts bytes/stdin or participates in
/// an acknowledged post-exec descriptor-isolation protocol.
pub const fn parakeet_private_audio_transport_supported() -> bool {
    false
}

/// Whether the pathname-only Apple Speech helper can receive private audio
/// without publishing a named plaintext WAV.
///
/// It cannot today: the helper accepts only `--audio-path`. Keep retained
/// Apple Speech preferences on the sealed in-process Whisper path until the
/// helper has an exact byte/fd transport (minutes-hueo).
pub const fn apple_speech_private_audio_transport_supported() -> bool {
    false
}

pub const fn apple_speech_unavailable_reason() -> &'static str {
    "the Apple Speech helper cannot receive secure private audio yet"
}

/// One cross-surface answer to whether Parakeet can actually be selected.
///
/// Keeping the individual layers visible lets settings and health explain the
/// missing prerequisite without ever confusing "compiled" with "usable".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParakeetCapability {
    pub compiled: bool,
    pub platform_supported: bool,
    pub transport_supported: bool,
    pub runtime_available: bool,
    pub selectable: bool,
}

impl ParakeetCapability {
    /// Compose a capability from independently testable layers.
    pub const fn from_layers(
        compiled: bool,
        platform_supported: bool,
        transport_supported: bool,
        runtime_available: bool,
    ) -> Self {
        Self {
            compiled,
            platform_supported,
            transport_supported,
            runtime_available,
            selectable: compiled && platform_supported && transport_supported && runtime_available,
        }
    }

    /// Stable human-readable reason used by every user-facing surface.
    pub const fn unavailable_reason(self) -> &'static str {
        // The secure transport is the load-bearing blocker: rebuilding with
        // the feature enabled still cannot make the pathname-only helper safe.
        // Lead with that invariant so minimal and feature builds tell users
        // the same truth instead of implying that a rebuild is sufficient.
        if !self.transport_supported {
            "unavailable because the Parakeet process cannot receive secure private audio on this platform"
        } else if !self.compiled {
            "unavailable in this build"
        } else if !self.platform_supported {
            "unavailable on this platform"
        } else if !self.runtime_available {
            "unavailable because secure private-audio storage is not available at runtime"
        } else {
            "unavailable"
        }
    }
}

/// Resolve the actual Parakeet capability for this platform and runtime.
///
/// `compiled` is supplied by the caller because the CLI and desktop have
/// separate Cargo feature surfaces even though both share this policy.
pub fn parakeet_capability(compiled: bool) -> ParakeetCapability {
    ParakeetCapability::from_layers(
        compiled,
        private_audio_processing_supported(),
        parakeet_private_audio_transport_supported(),
        private_audio_processing_available(),
    )
}

#[cfg(test)]
thread_local! {
    static PRIVATE_AUDIO_PROCESSING_AVAILABLE_OVERRIDE: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

/// Override the runtime capability probe for one test thread. The scoped,
/// thread-local guard keeps parallel tests from changing production dispatch
/// decisions in neighboring tests.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn with_private_audio_processing_available_for_test<T>(
    available: bool,
    run: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<bool>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PRIVATE_AUDIO_PROCESSING_AVAILABLE_OVERRIDE.with(|slot| slot.set(self.0));
        }
    }

    PRIVATE_AUDIO_PROCESSING_AVAILABLE_OVERRIDE.with(|slot| {
        let restore = Restore(slot.replace(Some(available)));
        let result = run();
        drop(restore);
        result
    })
}

/// Probe the actual anonymous-audio primitive for the configured temp root.
/// Platform support alone is insufficient on Linux because the selected
/// filesystem may reject `O_TMPFILE`.
pub fn private_audio_processing_available() -> bool {
    #[cfg(test)]
    if let Some(available) = PRIVATE_AUDIO_PROCESSING_AVAILABLE_OVERRIDE.with(std::cell::Cell::get)
    {
        return available;
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    {
        PrivateAudioTempFile::new("minutes-private-audio-probe-", ".wav").is_ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        false
    }
}

/// A child-visible view of an exact private-audio capability.
///
/// Unix descriptor paths are meaningful only in a process that inherited the
/// corresponding descriptor. The lease owns a fresh read handle that remains
/// `FD_CLOEXEC` in the parent and must stay alive through the complete child
/// lifecycle. Only the selected child's `pre_exec` hook clears cloexec and
/// rewinds that handle; concurrent unrelated children cannot inherit it.
#[cfg(any(feature = "parakeet", all(test, target_os = "linux")))]
#[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
pub(crate) struct PrivateAudioChildLease {
    path: PathBuf,
    #[cfg(unix)]
    _inherited_read: File,
}

#[cfg(any(feature = "parakeet", all(test, target_os = "linux")))]
#[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
impl PrivateAudioChildLease {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Make only this exact command inherit the anonymous read descriptor.
    /// The parent descriptor remains `FD_CLOEXEC`, so concurrent unrelated
    /// spawns cannot enumerate or read raw meeting audio.
    pub(crate) fn configure_command(
        &self,
        command: &mut crate::bounded_child::BoundedCommand,
    ) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let fd = self._inherited_read.as_raw_fd();
            let current = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            if current < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if current & libc::FD_CLOEXEC == 0 {
                return Err(std::io::Error::other(
                    "private audio child descriptor was unexpectedly inheritable in the parent",
                ));
            }
            // SAFETY: this closure executes after fork and before exec; `fcntl`
            // and `lseek` are async-signal-safe and the captured descriptor is
            // a plain integer.
            unsafe {
                command.pre_exec(move || {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0
                        || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                        || libc::lseek(fd, 0, libc::SEEK_SET) < 0
                    {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
        }
        #[cfg(not(unix))]
        let _ = command;
        Ok(())
    }
}

#[cfg(all(not(unix), not(windows)))]
fn ensure_private_temp_directory(path: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path)?.is_dir() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "private audio temporary root is not a directory",
        ))
    }
}

#[cfg(all(not(unix), not(windows)))]
fn ensure_private_temp_file(path: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path)?.is_file() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "private audio temporary capability is not a regular file",
        ))
    }
}

impl PrivateAudioTempFile {
    pub(crate) fn new(prefix: &str, suffix: &str) -> std::io::Result<Self> {
        Self::new_in(&std::env::temp_dir(), prefix, suffix)
    }

    fn new_in(root: &Path, prefix: &str, suffix: &str) -> std::io::Result<Self> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let _ = prefix;
            let file = create_anonymous_audio_file(root)?;
            let registration = register_private_audio_file(&file, suffix)?;
            let processing_path = registration.path.clone();
            let temp = Self {
                file,
                processing_path,
                _registration: registration,
            };
            temp.verify_private_identity()?;
            Ok(temp)
        }

        #[cfg(any(target_os = "macos", windows))]
        {
            let _ = prefix;
            let sealed = crate::sealed_audio::SealedAudio::new_in(root)?;
            let registration = register_sealed_private_audio(&sealed, suffix)?;
            let processing_path = registration.path.clone();
            let temp = Self {
                sealed,
                processing_path,
                _registration: registration,
            };
            temp.verify_private_identity()?;
            Ok(temp)
        }

        #[cfg(all(not(unix), not(windows)))]
        {
            let directory = tempfile::Builder::new().prefix(prefix).tempdir_in(root)?;
            ensure_private_temp_directory(directory.path())?;
            let file = tempfile::Builder::new()
                .prefix("audio-")
                .suffix(suffix)
                .tempfile_in(directory.path())?;
            ensure_private_temp_file(file.path())?;
            let temp = Self {
                file,
                _directory: directory,
            };
            temp.verify_private_identity()?;
            Ok(temp)
        }
    }

    pub(crate) fn as_path(&self) -> &Path {
        #[cfg(any(unix, windows))]
        return &self.processing_path;

        #[cfg(all(not(unix), not(windows)))]
        return self.file.path();
    }

    /// Return the opaque process-local key for the retained handle. It is not
    /// a filesystem pathname; readers resolve it through the exact capability
    /// registry and decoder children receive a cloned fd or byte stream.
    pub(crate) fn processing_path(&self) -> PathBuf {
        #[cfg(any(unix, windows))]
        return self.processing_path.clone();

        #[cfg(all(not(unix), not(windows)))]
        return self.file.path().to_path_buf();
    }

    pub(crate) fn prepare_for_write(&mut self) -> std::io::Result<PrivateAudioWriter> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            registered_file_audio(&self.processing_path)?.prepare_writer()
        }
        #[cfg(any(target_os = "macos", windows))]
        {
            self.sealed.reset()?;
            Ok(PrivateAudioWriter::Sealed(self.sealed.writer()?))
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            let file = self.file.as_file_mut();
            file.set_len(0)?;
            file.rewind()?;
            Ok(PrivateAudioWriter::File {
                file: file.try_clone()?,
                _registered_lease: None,
            })
        }
    }

    pub(crate) fn finish_write(&mut self) -> std::io::Result<()> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            registered_file_audio(&self.processing_path)?.finish()?;
            self.verify_private_identity()
        }
        #[cfg(any(target_os = "macos", windows))]
        {
            self.sealed.finish()?;
            self.verify_private_identity()
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            let file = self.file.as_file_mut();
            file.flush()?;
            file.sync_all()?;
            file.rewind()?;
            self.verify_private_identity()
        }
    }

    pub(crate) fn try_clone_reader(&self) -> std::io::Result<PrivateAudioReader> {
        #[cfg(any(unix, windows))]
        {
            registered_private_audio_reader(&self.processing_path)?.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "private audio capability is not registered",
                )
            })
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            PrivateAudioReader::from_file(self.file.as_file().try_clone()?)
        }
    }

    /// Discard a failed child output generation. If reset itself cannot prove
    /// the destination empty (for example because a detached pipe worker still
    /// owns the writer lease), retire the opaque registry entry immediately so
    /// no caller can resolve the partial generation.
    fn discard_failed_write(&mut self) -> std::io::Result<()> {
        match self.prepare_for_write() {
            Ok(writer) => {
                drop(writer);
                Ok(())
            }
            Err(error) => {
                #[cfg(any(unix, windows))]
                retire_private_audio_registration(&self.processing_path);
                Err(std::io::Error::new(
                    error.kind(),
                    format!("private audio destination reset failed; capability retired: {error}"),
                ))
            }
        }
    }

    #[cfg(any(feature = "parakeet", all(test, target_os = "linux")))]
    pub(crate) fn child_lease(&self) -> std::io::Result<PrivateAudioChildLease> {
        #[cfg(target_os = "linux")]
        {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "pathname-only child cannot safely inherit private audio across exec on Linux",
            ))
        }

        #[cfg(target_os = "macos")]
        {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "pathname-only child cannot receive sealed private audio on macOS",
            ))
        }

        #[cfg(all(not(unix), not(windows)))]
        {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "pathname-only child private-audio transport is unavailable",
            ))
        }

        #[cfg(windows)]
        {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "pathname-only child cannot receive sealed private audio on Windows",
            ))
        }
    }

    pub(crate) fn verify_private_identity(&self) -> std::io::Result<()> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // The platform constructors and this verifier must share one
            // definition of an anonymous capability. Linux uses O_TMPFILE and
            // requires a regular 0600 zero-link inode. The opaque registry
            // must still resolve to this exact retained object before every
            // processing boundary.
            use std::os::unix::fs::MetadataExt;

            verify_anonymous_audio_file(&self.file)?;
            let registry = private_audio_registry()
                .lock()
                .map_err(|_| std::io::Error::other("private audio registry lock poisoned"))?;
            let registered = match registry.get(&self.processing_path) {
                Some(RegisteredPrivateAudio::File(file)) => file,
                _ => {
                    return Err(std::io::Error::other(
                        "private audio capability is not registered",
                    ))
                }
            };
            let held = self.file.metadata()?;
            let resolved = registered.file.metadata()?;
            if held.dev() != resolved.dev() || held.ino() != resolved.ino() {
                return Err(std::io::Error::other(
                    "private audio registry changed identity",
                ));
            }
            Ok(())
        }

        #[cfg(any(target_os = "macos", windows))]
        {
            self.sealed.verify()?;
            let registry = private_audio_registry()
                .lock()
                .map_err(|_| std::io::Error::other("private audio registry lock poisoned"))?;
            match registry.get(&self.processing_path) {
                Some(RegisteredPrivateAudio::Sealed(registered))
                    if registered
                        .upgrade()
                        .is_some_and(|audio| audio.same_backing(&self.sealed)) =>
                {
                    Ok(())
                }
                _ => Err(std::io::Error::other(
                    "sealed private audio registry changed identity",
                )),
            }
        }

        #[cfg(all(not(unix), not(windows)))]
        {
            let held = self.file.as_file().metadata()?;
            let live = std::fs::symlink_metadata(self.file.path())?;
            if !held.is_file() || !live.is_file() || live.file_type().is_symlink() {
                return Err(std::io::Error::other(
                    "private audio temp leaf is not a regular file",
                ));
            }
            Ok(())
        }
    }
}

enum PreparedTranscriptionInput {
    Original,
    Mixed(AuthorizedProcessAudioInput),
    SingleStem {
        path: std::path::PathBuf,
        recording_health: markdown::RecordingHealth,
    },
}

impl PreparedTranscriptionInput {
    fn processing_path(&self, original: &Path) -> PathBuf {
        match self {
            Self::Original => original.to_path_buf(),
            Self::Mixed(handle) => handle.processing_path().to_path_buf(),
            Self::SingleStem { path, .. } => path.clone(),
        }
    }

    fn format_extension(&self) -> Option<&'static str> {
        matches!(self, Self::Mixed(_)).then_some("wav")
    }

    fn internal_authority(&self) -> Option<&AuthorizedProcessAudioInput> {
        match self {
            Self::Mixed(input) => Some(input),
            Self::Original | Self::SingleStem { .. } => None,
        }
    }

    fn recording_health(&self) -> Option<&markdown::RecordingHealth> {
        match self {
            Self::SingleStem {
                recording_health, ..
            } => Some(recording_health),
            Self::Original | Self::Mixed(_) => None,
        }
    }

    fn diarization_audio_path(&self) -> Option<&Path> {
        match self {
            Self::SingleStem { path, .. } => Some(path),
            Self::Original | Self::Mixed(_) => None,
        }
    }
}

/// Dispatch a prepared source without erasing its authority. Ambient originals
/// and surviving named stems use the public pathname entry; an owned stem mix
/// stays bound to the typed private capability accepted by the authorized
/// coordinator entry. A caller-supplied proof authority takes precedence and
/// is independently revalidated by that entry.
fn transcribe_prepared_input_with_hints(
    prepared: &PreparedTranscriptionInput,
    original: &Path,
    input_authority: Option<&AuthorizedProcessAudioInput>,
    content_type: ContentType,
    config: &Config,
    decode_hints: crate::transcribe::DecodeHints,
) -> Result<crate::transcribe::TranscribeResult, crate::error::TranscribeError> {
    if let Some(authority) = input_authority.or_else(|| prepared.internal_authority()) {
        crate::transcription_coordinator::transcribe_authorized_path_for_content_with_hints(
            authority,
            content_type,
            config,
            decode_hints,
        )
    } else {
        crate::transcription_coordinator::transcribe_path_for_content_with_hints(
            &prepared.processing_path(original),
            content_type,
            config,
            decode_hints,
        )
    }
}

/// Prepare the input handed to the transcription coordinator, working around
/// the macOS 26 SCRecordingOutput dual-track `.mov` 2x decode bug (#234).
///
/// macOS SCRecordingOutput writes a `.mov` with two audio tracks (system + mic)
/// plus pristine `.voice.wav` and `.system.wav` PCM stems beside it. Decoding
/// the `.mov` for transcription produces audio at 2x real duration, so whisper
/// receives garbled samples and emits gibberish. When the stems are present and
/// valid, this helper mixes them into a 16kHz mono PCM via `ffmpeg amix` and
/// returns a prepared input for the caller to hand to the transcriber. When
/// both stems are usable, the mixed handle's `Drop` impl retires the anonymous
/// or sealed capability on success, Err, panic, or any future early-return;
/// no plaintext filesystem path is published. When exactly one
/// stem is usable, the surviving PCM is returned directly with explicit
/// degraded recording health; it is never deleted by this helper (#463).
///
/// Return contract:
/// - `Ok(Original)` — input does not need stem-mixing. Either it is not a `.mov`,
///   or it is a `.mov` with no sibling stems at all (treated as an ordinary
///   non-native-call container; the caller hands the original path to the
///   transcriber and accepts whatever the decoder does with it).
/// - `Ok(Mixed(handle))` — both native-call stems mixed cleanly.
/// - `Ok(SingleStem { .. })` — exactly one native-call stem contains signal;
///   transcribe it directly and surface the attached health warning.
/// - `Err(MinutesError::Transcribe(NativeCaptureStemMixUnavailable))` — input is
///   a native-call `.mov` whose sibling stem files exist but neither contains
///   usable signal, or whose two valid stems cannot be mixed. Falling back to
///   the broken `.mov` decode would re-enter the 2x bug, so that case remains a
///   clear failure.
///
/// Path handling: the `.mov` is canonicalized before stem lookup so a symlinked
/// recording resolves to its target before sibling lookup (#237 touched the
/// same area in the diarization path). Stem discovery, including the empty-stem
/// check via `stem_has_audio`, reuses [`crate::diarize::discover_stem_plan`] so
/// a single source of truth governs which side files count as "stems present".
#[cfg(test)]
fn prepare_transcription_input(
    audio_path: &Path,
) -> Result<PreparedTranscriptionInput, MinutesError> {
    prepare_transcription_input_with_format(audio_path, None)
}

fn prepare_transcription_input_with_format(
    audio_path: &Path,
    format_extension: Option<&str>,
) -> Result<PreparedTranscriptionInput, MinutesError> {
    // Only `.mov` containers can hit the 2x decode bug. Everything else is
    // either a clean PCM wav (Jake's manual reprocess flow), a single-stream
    // m4a/mp3/ogg (voice memos), or a format that does not exercise the
    // SCRecordingOutput dual-track path.
    let ext = format_extension.map(str::to_lowercase).or_else(|| {
        audio_path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
    });
    if ext.as_deref() != Some("mov") {
        return Ok(PreparedTranscriptionInput::Original);
    }

    // An explicit format belongs to a proof/descriptor-authorized input that
    // has already been copied into anonymous storage. Its anonymous fd path
    // is not a namespace from which sibling stems may be discovered. Recovery
    // jobs authorize and mix their exact stem members before reaching here.
    if format_extension.is_some() {
        return Ok(PreparedTranscriptionInput::Original);
    }

    // Canonicalize so a symlinked `.mov` resolves to its target before stem
    // lookup. Stems live next to the canonical file, not the symlink. Falls
    // back to the original path if canonicalize fails (symlink to a target
    // we cannot stat, permission denied, etc.); the stem discovery on the
    // next line will return None for any path it cannot read alongside.
    let canonical = audio_path
        .canonicalize()
        .unwrap_or_else(|_| audio_path.to_path_buf());

    // Stem discovery reuses the diarization helper so transcription and
    // diarization agree on what "stems present" means, including the
    // zero-byte-stem check via `stem_has_audio` that catches partial-crash
    // wavs (.exists() alone accepts them; `stem_has_audio` requires a valid
    // hound-readable header with non-zero sample/channel counts).
    let checked = crate::diarize::discover_stem_plan_checked(&canonical).map_err(|reason| {
        crate::error::TranscribeError::NativeCaptureStemMixUnavailable {
            reason: format!(
                "native call stem validation failed for {}: {reason}",
                canonical.display()
            ),
        }
    })?;
    let invalid_sibling = checked.invalid_sibling;

    let stems = match checked.plan {
        Some(crate::diarize::SourceAwareDiarizationPlan::FullStems(paths)) => paths,
        Some(crate::diarize::SourceAwareDiarizationPlan::SystemStemOnly(system)) => {
            tracing::warn!(
                audio = %canonical.display(),
                system = %system.display(),
                "microphone stem is missing or silent; transcribing surviving system stem"
            );
            let mut recording_health =
                crate::health::recording_health_for_native_call_stem_recovery(
                    crate::diarize::CaptureSource::System,
                );
            if let Some(invalid) = invalid_sibling.as_ref() {
                crate::health::append_native_call_invalid_stem_warning(
                    &mut recording_health,
                    invalid.source,
                    &invalid.reason,
                );
            }
            return Ok(PreparedTranscriptionInput::SingleStem {
                path: system,
                recording_health,
            });
        }
        Some(crate::diarize::SourceAwareDiarizationPlan::SilentSystemStem(paths)) => {
            tracing::warn!(
                audio = %canonical.display(),
                voice = %paths.voice.display(),
                system = %paths.system.display(),
                "system stem is missing or silent; transcribing surviving microphone stem"
            );
            let mut recording_health =
                crate::health::recording_health_for_native_call_stem_recovery(
                    crate::diarize::CaptureSource::Voice,
                );
            if let Some(invalid) = invalid_sibling.as_ref() {
                crate::health::append_native_call_invalid_stem_warning(
                    &mut recording_health,
                    invalid.source,
                    &invalid.reason,
                );
            }
            return Ok(PreparedTranscriptionInput::SingleStem {
                path: paths.voice,
                recording_health,
            });
        }
        None => {
            // `discover_stem_plan` returns None for two semantically different
            // cases: (a) neither stem present (ordinary non-native-call `.mov`
            // or a native capture whose stems were cleaned up) and (b) voice
            // stem present and audible but system stem file is entirely
            // absent from disk (`(true, false) && !system.exists()` branch of
            // discover_stem_plan at `diarize.rs:600-606`). Case (b) is a
            // native capture where the system side was lost during recording,
            // and falling through to the broken `.mov` decoder reproduces the
            // exact 2x-duration bug this helper exists to prevent. Codex
            // review of PR #235 v2 caught this.
            //
            // Distinguish by independently checking the sibling paths. A
            // usable voice stem is recoverable even when the system file is
            // absent. If either sibling exists but neither has signal, this
            // is definitely a native capture and must fail clearly instead
            // of decoding the known-broken dual-track `.mov`.
            if let Some(parent) = canonical.parent() {
                if let Some(stem_name) = canonical.file_stem().and_then(|s| s.to_str()) {
                    let voice = parent.join(format!("{}.voice.wav", stem_name));
                    let system = parent.join(format!("{}.system.wav", stem_name));
                    match crate::diarize::classify_stem_signal(&voice) {
                        crate::diarize::StemSignal::Signal => {
                            return Ok(PreparedTranscriptionInput::SingleStem {
                                path: voice,
                                recording_health:
                                    crate::health::recording_health_for_native_call_stem_recovery(
                                        crate::diarize::CaptureSource::Voice,
                                    ),
                            });
                        }
                        crate::diarize::StemSignal::Invalid(reason) => {
                            return Err(
                                crate::error::TranscribeError::NativeCaptureStemMixUnavailable {
                                    reason: format!(
                                        "native call voice stem validation failed for {}: {reason}",
                                        canonical.display()
                                    ),
                                }
                                .into(),
                            );
                        }
                        crate::diarize::StemSignal::Silence => {}
                    }
                    if voice.exists() || system.exists() {
                        return Err(crate::error::TranscribeError::NativeCaptureStemMixUnavailable {
                            reason: format!(
                                "native call capture at {} has no usable PCM stem: voice={}, system={}. Both stems are missing, empty, invalid, or digitally silent.",
                                canonical.display(),
                                voice.display(),
                                system.display()
                            ),
                        }
                        .into());
                    }
                }
            }
            // No usable stems at all. Could be a non-native-call `.mov`
            // (screen recording, downloaded file) or a native-call capture
            // whose stems were cleaned up; we cannot distinguish. Conservative:
            // treat as ordinary `.mov` and let the existing decoder handle
            // it. Hard-erroring on every stemless `.mov` would break
            // legitimate non-native-call use cases.
            return Ok(PreparedTranscriptionInput::Original);
        }
    };

    let mut raw_pcm =
        PrivateAudioTempFile::new("minutes-stem-mix-pcm-", ".s16le").map_err(|error| {
            crate::error::TranscribeError::NativeCaptureStemMixUnavailable {
                reason: format!(
                    "owner-private temp file could not be created for stem mix of {}: {}",
                    canonical.display(),
                    error
                ),
            }
        })?;

    // ffmpeg amix defaults: `duration=longest` is the framework default
    // (specifying it explicitly was redundant); `normalize=1` is the
    // default and prevents combined-amplitude clipping when one stem is
    // significantly louder than the other. System audio is usually
    // hotter than mic, so `normalize=0` could bake clipping into the
    // PCM before whisper sees it (jmh1313 confirmed the original PR's
    // recordings had stems at -0.0 and -0.1 dB peak, which would have
    // clipped on normalize=0). Default normalization is the safer
    // choice unless we add explicit weights with measured levels.
    let system_str = stems.system.to_str().ok_or_else(|| {
        crate::error::TranscribeError::NativeCaptureStemMixUnavailable {
            reason: format!(
                "system stem path is not valid UTF-8: {}",
                stems.system.display()
            ),
        }
    })?;
    let voice_str = stems.voice.to_str().ok_or_else(|| {
        crate::error::TranscribeError::NativeCaptureStemMixUnavailable {
            reason: format!(
                "voice stem path is not valid UTF-8: {}",
                stems.voice.display()
            ),
        }
    })?;
    let ffmpeg = crate::ffmpeg::resolve_ffmpeg().map_err(|error| {
        crate::error::TranscribeError::NativeCaptureStemMixUnavailable {
            reason: format!(
                "ffmpeg could not be resolved for stem mix of {}: {}",
                canonical.display(),
                error
            ),
        }
    })?;

    let mut command = crate::bounded_child::BoundedCommand::new(&ffmpeg);
    command.args([
        "-i",
        system_str,
        "-i",
        voice_str,
        "-filter_complex",
        "[0:a][1:a]amix=inputs=2",
        "-ac",
        "1",
        "-ar",
        "16000",
        "-c:a",
        "pcm_s16le",
        "-f",
        "s16le",
        "pipe:1",
    ]);
    let output = output_with_authorized_audio_stdin_to_private_file_with_budget(
        &mut command,
        None,
        &mut raw_pcm,
        crate::audio_budget::AudioWorkBudget::max_pcm_s16le_bytes(),
        crate::audio_budget::AUDIO_DECODE_DEADLINE,
    )
        .map_err(|e| {
            crate::error::TranscribeError::NativeCaptureStemMixUnavailable {
                reason: format!(
                    "ffmpeg could not be invoked for stem mix of {} via {}: {}. Install ffmpeg (brew install ffmpeg) or set MINUTES_FFMPEG.",
                    canonical.display(),
                    ffmpeg.display(),
                    e
                ),
            }
        })?;
    if !output.status.success() {
        let stderr_tail = String::from_utf8_lossy(&output.stderr);
        let last_line = stderr_tail
            .lines()
            .last()
            .unwrap_or("(no stderr)")
            .to_string();
        return Err(
            crate::error::TranscribeError::NativeCaptureStemMixUnavailable {
                reason: format!(
                    "ffmpeg amix failed for {} (voice={}, system={}): {}",
                    canonical.display(),
                    stems.voice.display(),
                    stems.system.display(),
                    last_line
                ),
            }
            .into(),
        );
    }
    let tmp = private_pcm_s16le_mono_to_wav(&raw_pcm, 16_000).map_err(|error| {
        crate::error::TranscribeError::NativeCaptureStemMixUnavailable {
            reason: format!(
                "mixed PCM could not be wrapped as a bounded WAV for {}: {}",
                canonical.display(),
                error
            ),
        }
    })?;
    tracing::info!(
        audio = %canonical.display(),
        mixed = %private_audio_diagnostic_label(tmp.as_path()),
        "using mixed stems instead of .mov for transcription (workaround for dual-track 2x bug)"
    );
    Ok(PreparedTranscriptionInput::Mixed(
        AuthorizedProcessAudioInput::from_internal_private_wav(tmp)?,
    ))
}

/// Wrap an exact signed-16-bit mono PCM capability in a canonical WAV without
/// ever publishing plaintext audio as a filesystem pathname.
///
/// FFmpeg cannot backfill a WAV header when stdout is a pipe, so its streamed
/// WAV muxer writes `0xffff_ffff` as the data length. Hound correctly rejects
/// that value because it is not divisible by the two-byte sample width. Keep
/// FFmpeg on bounded headerless PCM, then construct the exact header in this
/// trusted parent after the sealed raw length is known. The copy uses fixed,
/// zeroizing working memory and writes a fresh private generation; no encrypted
/// chunk is ever rewritten under the same key and nonce.
pub(crate) fn private_pcm_s16le_mono_to_wav(
    raw_pcm: &PrivateAudioTempFile,
    sample_rate: u32,
) -> std::io::Result<PrivateAudioTempFile> {
    const WAV_HEADER_BYTES: usize = 44;
    const CHANNELS: u16 = 1;
    const BITS_PER_SAMPLE: u16 = 16;
    const BLOCK_ALIGN: u16 = CHANNELS * (BITS_PER_SAMPLE / 8);

    if sample_rate == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "WAV sample rate must be non-zero",
        ));
    }
    let data_len = private_audio_len(raw_pcm.as_path())?;
    if data_len == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "mixed PCM output is empty",
        ));
    }
    if data_len % u64::from(BLOCK_ALIGN) != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "mixed PCM length is not aligned to a complete sample",
        ));
    }
    let data_len = u32::try_from(data_len).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "mixed PCM is too large for a canonical WAV container",
        )
    })?;
    if u64::from(data_len) > MAX_AUTHORIZED_PROCESS_AUDIO_BYTES - WAV_HEADER_BYTES as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "mixed PCM plus its WAV header exceeds the private-audio budget",
        ));
    }
    let riff_len = data_len.checked_add(36).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "mixed PCM exceeds the WAV container length limit",
        )
    })?;
    let byte_rate = sample_rate
        .checked_mul(u32::from(BLOCK_ALIGN))
        .ok_or_else(|| std::io::Error::other("WAV byte rate overflowed"))?;

    let mut header = [0_u8; WAV_HEADER_BYTES];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&riff_len.to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&1_u16.to_le_bytes());
    header[22..24].copy_from_slice(&CHANNELS.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&BLOCK_ALIGN.to_le_bytes());
    header[34..36].copy_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_len.to_le_bytes());

    let mut wav = PrivateAudioTempFile::new("minutes-pcm-wav-", ".wav")?;
    let write_result = (|| -> std::io::Result<()> {
        let mut writer = wav.prepare_for_write()?;
        writer.write_all(&header)?;

        let mut reader = raw_pcm.try_clone_reader()?;
        let mut buffer = Zeroizing::new(vec![0_u8; 32 * 1024]);
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            writer.write_all(&buffer[..read])?;
        }
        writer.flush()?;
        drop(writer);
        wav.finish_write()
    })();

    if let Err(error) = write_result {
        let _ = wav.discard_failed_write();
        return Err(error);
    }
    Ok(wav)
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
        &private_audio_diagnostic_label(audio_path),
        duration_ms,
        extra,
    );
    tracing::info!(
        audio = %private_audio_diagnostic_label(audio_path),
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
        &private_audio_diagnostic_label(audio_path),
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
    diagnostic_audio_path: &Path,
    allow_path_derived_audio: bool,
    content_type: ContentType,
    source: Option<&str>,
    config: &Config,
    trusted_attendees: &[String],
    llm_attendees: &[String],
    diarization_num_speakers: usize,
    diarization_from_stems: bool,
    degraded_ml_fallback: bool,
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
        let self_attribution = if allow_path_derived_audio {
            single_stem_speaker_self_attribution(
                audio_path,
                config,
                &voice_result,
                diarization_from_stems,
                &transcript,
                &transcript_labels,
                &already_mapped_labels,
            )
        } else {
            SelfAttributionOutcome::skipped(SelfAttributionSkippedReason::NoStems)
        };
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
            let log_file = diagnostic_audio_path.display().to_string();
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

        if degraded_ml_fallback {
            mark_degraded_ml_attributions(&mut speaker_map);
        }

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
// Transcription uses whisper-rs (whisper.cpp); ambient compressed formats use
// the bounded ffmpeg child, while private exact-byte processing admits WAV.
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
    /// Consent basis loaded from the record-start sidecar, if any.
    pub consent: Option<crate::markdown::ConsentBasis>,
    /// Exact disclosure text loaded from the record-start sidecar, if any.
    pub consent_notice: Option<String>,
    pub calendar_event: Option<crate::calendar::CalendarEvent>,
    pub recorded_at: Option<DateTime<Local>>,
    pub requested_title: Option<String>,
    pub recording_health: Option<crate::markdown::RecordingHealth>,
    /// Canonical context session for desktop and screen evidence captured
    /// alongside this recording.
    pub context_session_id: Option<String>,
    /// Optional template applied to summarization. Recorded in frontmatter
    /// so Phase 2 reprocessing knows which template produced this file.
    pub template: Option<crate::template::Template>,
}

#[derive(Clone, PartialEq, Eq)]
struct PrivateAudioAuthority(PathBuf);

#[derive(Clone, PartialEq, Eq)]
enum TranscriptAuthority {
    AmbientPath,
    AuthorizedCapability(PrivateAudioAuthority),
}

#[derive(Clone)]
pub struct TranscriptArtifact {
    pub write_result: WriteResult,
    pub frontmatter: Frontmatter,
    pub transcript: String,
    /// Private provenance prevents callers from downgrading an authorized
    /// artifact into the ambient-path enrichment path. Authorized enrichment
    /// additionally requires the live capability and re-verifies it at use.
    authority: TranscriptAuthority,
    /// Signal-verified surviving native-call stem selected during
    /// transcription. Diarization must reuse it rather than reopening the
    /// dual-track `.mov` that recovery deliberately bypassed.
    diarization_audio_path: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for TranscriptArtifact {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TranscriptArtifact")
            .field("status", &self.frontmatter.status)
            .field("transcript_bytes", &self.transcript.len())
            .field(
                "authority",
                &if self.is_descriptor_authorized() {
                    "authorized-capability"
                } else {
                    "ambient-path"
                },
            )
            .field(
                "has_diarization_audio",
                &self.diarization_audio_path.is_some(),
            )
            .finish()
    }
}

impl TranscriptArtifact {
    fn is_descriptor_authorized(&self) -> bool {
        matches!(self.authority, TranscriptAuthority::AuthorizedCapability(_))
    }

    #[cfg(test)]
    pub(crate) fn ambient_for_test(
        write_result: WriteResult,
        frontmatter: Frontmatter,
        transcript: String,
    ) -> Self {
        Self {
            write_result,
            frontmatter,
            transcript,
            authority: TranscriptAuthority::AmbientPath,
            diarization_audio_path: None,
        }
    }
}

fn resolve_screen_context_directory(
    _context_session_id: Option<&str>,
    audio_path: &Path,
    descriptor_authorized: bool,
) -> Option<PathBuf> {
    // The sealed-screenshot lifecycle is intentionally deferred to #510.
    // Until that complete authority lands, preserve current-main path
    // behavior for ordinary jobs and never attach ambient screen paths to a
    // descriptor-authorized processing request.
    (!descriptor_authorized).then(|| crate::screen::screens_dir_for(audio_path))
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

/// A private, byte-exact input capability for an authorized `process` run.
///
/// Proof-bound callers copy the proven source revision into storage that cannot
/// be replaced while the pipeline runs; pipeline-owned mixes bind their
/// retained private storage directly. Linux uses an anonymous file retained by
/// descriptor. macOS and Windows retain only authenticated ciphertext behind a
/// non-inheritable parent handle. Processing resolves the opaque registry
/// capability in-process; no platform creates a named plaintext staging copy.
#[allow(dead_code)] // Constructed by the restricted process-audio boundary in slice B.
pub struct AuthorizedProcessAudioInput {
    #[cfg(any(unix, windows))]
    private_audio: PrivateAudioTempFile,
    processing_path: PathBuf,
    format_extension: String,
}

/// Open a fresh parent-side handle when `audio_path` is an opaque registered
/// private-audio key. External decoders receive its bytes over stdin, never the
/// raw backing descriptor or a parent-addressable descriptor path.
pub(crate) fn authorized_audio_stdin(
    audio_path: &Path,
) -> std::io::Result<Option<PrivateAudioReader>> {
    registered_private_audio_reader(audio_path)
}

/// Run a decoder while streaming an authorized input through a one-way pipe.
/// A decoder grandchild may retain the pipe endpoint, but it never receives a
/// seekable raw-audio descriptor and cannot reopen the exhausted input.
#[allow(dead_code)] // Exercised by transport regressions and used by policy slice B.
pub(crate) fn output_with_authorized_audio_stdin(
    command: &mut crate::bounded_child::BoundedCommand,
    input: Option<PrivateAudioReader>,
) -> std::io::Result<std::process::Output> {
    #[cfg(target_os = "linux")]
    if input.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "authorized audio cannot cross a Linux child-process boundary",
        ));
    }
    let run = crate::bounded_child::run(
        command,
        input.map(|input| Box::new(input) as crate::bounded_child::StdinSource),
        crate::bounded_child::StdoutTarget::Capture {
            max_bytes: crate::bounded_child::DEFAULT_STDOUT_LIMIT,
        },
        crate::bounded_child::ChildBudget {
            wall_clock: std::time::Duration::from_secs(30 * 60),
            stderr_tail: MAX_PRIVATE_AUDIO_CHILD_STDERR_BYTES,
        },
    )?;
    if run.timed_out {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "authorized audio child exceeded its wall-clock budget",
        ));
    }
    Ok(run.output)
}

const MAX_PRIVATE_AUDIO_CHILD_STDERR_BYTES: usize = 256 * 1024;

/// Run an exact decoder child without granting it an output pathname.
///
/// Stdout is drained in bounded chunks directly into the retained private-file
/// handle while stderr is drained concurrently and retained only as a bounded
/// tail. Optional authorized input is likewise streamed over stdin. The child
/// never receives an output pathname. Unix has no temporary leaf at all; on a
/// named platform fallback, replacement can only make the final identity check
/// deny the result and cannot redirect the streamed bytes.
pub(crate) fn output_with_authorized_audio_stdin_to_private_file_with_budget(
    command: &mut crate::bounded_child::BoundedCommand,
    input: Option<PrivateAudioReader>,
    destination: &mut PrivateAudioTempFile,
    max_output_bytes: u64,
    wall_clock: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    #[cfg(target_os = "linux")]
    if input.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "authorized audio cannot cross a Linux child-process boundary",
        ));
    }
    let exact_output = destination.prepare_for_write()?;
    let run = crate::bounded_child::run(
        command,
        input.map(|input| Box::new(input) as crate::bounded_child::StdinSource),
        crate::bounded_child::StdoutTarget::ExactWriter {
            writer: Box::new(exact_output),
            max_bytes: max_output_bytes,
        },
        crate::bounded_child::ChildBudget {
            wall_clock,
            stderr_tail: MAX_PRIVATE_AUDIO_CHILD_STDERR_BYTES,
        },
    );
    let run = match run {
        Ok(run) if !run.timed_out => run,
        Ok(_) => {
            let timeout = std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "private audio child exceeded its wall-clock budget",
            );
            return match destination.discard_failed_write() {
                Ok(()) => Err(timeout),
                Err(retirement) => Err(std::io::Error::new(
                    timeout.kind(),
                    format!("{timeout}; {retirement}"),
                )),
            };
        }
        Err(error) => {
            return match destination.discard_failed_write() {
                Ok(()) => Err(error),
                Err(retirement) => {
                    Err(crate::bounded_child::with_context_preserving_spawn_failure(
                        error,
                        retirement.to_string(),
                    ))
                }
            };
        }
    };
    if run.output.status.success() {
        if let Err(error) = destination.finish_write() {
            return match destination.discard_failed_write() {
                Ok(()) => Err(error),
                Err(retirement) => Err(std::io::Error::new(
                    error.kind(),
                    format!("{error}; {retirement}"),
                )),
            };
        }
    } else {
        destination.discard_failed_write()?;
    }
    Ok(run.output)
}

fn reject_authorized_input(message: impl Into<String>) -> MinutesError {
    crate::error::TranscribeError::UnsupportedFormat(format!(
        "authorized process input rejected: {}",
        message.into()
    ))
    .into()
}

fn normalize_authorized_format(format_extension: &str) -> Result<String, MinutesError> {
    let format = format_extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    // The exact-byte authorization boundary currently admits only the WAV
    // parser, which is streaming and allocation-bounded. Symphonia 0.5.5
    // demuxers allocate attacker-controlled container table counts while
    // probing, before the decoder resource guard can run. Compressed/private
    // inputs must fail closed until a bounded demuxer or secure byte-streaming
    // child transport exists.
    if format != "wav" {
        return Err(reject_authorized_input(
            "private processing currently supports bounded WAV input only",
        ));
    }
    Ok(format)
}

#[cfg(unix)]
fn open_authorized_source(path: &Path) -> Result<File, MinutesError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() || metadata.nlink() != 1 {
        return Err(reject_authorized_input(
            "source must be a regular, single-link file",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
fn windows_file_identity(file: &File) -> Result<(u32, u64), MinutesError> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), info.as_mut_ptr()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let info = unsafe { info.assume_init() };
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(reject_authorized_input("reparse points are not allowed"));
    }
    if info.nNumberOfLinks != 1 {
        return Err(reject_authorized_input(
            "source must be a regular, single-link file",
        ));
    }
    Ok((
        info.dwVolumeSerialNumber,
        ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    ))
}

#[cfg(windows)]
fn open_authorized_source(path: &Path) -> Result<File, MinutesError> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(reject_authorized_input("source must be a regular file"));
    }
    windows_file_identity(&file)?;
    Ok(file)
}

fn copy_and_verify_authorized_bytes<W: Write>(
    source: File,
    destination: &mut W,
    expected_sha256: &str,
    expected_byte_length: u64,
) -> Result<(), MinutesError> {
    copy_and_verify_authorized_bytes_with_budget(
        source,
        destination,
        expected_sha256,
        expected_byte_length,
        MAX_AUTHORIZED_PROCESS_AUDIO_BYTES,
        AUTHORIZED_PROCESS_AUDIO_COPY_TIMEOUT,
    )
}

pub const MAX_AUTHORIZED_PROCESS_AUDIO_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const AUTHORIZED_PROCESS_AUDIO_COPY_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(120);

fn copy_and_verify_authorized_bytes_with_budget<W: Write>(
    mut source: File,
    destination: &mut W,
    expected_sha256: &str,
    expected_byte_length: u64,
    max_bytes: u64,
    timeout: std::time::Duration,
) -> Result<(), MinutesError> {
    copy_and_verify_authorized_bytes_from_reader(
        &mut source,
        destination,
        expected_sha256,
        expected_byte_length,
        max_bytes,
        timeout,
    )
}

fn copy_and_verify_authorized_bytes_from_reader<W: Write>(
    source: &mut File,
    destination: &mut W,
    expected_sha256: &str,
    expected_byte_length: u64,
    max_bytes: u64,
    timeout: std::time::Duration,
) -> Result<(), MinutesError> {
    copy_authorized_bytes_from_reader(
        source,
        destination,
        Some(expected_sha256),
        expected_byte_length,
        max_bytes,
        timeout,
    )
}

fn copy_authorized_bytes_from_reader<W: Write>(
    source: &mut File,
    destination: &mut W,
    expected_sha256: Option<&str>,
    expected_byte_length: u64,
    max_bytes: u64,
    timeout: std::time::Duration,
) -> Result<(), MinutesError> {
    let normalized_sha = expected_sha256.map(|value| value.trim().to_ascii_lowercase());
    if normalized_sha
        .as_ref()
        .is_some_and(|sha| sha.len() != 64 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(reject_authorized_input("invalid SHA-256 proof"));
    }

    if expected_byte_length > max_bytes || source.metadata()?.len() > max_bytes {
        return Err(reject_authorized_input(
            "audio copy resource budget exceeded",
        ));
    }

    let started = std::time::Instant::now();
    let mut digest = Sha256::new();
    let mut byte_length = 0_u64;
    let mut chunk = Zeroizing::new([0_u8; 256 * 1024]);
    loop {
        if started.elapsed() >= timeout {
            return Err(reject_authorized_input(
                "audio copy resource budget exceeded",
            ));
        }
        let read = source.read(chunk.as_mut())?;
        if read == 0 {
            break;
        }
        byte_length = byte_length
            .checked_add(read as u64)
            .ok_or_else(|| reject_authorized_input("source length overflowed"))?;
        if byte_length > expected_byte_length || byte_length > max_bytes {
            return Err(reject_authorized_input("byte length did not match proof"));
        }
        digest.update(&chunk[..read]);
        destination.write_all(&chunk[..read])?;
    }

    if started.elapsed() >= timeout {
        return Err(reject_authorized_input(
            "audio copy resource budget exceeded",
        ));
    }

    let actual_sha = format!("{:x}", digest.finalize());
    if byte_length != expected_byte_length
        || normalized_sha
            .as_ref()
            .is_some_and(|expected| actual_sha != *expected)
    {
        return Err(reject_authorized_input(
            "source bytes did not match the final authorization proof",
        ));
    }
    destination.flush()?;
    Ok(())
}

#[allow(dead_code)] // The processor entry point lands in the dependent policy slice B.
impl AuthorizedProcessAudioInput {
    /// Bind a completed pipeline-owned WAV to the same typed authority used by
    /// proof-bound external inputs. This constructor is deliberately private:
    /// callers cannot turn an opaque token string into authorized behavior.
    fn from_internal_private_wav(
        private_audio: PrivateAudioTempFile,
    ) -> Result<Self, MinutesError> {
        private_audio.verify_private_identity()?;
        let processing_path = private_audio.processing_path();
        if private_audio_len(&processing_path)? == 0 {
            return Err(reject_authorized_input(
                "retained internal audio capability is empty",
            ));
        }
        Ok(Self {
            private_audio,
            processing_path,
            format_extension: "wav".into(),
        })
    }

    pub(crate) fn from_proof(
        source_path: &Path,
        expected_sha256: &str,
        expected_byte_length: u64,
        original_format_extension: &str,
    ) -> Result<Self, MinutesError> {
        if expected_byte_length > MAX_AUTHORIZED_PROCESS_AUDIO_BYTES {
            return Err(reject_authorized_input(
                "audio copy resource budget exceeded",
            ));
        }
        let format_extension = normalize_authorized_format(original_format_extension)?;
        let source = open_authorized_source(source_path)?;
        if source.metadata()?.len() > MAX_AUTHORIZED_PROCESS_AUDIO_BYTES {
            return Err(reject_authorized_input(
                "audio copy resource budget exceeded",
            ));
        }

        #[cfg(any(unix, windows))]
        {
            let mut private_audio = PrivateAudioTempFile::new(
                "minutes-authorized-process-",
                &format!(".{format_extension}"),
            )?;
            {
                let mut writer = private_audio.prepare_for_write()?;
                copy_and_verify_authorized_bytes(
                    source,
                    &mut writer,
                    expected_sha256,
                    expected_byte_length,
                )?;
            }
            private_audio.finish_write()?;
            let processing_path = private_audio.processing_path();
            Ok(Self {
                private_audio,
                processing_path,
                format_extension,
            })
        }
    }

    pub(crate) fn verify_pipeline_binding(&self) -> Result<(), MinutesError> {
        #[cfg(any(unix, windows))]
        {
            self.private_audio.verify_private_identity()?;
            if private_audio_len(&self.processing_path)? == 0 {
                return Err(reject_authorized_input(
                    "retained audio capability is empty",
                ));
            }
        }

        Ok(())
    }

    pub(crate) fn processing_path(&self) -> &Path {
        &self.processing_path
    }

    pub(crate) fn format_extension(&self) -> &str {
        &self.format_extension
    }
}

#[derive(Clone, Copy, Default)]
struct ProcessOptions<'a> {
    sidecar: Option<&'a SidecarMetadata>,
    template: Option<&'a crate::template::Template>,
    input_authority: Option<&'a AuthorizedProcessAudioInput>,
}

#[cfg(test)]
mod authorized_process_input_tests {
    use super::*;

    fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn descriptor_authority_never_discovers_an_ambient_screen_directory() {
        let audio_path = Path::new("/synthetic/meeting.wav");
        assert!(resolve_screen_context_directory(None, audio_path, true).is_none());
        assert_eq!(
            resolve_screen_context_directory(None, audio_path, false),
            Some(crate::screen::screens_dir_for(audio_path))
        );
    }

    #[test]
    fn descriptor_authority_survives_transcript_artifact_enrichment() {
        let dir = tempfile::TempDir::new().unwrap();
        let unique = dir
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap();
        let audio_path = dir.path().join(format!("authorized-{unique}.wav"));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&audio_path, spec).unwrap();
        for _ in 0..1_600 {
            writer.write_sample(0_i16).unwrap();
        }
        writer.finalize().unwrap();
        let audio_bytes = std::fs::read(&audio_path).unwrap();
        let input = AuthorizedProcessAudioInput::from_proof(
            &audio_path,
            &sha256(&audio_bytes),
            audio_bytes.len() as u64,
            "wav",
        )
        .unwrap();

        // Poison the ambient pathname namespace with sibling stems.
        // Descriptor-authorized enrichment must leave them untouched and must
        // not derive output behavior from them.
        let stem = audio_path.file_stem().unwrap().to_string_lossy();
        let voice_stem = dir.path().join(format!("{stem}.voice.wav"));
        let system_stem = dir.path().join(format!("{stem}.system.wav"));
        std::fs::write(&voice_stem, b"ambient voice stem canary").unwrap();
        std::fs::write(&system_stem, b"ambient system stem canary").unwrap();
        let mut config = Config {
            output_dir: dir.path().join("meetings"),
            ..Config::default()
        };
        config.transcription.min_words = 1;
        config.summarization.engine = "none".into();
        config.diarization.engine = "none".into();
        config.screen_context.keep_after_summary = false;
        let context = BackgroundPipelineContext::default();

        // This is the transcribe/write seam. The private artifact provenance
        // is bound to this exact retained capability token.
        let artifact = write_transcript_artifact_with_authority(
            input.processing_path(),
            ContentType::Meeting,
            Some("Authorized lifecycle"),
            &config,
            &context,
            None,
            "[0:00] We confirmed the proof-bound processing boundary.\n".into(),
            crate::transcribe::FilterStats::default(),
            0,
            TranscriptAuthority::AuthorizedCapability(PrivateAudioAuthority(
                input.processing_path().to_path_buf(),
            )),
        )
        .unwrap();
        assert!(artifact.is_descriptor_authorized());
        let rendered = format!("{artifact:?}");
        assert!(!rendered.contains(input.processing_path().to_string_lossy().as_ref()));
        assert!(!rendered.contains("proof-bound processing boundary"));
        let ordinary_process = process(
            input.processing_path(),
            ContentType::Meeting,
            Some("Bearer replay"),
            &config,
        )
        .expect_err("ordinary process must reject a live private token");
        assert!(ordinary_process
            .to_string()
            .contains("typed authorized entry point"));
        let ordinary_transcribe = crate::transcribe::transcribe_with_hints(
            input.processing_path(),
            &config,
            &crate::transcribe::DecodeHints::default(),
        )
        .expect_err("ordinary transcribe must reject a live private token");
        assert!(ordinary_transcribe
            .to_string()
            .contains("typed authorized entry point"));
        assert!(crate::diarize::audio_duration_secs(input.processing_path()).is_err());
        assert!(crate::diarize::audio_duration_secs_authorized(&input).is_ok());
        assert!(matches!(
            crate::diarize::diarize_with_context(
                input.processing_path(),
                &config,
                crate::diarize::DiarizationContext {
                    purpose: crate::diarize::DiarizationPurpose::Auxiliary,
                    transcript_windows: None,
                },
            ),
            crate::diarize::DiarizationOutcome::NotConfigured
        ));
        assert!(resolve_screen_context_directory(
            context.context_session_id.as_deref(),
            &audio_path,
            artifact.is_descriptor_authorized(),
        )
        .is_none());

        let ordinary_error =
            enrich_transcript_artifact(&audio_path, &artifact, &config, &context, |_| {})
                .expect_err("authorized artifact must not enter ambient enrichment");
        assert!(ordinary_error.to_string().contains("retained capability"));

        let other_path = dir.path().join("unrelated.wav");
        std::fs::write(&other_path, &audio_bytes).unwrap();
        let other = AuthorizedProcessAudioInput::from_proof(
            &other_path,
            &sha256(&audio_bytes),
            audio_bytes.len() as u64,
            "wav",
        )
        .unwrap();
        let substitution_error =
            enrich_transcript_artifact_authorized(&other, &artifact, &config, &context, |_| {})
                .expect_err("a different retained capability must not authorize enrichment");
        assert!(substitution_error.to_string().contains("does not match"));

        let result =
            enrich_transcript_artifact_authorized(&input, &artifact, &config, &context, |_| {})
                .unwrap();
        let written = std::fs::read_to_string(&result.path).unwrap();
        assert!(!written.contains(&audio_path.display().to_string()));
        assert!(!written.contains("Retry audio"));
        assert!(!written.contains("minutes process"));
        assert_eq!(
            std::fs::read(&voice_stem).unwrap(),
            b"ambient voice stem canary"
        );
        assert_eq!(
            std::fs::read(&system_stem).unwrap(),
            b"ambient system stem canary"
        );
        // No-speech artifacts normally include a retry command. The authority
        // bit must suppress it at the initial transcript write as well as on
        // later rewrites, so neither an ambient nor opaque path is disclosed.
        config.transcription.min_words = 100;
        let no_speech = write_transcript_artifact_with_authority(
            input.processing_path(),
            ContentType::Meeting,
            Some("Authorized no speech"),
            &config,
            &context,
            None,
            "[0:00] brief\n".into(),
            crate::transcribe::FilterStats::default(),
            0,
            TranscriptAuthority::AuthorizedCapability(PrivateAudioAuthority(
                input.processing_path().to_path_buf(),
            )),
        )
        .unwrap();
        assert_eq!(no_speech.frontmatter.status, Some(OutputStatus::NoSpeech));
        enrich_transcript_artifact_authorized(&input, &no_speech, &config, &context, |_| {})
            .unwrap();
        let written = std::fs::read_to_string(&no_speech.write_result.path).unwrap();
        assert!(!written.contains(&audio_path.display().to_string()));
        assert!(!written.contains("Retry audio"));
        assert!(!written.contains("minutes process"));
    }

    #[test]
    fn authorized_compressed_container_fails_before_private_copy_or_probe() {
        let dir = tempfile::TempDir::new().unwrap();
        let source = dir.path().join("hostile.m4a");
        let bytes = b"synthetic hostile container metadata";
        std::fs::write(&source, bytes).unwrap();
        let error = match AuthorizedProcessAudioInput::from_proof(
            &source,
            &sha256(bytes),
            bytes.len() as u64,
            "m4a",
        ) {
            Ok(_) => panic!("compressed private input must fail before copy and demux probing"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("bounded WAV input only"));

        let disguised = AuthorizedProcessAudioInput::from_proof(
            &source,
            &sha256(bytes),
            bytes.len() as u64,
            "wav",
        )
        .unwrap();
        let config = Config::default();
        let decode_error = crate::transcribe::transcribe_authorized_with_hints(
            &disguised,
            ContentType::Memo,
            &config,
            &crate::transcribe::DecodeHints::default().with_audio_format_extension("wav"),
        )
        .expect_err("content disguised as WAV must stay on the bounded WAV parser");
        assert!(!decode_error
            .to_string()
            .contains(disguised.processing_path().to_string_lossy().as_ref()));
    }

    #[test]
    fn authorized_input_consumes_retained_original_after_source_replacement() {
        let dir = tempfile::TempDir::new().unwrap();
        let source = dir.path().join("source.wav");
        let original = b"synthetic authorized original revision";
        std::fs::write(&source, original).unwrap();

        let input = AuthorizedProcessAudioInput::from_proof(
            &source,
            &sha256(original),
            original.len() as u64,
            "wav",
        )
        .unwrap();
        assert!(is_reserved_private_audio_path(&input.processing_path));
        assert!(
            !input.processing_path.exists(),
            "authorized processing must expose only an opaque registry token"
        );
        std::fs::write(&source, b"replacement revision after proof").unwrap();

        assert_eq!(
            read_registered_private_audio(&input.processing_path).unwrap(),
            original
        );
        input.verify_pipeline_binding().unwrap();
    }

    #[test]
    fn authorized_pipeline_entry_revalidates_the_retained_capability() {
        let dir = tempfile::TempDir::new().unwrap();
        let source = dir.path().join("source.wav");
        let bytes = b"synthetic authorized audio";
        std::fs::write(&source, bytes).unwrap();
        let mut input = AuthorizedProcessAudioInput::from_proof(
            &source,
            &sha256(bytes),
            bytes.len() as u64,
            "wav",
        )
        .unwrap();
        input.processing_path = PathBuf::from("minutes-private-audio://missing-authority.wav");

        let error = process_with_template_authorized(
            &input,
            ContentType::Memo,
            None,
            &Config::default(),
            None,
            |_| {},
        )
        .expect_err("pipeline must verify the capability before any processing");
        assert!(
            matches!(&error, MinutesError::Io(io) if io.kind() == std::io::ErrorKind::NotFound),
            "unexpected pre-processing error: {error}"
        );
    }

    #[test]
    fn authorized_input_rejects_replacement_and_in_place_mutation_after_proof() {
        let dir = tempfile::TempDir::new().unwrap();
        let source = dir.path().join("source.wav");
        let displaced = dir.path().join("source.displaced.wav");
        let original = b"synthetic proof revision";
        std::fs::write(&source, original).unwrap();
        let proof = sha256(original);

        std::fs::rename(&source, &displaced).unwrap();
        std::fs::write(&source, b"synthetic replacement!!").unwrap();
        let replacement_error =
            AuthorizedProcessAudioInput::from_proof(&source, &proof, original.len() as u64, "wav")
                .err()
                .expect("replacement must not satisfy the original proof");
        assert!(replacement_error.to_string().contains("proof"));

        std::fs::write(&source, original).unwrap();
        let mut mutated = original.to_vec();
        mutated[0] ^= 0x01;
        std::fs::write(&source, mutated).unwrap();
        let mutation_error =
            AuthorizedProcessAudioInput::from_proof(&source, &proof, original.len() as u64, "wav")
                .err()
                .expect("in-place mutation must not satisfy the original proof");
        assert!(mutation_error.to_string().contains("proof"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn authorized_input_is_rejected_before_either_linux_child_can_spawn() {
        let dir = tempfile::TempDir::new().unwrap();
        let source = dir.path().join("source.wav");
        let marker = dir.path().join("child-spawned");
        let original = b"synthetic descriptor inheritance revision";
        std::fs::write(&source, original).unwrap();
        let input = AuthorizedProcessAudioInput::from_proof(
            &source,
            &sha256(original),
            original.len() as u64,
            "wav",
        )
        .unwrap();

        let mut exact_child = crate::bounded_child::BoundedCommand::new("/bin/sh");
        exact_child.args([
            "-c",
            "printf spawned > \"$1\"; /bin/cat",
            "authorized-child",
        ]);
        exact_child.arg(&marker);
        let authorized_input = authorized_audio_stdin(&input.processing_path).unwrap();
        let error = output_with_authorized_audio_stdin(&mut exact_child, authorized_input)
            .expect_err("Linux must reject authorized input before spawn");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            !marker.exists(),
            "rejected child must never create its marker"
        );

        let mut destination = PrivateAudioTempFile::new("minutes-no-spawn-", ".wav").unwrap();
        let mut output_child = crate::bounded_child::BoundedCommand::new("/bin/sh");
        output_child.args([
            "-c",
            "printf spawned > \"$1\"; /bin/cat",
            "authorized-output-child",
        ]);
        output_child.arg(&marker);
        let authorized_input = authorized_audio_stdin(&input.processing_path).unwrap();
        let error = output_with_authorized_audio_stdin_to_private_file_with_budget(
            &mut output_child,
            authorized_input,
            &mut destination,
            1024,
            std::time::Duration::from_secs(5),
        )
        .expect_err("Linux must reject authorized input before output-child spawn");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            !marker.exists(),
            "rejected child must never create its marker"
        );
    }

    #[test]
    #[cfg(unix)]
    fn private_audio_temp_creation_has_no_named_leaf() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::TempDir::new().unwrap();
        let before = std::fs::read_dir(dir.path()).unwrap().count();
        let mut private = PrivateAudioTempFile::new_in(dir.path(), "ignored-", ".wav").unwrap();
        let after = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(before, after);
        assert_eq!(
            private_audio_metadata(private.as_path()).unwrap().nlink(),
            0
        );
        private
            .prepare_for_write()
            .unwrap()
            .write_all(b"synthetic anonymous audio")
            .unwrap();
        private.finish_write().unwrap();
        assert_eq!(
            read_registered_private_audio(&private.processing_path()).unwrap(),
            b"synthetic anonymous audio"
        );
        private.verify_private_identity().unwrap();
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn registered_private_audio_readers_are_bounded_and_release_their_lease() {
        let mut private = PrivateAudioTempFile::new("minutes-reader-cap-", ".wav").unwrap();
        private
            .prepare_for_write()
            .unwrap()
            .write_all(b"synthetic bounded reader audio")
            .unwrap();
        private.finish_write().unwrap();
        let path = private.processing_path();
        let mut readers = (0..MAX_ACTIVE_REGISTERED_PRIVATE_AUDIO_READERS)
            .map(|_| registered_private_audio_reader(&path).unwrap().unwrap())
            .collect::<Vec<_>>();
        let error = match registered_private_audio_reader(&path) {
            Ok(_) => panic!("registered private audio readers must remain bounded"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        readers.pop();
        assert!(registered_private_audio_reader(&path).unwrap().is_some());

        drop(readers);
        let mut owner_readers = (0..MAX_ACTIVE_REGISTERED_PRIVATE_AUDIO_READERS)
            .map(|_| private.try_clone_reader().unwrap())
            .collect::<Vec<_>>();
        let error = match private.try_clone_reader() {
            Ok(_) => panic!("owner-side readers must share the registry lease cap"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        owner_readers.pop();
        assert!(private.try_clone_reader().is_ok());
    }

    #[test]
    fn explicit_authorized_mov_format_never_discovers_ambient_sibling_stems() {
        let dir = tempfile::TempDir::new().unwrap();
        let anonymous_view = dir.path().join("authorized.mov");
        std::fs::write(&anonymous_view, b"proof-bound primary").unwrap();
        let voice = dir.path().join("authorized.voice.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&voice, spec).unwrap();
        for _ in 0..1_600 {
            writer.write_sample(4_000_i16).unwrap();
        }
        writer.finalize().unwrap();

        assert!(matches!(
            prepare_transcription_input_with_format(&anonymous_view, Some("mov")).unwrap(),
            PreparedTranscriptionInput::Original
        ));
    }

    #[test]
    #[cfg(all(unix, not(target_os = "linux")))]
    fn authorized_audio_pipe_drives_a_real_ffmpeg_decoder() {
        let Ok(ffmpeg) = crate::ffmpeg::resolve_ffmpeg() else {
            return;
        };
        let dir = tempfile::TempDir::new().unwrap();
        let source = dir.path().join("source.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&source, spec).unwrap();
        for _ in 0..1_600 {
            writer.write_sample(0_i16).unwrap();
        }
        writer.finalize().unwrap();
        let bytes = std::fs::read(&source).unwrap();
        let input = AuthorizedProcessAudioInput::from_proof(
            &source,
            &sha256(&bytes),
            bytes.len() as u64,
            "wav",
        )
        .unwrap();

        let mut command = crate::bounded_child::BoundedCommand::new(ffmpeg);
        command.args(["-v", "error", "-i", "pipe:0", "-f", "null", "-"]);
        let authorized_input = authorized_audio_stdin(&input.processing_path).unwrap();
        let output = output_with_authorized_audio_stdin(&mut command, authorized_input).unwrap();
        assert!(
            output.status.success(),
            "ffmpeg rejected authorized stdin: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn authorized_copy_rejects_sparse_and_expired_resource_budgets() {
        let dir = tempfile::TempDir::new().unwrap();
        let source_path = dir.path().join("PRIVATE_SPARSE_CANARY.wav");
        let source = File::create(&source_path).unwrap();
        source.set_len(1_025).unwrap();
        drop(source);
        let mut destination = tempfile::tempfile().unwrap();
        let failure = copy_and_verify_authorized_bytes_with_budget(
            File::open(&source_path).unwrap(),
            &mut destination,
            &"0".repeat(64),
            1_025,
            1_024,
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(failure
            .to_string()
            .contains("audio copy resource budget exceeded"));
        assert!(!failure.to_string().contains("PRIVATE_SPARSE_CANARY"));

        let small_path = dir.path().join("small.wav");
        std::fs::write(&small_path, b"small").unwrap();
        let mut destination = tempfile::tempfile().unwrap();
        let expired = copy_and_verify_authorized_bytes_with_budget(
            File::open(&small_path).unwrap(),
            &mut destination,
            &sha256(b"small"),
            5,
            10,
            std::time::Duration::ZERO,
        )
        .unwrap_err();
        assert!(expired.to_string().contains("resource budget exceeded"));
    }
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
        ProcessOptions {
            sidecar,
            ..ProcessOptions::default()
        },
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
    process_with_progress_and_sidecar(
        audio_path,
        content_type,
        title,
        config,
        ProcessOptions::default(),
        on_progress,
    )
}

/// Process an audio file with an optional template applied to summarization.
/// The template's slug is recorded in the meeting's frontmatter so a Phase 2
/// reprocessor can identify which template produced the file.
pub fn process_with_template<F>(
    audio_path: &Path,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
    sidecar: Option<&SidecarMetadata>,
    template: Option<&crate::template::Template>,
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
        ProcessOptions {
            sidecar,
            template,
            ..ProcessOptions::default()
        },
        on_progress,
    )
}

/// Process only the exact bytes retained by an authorization capability.
///
/// Keeping the capability itself in the call graph makes it impossible for a
/// caller to obtain descriptor-authorized behavior by supplying a boolean or
/// format string beside an unrelated path.
#[allow(dead_code)] // Public bridge is enabled by the dependent policy slice B.
pub(crate) fn process_with_template_authorized<F>(
    input: &AuthorizedProcessAudioInput,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
    template: Option<&crate::template::Template>,
    on_progress: F,
) -> Result<WriteResult, MinutesError>
where
    F: FnMut(PipelineStage),
{
    input.verify_pipeline_binding()?;
    process_with_progress_and_sidecar(
        input.processing_path(),
        content_type,
        title,
        config,
        ProcessOptions {
            template,
            input_authority: Some(input),
            ..ProcessOptions::default()
        },
        on_progress,
    )
}

pub fn transcribe_to_artifact(
    audio_path: &Path,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
    context: &BackgroundPipelineContext,
    existing_output_path: Option<&Path>,
) -> Result<TranscriptArtifact, MinutesError> {
    transcribe_to_artifact_with_authority(
        audio_path,
        content_type,
        title,
        config,
        context,
        existing_output_path,
        None,
    )
}

#[allow(dead_code)] // Background bridge is enabled by the dependent policy slice B.
pub(crate) fn transcribe_to_artifact_authorized(
    input: &AuthorizedProcessAudioInput,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
    context: &BackgroundPipelineContext,
    existing_output_path: Option<&Path>,
) -> Result<TranscriptArtifact, MinutesError> {
    input.verify_pipeline_binding()?;
    transcribe_to_artifact_with_authority(
        input.processing_path(),
        content_type,
        title,
        config,
        context,
        existing_output_path,
        Some(input),
    )
}

#[allow(clippy::too_many_arguments)]
fn transcribe_to_artifact_with_authority(
    audio_path: &Path,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
    context: &BackgroundPipelineContext,
    existing_output_path: Option<&Path>,
    input_authority: Option<&AuthorizedProcessAudioInput>,
) -> Result<TranscriptArtifact, MinutesError> {
    if input_authority.is_none() && is_reserved_private_audio_path(audio_path) {
        return Err(reject_authorized_input(
            "private audio tokens require the typed authorized entry point",
        ));
    }
    if let Some(input) = input_authority {
        input.verify_pipeline_binding()?;
    }
    let audio_path = input_authority
        .map(AuthorizedProcessAudioInput::processing_path)
        .unwrap_or(audio_path);
    let authorized_format = input_authority.map(AuthorizedProcessAudioInput::format_extension);
    let metadata = private_audio_metadata(audio_path)?;
    if private_audio_len(audio_path)? == 0 {
        return Err(crate::error::TranscribeError::EmptyAudio.into());
    }
    let recording_date =
        infer_recording_date(context.recorded_at, context.sidecar.as_ref(), &metadata);

    if authorized_format.is_none() {
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
    let mut decode_hints = build_decode_hints(
        title,
        calendar_event_title.as_deref(),
        context.pre_context.as_deref(),
        &attendees,
        Some(&config.identity),
        load_vocabulary_for_decode_hints().as_ref(),
    );
    if let Some(format) = authorized_format {
        decode_hints = decode_hints.with_audio_format_extension(format);
    }

    // Apply the same stem-mix workaround as `process_with_progress_and_sidecar`
    // so background-job and `minutes process` callers (which reach the
    // pipeline via `transcribe_to_artifact` and bypass the foreground
    // entry point) are not exposed to the macOS 26 dual-track `.mov` 2x
    // bug. A mixed temp handle cleans itself up; a single surviving stem is
    // borrowed in place and contributes explicit recording health (#463).
    let prepared_input = prepare_transcription_input_with_format(audio_path, authorized_format)?;
    if let Some(format_extension) = prepared_input.format_extension() {
        decode_hints = decode_hints.with_audio_format_extension(format_extension);
    }
    let prepared_recording_health = prepared_input.recording_health().cloned();
    let diarization_audio_path = prepared_input
        .diarization_audio_path()
        .map(Path::to_path_buf);
    let step_start = std::time::Instant::now();
    let result = transcribe_prepared_input_with_hints(
        &prepared_input,
        audio_path,
        input_authority,
        content_type,
        config,
        decode_hints,
    )?;
    crate::process_trace::stage("transcribe.done");
    drop(prepared_input);
    let transcript = if content_type == ContentType::Meeting {
        normalize_transcript_for_self_name_participant(&result.text, &attendees, &config.identity)
    } else {
        result.text
    };
    let filter_stats = result.stats;
    crate::process_trace::stage("pipeline.write.start");
    let mut effective_context = context.clone();
    effective_context.recording_health = merge_recording_health(
        prepared_recording_health,
        effective_context.recording_health,
    );
    let artifact = write_transcript_artifact_with_authority(
        audio_path,
        content_type,
        title,
        config,
        &effective_context,
        existing_output_path,
        transcript,
        filter_stats,
        step_start.elapsed().as_millis() as u64,
        if let Some(input) = input_authority {
            TranscriptAuthority::AuthorizedCapability(PrivateAudioAuthority(
                input.processing_path().to_path_buf(),
            ))
        } else {
            TranscriptAuthority::AmbientPath
        },
    );
    let artifact = artifact.map(|mut artifact| {
        artifact.diarization_audio_path = diarization_audio_path;
        artifact
    });
    match &artifact {
        Ok(_) => crate::process_trace::stage("pipeline.write.done"),
        Err(error) => crate::process_trace::stage_with_extra(
            "pipeline.write.error",
            serde_json::json!({"error": error.to_string()}),
        ),
    }
    artifact
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
    if is_reserved_private_audio_path(audio_path) {
        return Err(reject_authorized_input(
            "private audio tokens require the typed authorized entry point",
        ));
    }
    write_transcript_artifact_with_authority(
        audio_path,
        content_type,
        title,
        config,
        context,
        existing_output_path,
        transcript,
        filter_stats,
        transcribe_ms,
        TranscriptAuthority::AmbientPath,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_transcript_artifact_with_authority(
    audio_path: &Path,
    content_type: ContentType,
    title: Option<&str>,
    config: &Config,
    context: &BackgroundPipelineContext,
    existing_output_path: Option<&Path>,
    transcript: String,
    filter_stats: crate::transcribe::FilterStats,
    transcribe_ms: u64,
    authority: TranscriptAuthority,
) -> Result<TranscriptArtifact, MinutesError> {
    match &authority {
        TranscriptAuthority::AmbientPath if is_reserved_private_audio_path(audio_path) => {
            return Err(reject_authorized_input(
                "private audio tokens require the typed authorized entry point",
            ));
        }
        TranscriptAuthority::AuthorizedCapability(identity) if identity.0 != audio_path => {
            return Err(reject_authorized_input(
                "transcript authority does not match the private audio capability",
            ));
        }
        TranscriptAuthority::AmbientPath | TranscriptAuthority::AuthorizedCapability(_) => {}
    }
    let descriptor_authorized = matches!(&authority, TranscriptAuthority::AuthorizedCapability(_));
    let diagnostic_audio_target = if descriptor_authorized {
        "authorized-audio".to_string()
    } else {
        audio_path.display().to_string()
    };
    let metadata = private_audio_metadata(audio_path)?;
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

    // Suppression gate (issue #241): if the cleaned transcript is nothing but
    // hallucinated non-speech markers AND both capture stems were sparse, the
    // body is almost certainly fabricated on near-silent audio. Replace it
    // with a clear diagnostic message and promote `status: NoSpeech` for
    // greppability. The original noisy text is dropped - the source WAV is
    // preserved on disk and `minutes process` is the canonical retry path,
    // so there is no need to round-trip the hallucinated lines through the
    // markdown output.
    //
    // Decision routed through `should_suppress_transcript` so this path and
    // `process_with_progress_and_sidecar` share the exact same gate (codex
    // blocker #2 on PR #246).
    let (transcript, forced_no_speech_diagnosis) =
        match should_suppress_transcript(&transcript, context.recording_health.as_ref()) {
            Some(outcome) => (outcome.body, Some(outcome.diagnosis)),
            None => (transcript, None),
        };

    let word_count = transcript.split_whitespace().count();
    logging::log_step(
        "transcribe",
        &diagnostic_audio_target,
        transcribe_ms,
        serde_json::json!({"words": word_count, "mode": "background", "diagnosis": filter_stats.diagnosis()}),
    );

    let status =
        if forced_no_speech_diagnosis.is_some() || word_count < config.transcription.min_words {
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
        duration: format_duration_secs(filter_stats.audio_duration_secs),
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
        capture: None,
        sensitivity: None,
        debrief: None,
        consent: context.consent,
        consent_notice: context.consent_notice.clone(),
        visibility: None,
        speaker_map: vec![],
        name_corrections: Vec::new(),
        recording_health: context.recording_health.clone(),
        speaker_mapping: None,
        processing_warnings: Vec::new(),
        template: context.template.as_ref().map(|t| t.slug().to_string()),
        filter_diagnosis: if status == Some(OutputStatus::NoSpeech) {
            // Prefer the all-noise-suppression diagnosis when it fired; it
            // describes a different failure mode (whisper produced only
            // non-speech markers on sparse stems) than the standard
            // min_words / no_speech filter path.
            Some(
                forced_no_speech_diagnosis
                    .clone()
                    .unwrap_or_else(|| filter_stats.diagnosis()),
            )
        } else {
            None
        },
    };

    let write_result = if let Some(path) = existing_output_path {
        if descriptor_authorized {
            markdown::rewrite_without_retry_path(
                path,
                &frontmatter,
                &transcript,
                None,
                context.user_notes.as_deref(),
            )?
        } else {
            markdown::rewrite_with_retry_path(
                path,
                &frontmatter,
                &transcript,
                None,
                context.user_notes.as_deref(),
                Some(audio_path),
            )?
        }
    } else if descriptor_authorized {
        markdown::write_without_retry_path(
            &frontmatter,
            &transcript,
            None,
            context.user_notes.as_deref(),
            config,
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
        authority,
        diarization_audio_path: None,
    })
}

pub fn enrich_transcript_artifact<F>(
    audio_path: &Path,
    artifact: &TranscriptArtifact,
    config: &Config,
    context: &BackgroundPipelineContext,
    on_progress: F,
) -> Result<WriteResult, MinutesError>
where
    F: FnMut(PipelineStage),
{
    if is_reserved_private_audio_path(audio_path) {
        return Err(reject_authorized_input(
            "private audio tokens require the typed authorized entry point",
        ));
    }
    if artifact.is_descriptor_authorized() {
        return Err(reject_authorized_input(
            "authorized transcript requires the retained capability at enrichment",
        ));
    }
    enrich_transcript_artifact_with_authority(
        audio_path,
        artifact,
        config,
        context,
        TranscriptAuthority::AmbientPath,
        None,
        on_progress,
    )
}

#[allow(dead_code)] // Background bridge is enabled by the dependent policy slice B.
pub(crate) fn enrich_transcript_artifact_authorized<F>(
    input: &AuthorizedProcessAudioInput,
    artifact: &TranscriptArtifact,
    config: &Config,
    context: &BackgroundPipelineContext,
    on_progress: F,
) -> Result<WriteResult, MinutesError>
where
    F: FnMut(PipelineStage),
{
    input.verify_pipeline_binding()?;
    if !artifact.is_descriptor_authorized() {
        return Err(reject_authorized_input(
            "ambient transcript cannot be promoted to authorized enrichment",
        ));
    }
    enrich_transcript_artifact_with_authority(
        input.processing_path(),
        artifact,
        config,
        context,
        TranscriptAuthority::AuthorizedCapability(PrivateAudioAuthority(
            input.processing_path().to_path_buf(),
        )),
        Some(input),
        on_progress,
    )
}

fn enrich_transcript_artifact_with_authority<F>(
    audio_path: &Path,
    artifact: &TranscriptArtifact,
    config: &Config,
    context: &BackgroundPipelineContext,
    authority: TranscriptAuthority,
    input_authority: Option<&AuthorizedProcessAudioInput>,
    mut on_progress: F,
) -> Result<WriteResult, MinutesError>
where
    F: FnMut(PipelineStage),
{
    if artifact.authority != authority {
        return Err(reject_authorized_input(
            "transcript authority does not match the enrichment entry point",
        ));
    }
    if artifact.frontmatter.status == Some(OutputStatus::NoSpeech) {
        on_progress(PipelineStage::Saving);
        return Ok(artifact.write_result.clone());
    }

    let descriptor_authorized = matches!(&authority, TranscriptAuthority::AuthorizedCapability(_));
    if descriptor_authorized != input_authority.is_some() {
        return Err(reject_authorized_input(
            "live capability does not match transcript provenance",
        ));
    }
    let diagnostic_audio_path = if descriptor_authorized {
        Path::new("authorized-audio")
    } else {
        audio_path
    };
    let diagnostic_audio_target = diagnostic_audio_path.display().to_string();
    let diarization_audio_path = if descriptor_authorized {
        audio_path
    } else {
        artifact
            .diarization_audio_path
            .as_deref()
            .unwrap_or(audio_path)
    };
    let mut transcript = artifact.transcript.clone();
    let mut diarization_num_speakers = 0usize;
    let mut diarization_from_stems = false;
    let mut degraded_ml_fallback = false;
    let mut diarization_embeddings: std::collections::HashMap<String, Vec<f32>> =
        std::collections::HashMap::new();
    let mut recording_health: Option<markdown::RecordingHealth> = None;
    if config.diarization.engine != "none" && artifact.frontmatter.r#type == ContentType::Meeting {
        on_progress(PipelineStage::Diarizing);
        let diarize_start = std::time::Instant::now();
        let audio_duration = if let Some(input) = input_authority {
            diarize::audio_duration_secs_authorized(input).unwrap_or(f64::INFINITY)
        } else {
            diarize::audio_duration_secs(diarization_audio_path).unwrap_or(f64::INFINITY)
        };
        let transcript_windows = build_transcript_windows(&transcript, audio_duration);
        let ctx = diarize::DiarizationContext {
            purpose: diarize::DiarizationPurpose::PrimaryMeeting,
            transcript_windows: Some(&transcript_windows),
        };
        let outcome = if descriptor_authorized {
            diarize::diarize_proof_bound_audio(
                input_authority.expect("authorized enrichment checked its live capability"),
                config,
                ctx,
            )
        } else {
            diarize::diarize_with_context(diarization_audio_path, config, ctx)
        };
        match outcome {
            diarize::DiarizationOutcome::Result(result) => {
                let diarize_ms = diarize_start.elapsed().as_millis() as u64;
                diarization_num_speakers = result.num_speakers;
                diarization_from_stems = result.source_aware;
                degraded_ml_fallback = is_degraded_ml_fallback_result(&result);
                if degraded_ml_fallback {
                    if let Some(reason) = result.degraded_capture.clone() {
                        recording_health = Some(degraded_ml_recording_health(reason));
                    }
                }
                diarization_embeddings = result.speaker_embeddings.clone();
                logging::log_step(
                    "diarize",
                    &diagnostic_audio_target,
                    diarize_ms,
                    serde_json::json!({
                        "speakers": result.num_speakers,
                        "segments": result.segments.len(),
                        "first_segment_start": result.segments.first().map(|s| s.start),
                        "last_segment_end": result.segments.last().map(|s| s.end),
                    }),
                );
                transcript = diarize::apply_speakers(&transcript, &result);
                log_rendered_label_collapse_diagnostic(diagnostic_audio_path, &result, &transcript);
            }
            diarize::DiarizationOutcome::Skipped { reason } => {
                let diarize_ms = diarize_start.elapsed().as_millis() as u64;
                logging::log_step(
                    "diarize",
                    &diagnostic_audio_target,
                    diarize_ms,
                    serde_json::json!({
                        "skipped": true,
                        "reason": "degraded_capture",
                        "failure_kind": format!("{:?}", reason.failure_kind),
                    }),
                );
                recording_health = Some(reason.into());
            }
            diarize::DiarizationOutcome::NotConfigured => {
                logging::log_step(
                    "diarize",
                    &diagnostic_audio_target,
                    diarize_start.elapsed().as_millis() as u64,
                    serde_json::json!({"skipped": true}),
                );
            }
        }
    }

    let screen_dir = resolve_screen_context_directory(
        context.context_session_id.as_deref(),
        audio_path,
        descriptor_authorized,
    );
    let screen_files = screen_dir
        .as_ref()
        .filter(|dir| dir.exists())
        .map(|dir| crate::screen::list_screenshots(dir))
        .unwrap_or_default();

    let mut summary_participants: Vec<String> = Vec::new();
    let mut structured_actions: Vec<markdown::ActionItem> = Vec::new();
    let mut structured_decisions: Vec<markdown::Decision> = Vec::new();
    let mut structured_intents: Vec<markdown::Intent> = Vec::new();
    let audio_log_target = diagnostic_audio_target.clone();
    let summary_model = summarize::summarization_model_hint(config, !screen_files.is_empty());

    let mut raw_summary: Option<summarize::Summary> = None;
    let summary = if config.summarization.engine != "none" {
        on_progress(PipelineStage::Summarizing);
        // Notes are passed separately so the agent path can byte-budget them
        // apart from the transcript and keep the screenshot coverage bound
        // derived from transcript text only.
        summarize::summarize_with_template(
            &transcript,
            context.user_notes.as_deref(),
            &screen_files,
            config,
            context.template.as_ref(),
            Some(&audio_log_target),
        )
        .map(|summary| {
            let summary_chars = summary_signal_chars(&summary);

            let actions_started = std::time::Instant::now();
            structured_actions = extract_action_items(&summary);
            log_structured_llm_step(
                "action_items",
                diagnostic_audio_path,
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
                diagnostic_audio_path,
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
            diagnostic_audio_path,
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
            diagnostic_audio_path,
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

    if !descriptor_authorized && !config.screen_context.keep_after_summary {
        if let Some(session_id) = context.context_session_id.as_deref() {
            match crate::context_store::cleanup_screen_context(session_id) {
                Ok(status) => {
                    crate::screen::write_current_session_status(&status).ok();
                    tracing::info!(session_id, "screen captures and retrieval refs cleaned up");
                }
                Err(error) => {
                    tracing::warn!(session_id, error = %error, "failed to clean screen context");
                }
            }
        } else if let Some(screen_dir) = screen_dir.as_ref() {
            if screen_dir.exists() && std::fs::remove_dir_all(screen_dir).is_ok() {
                tracing::info!(dir = %screen_dir.display(), "screen captures cleaned up");
            }
        }
    }

    let attendees = merge_attendees(&artifact.frontmatter.attendees, &summary_participants);

    let attribution_start = std::time::Instant::now();
    let attribution = attribute_meeting_speakers(
        if descriptor_authorized {
            diagnostic_audio_path
        } else {
            audio_path
        },
        diagnostic_audio_path,
        !descriptor_authorized,
        artifact.frontmatter.r#type,
        artifact.frontmatter.source.as_deref(),
        config,
        &artifact.frontmatter.attendees,
        &attendees,
        diarization_num_speakers,
        diarization_from_stems,
        degraded_ml_fallback,
        &diarization_embeddings,
        transcript,
    );
    let attribution_ms = attribution_start.elapsed().as_millis() as u64;
    // Count the speaker labels the summarizer actually saw (pre name-application),
    // for the #392 collapse guard below.
    let effective_speaker_labels =
        count_non_unknown_labels(&attribution.debug.effective_transcript_speaker_labels);
    transcript = attribution.transcript;
    let speaker_map = attribution.speaker_map;
    let attendees = normalize_attendees_with_speaker_map(&attendees, &speaker_map);
    let name_corrections = if config.transcription.name_correction != NameCorrectionMode::Off
        && artifact.frontmatter.r#type == ContentType::Meeting
    {
        let name_pool = crate::name_correction::build_name_pool(
            &attendees,
            Some(&config.identity),
            load_vocabulary_for_decode_hints().as_ref(),
        );
        // Confirmed participants gate the aggressive (name-position) tier:
        // attendees plus High-confidence attributed speakers.
        let mut participants = attendees.clone();
        participants.extend(
            speaker_map
                .iter()
                .filter(|a| a.confidence == crate::diarize::Confidence::High)
                .map(|a| a.name.clone()),
        );
        let (corrected_transcript, corrections) =
            crate::name_correction::correct_names_with_participants(
                &transcript,
                &name_pool,
                &participants,
            );
        transcript = corrected_transcript;
        corrections
    } else {
        Vec::new()
    };
    let mut structured_actions =
        normalize_action_items_with_speaker_map(structured_actions, &speaker_map);
    let mut structured_intents =
        normalize_intents_with_speaker_map(structured_intents, &speaker_map);
    let structured_decisions =
        normalize_decisions_with_speaker_map(structured_decisions, &speaker_map);

    // #392: if diarization collapsed this multi-party meeting to <= 1 rendered
    // speaker, withhold assignees rather than let the summarizer's guess invert
    // who owes whom. Runs before entity extraction so the graph never sees a
    // mis-attributed owner.
    let collapse_warning = withhold_assignees_on_collapse(
        &mut structured_actions,
        &mut structured_intents,
        artifact.frontmatter.r#type == ContentType::Meeting,
        diarization_num_speakers,
        effective_speaker_labels,
        artifact.frontmatter.attendees.len(),
        attendees.len(),
    );

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
        diagnostic_audio_path,
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
    // write_transcript_artifact calls summarize unconditionally whenever
    // engine != "none" (no all-noise gate at this site), so attempted-ness
    // collapses to the same condition.
    let summarization_attempted = config.summarization.engine != "none";
    let mut summarization_warnings = detect_summarization_warnings(
        summary.as_deref(),
        &config.summarization.engine,
        &config.summarization.agent_command,
        config.summarization.agent_timeout_secs,
        summarization_attempted,
    );
    if let Some(warning) = collapse_warning {
        summarization_warnings.push(warning);
    }
    if !descriptor_authorized {
        if let Some(warning) = detect_silent_remote_stem_warning(
            artifact.frontmatter.r#type,
            audio_path,
            artifact.frontmatter.source.as_deref(),
            merge_recording_health(
                recording_health.clone(),
                artifact.frontmatter.recording_health.clone(),
            )
            .as_ref(),
        ) {
            summarization_warnings.push(warning);
        }
    }
    frontmatter.status = if !summarization_warnings.is_empty() {
        Some(OutputStatus::Degraded)
    } else if config.summarization.engine != "none" {
        Some(OutputStatus::Complete)
    } else {
        Some(OutputStatus::TranscriptOnly)
    };
    frontmatter.processing_warnings = summarization_warnings;
    frontmatter.attendees = attendees;
    frontmatter.people = people;
    frontmatter.entities = entities;
    frontmatter.action_items = structured_actions;
    frontmatter.decisions = structured_decisions;
    frontmatter.intents = structured_intents;
    frontmatter.speaker_map = speaker_map;
    frontmatter.name_corrections = name_corrections;
    frontmatter.recording_health =
        merge_recording_health(recording_health, frontmatter.recording_health);

    let mut result = if descriptor_authorized {
        markdown::rewrite_without_retry_path(
            &artifact.write_result.path,
            &frontmatter,
            &transcript,
            summary.as_deref(),
            context.user_notes.as_deref(),
        )?
    } else {
        markdown::rewrite_with_retry_path(
            &artifact.write_result.path,
            &frontmatter,
            &transcript,
            summary.as_deref(),
            context.user_notes.as_deref(),
            Some(audio_path),
        )?
    };
    apply_title_generation(
        diagnostic_audio_path,
        &mut result,
        &mut frontmatter,
        title_generation,
        |duration_ms, extra| {
            logging::log_step(
                "title_generation",
                &diagnostic_audio_target,
                duration_ms,
                extra,
            );
        },
    );

    on_progress(PipelineStage::Saving);

    if frontmatter.r#type == ContentType::Meeting {
        log_attribution_decision(
            diagnostic_audio_path,
            &result.path,
            attribution_ms,
            &attribution.debug,
        );
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
    options: ProcessOptions<'_>,
    mut on_progress: F,
) -> Result<WriteResult, MinutesError>
where
    F: FnMut(PipelineStage),
{
    let ProcessOptions {
        sidecar,
        template,
        input_authority,
    } = options;
    if input_authority.is_none() && is_reserved_private_audio_path(audio_path) {
        return Err(reject_authorized_input(
            "private audio tokens require the typed authorized entry point",
        ));
    }
    if let Some(input) = input_authority {
        input.verify_pipeline_binding()?;
    }
    let audio_path = input_authority
        .map(AuthorizedProcessAudioInput::processing_path)
        .unwrap_or(audio_path);
    let authorized_format = input_authority.map(AuthorizedProcessAudioInput::format_extension);
    let descriptor_bound = input_authority.is_some();
    let diagnostic_audio_path = if descriptor_bound {
        Path::new("authorized-audio")
    } else {
        audio_path
    };
    let diagnostic_audio_target = diagnostic_audio_path.display().to_string();
    let start = std::time::Instant::now();
    tracing::info!(
        file = %diagnostic_audio_path.display(),
        content_type = ?content_type,
        "starting pipeline"
    );

    // Verify file exists and is not empty
    let metadata = private_audio_metadata(audio_path)?;
    let recording_date =
        infer_recording_date(sidecar.and_then(|s| s.captured_at), sidecar, &metadata);
    if metadata.len() == 0 {
        return Err(crate::error::TranscribeError::EmptyAudio.into());
    }

    // Security: verify file is in an allowed directory (prevents path traversal via MCP)
    if !descriptor_bound {
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
    let mut decode_hints = build_decode_hints(
        title,
        calendar_event_title.as_deref(),
        pre_context.as_deref(),
        &calendar_attendees,
        Some(&config.identity),
        load_vocabulary_for_decode_hints().as_ref(),
    );
    if let Some(format_extension) = authorized_format {
        decode_hints = decode_hints.with_audio_format_extension(format_extension);
    }

    // Step 1: Transcribe (always)
    on_progress(PipelineStage::Transcribing);
    // Workaround: if this is a native-call .mov with stems beside it, transcribe
    // a freshly-mixed PCM from both valid stems or the one surviving stem.
    // `prepared_input` owns any temporary mix so Err/panic cleanup remains
    // automatic; a surviving original stem is borrowed and never removed.
    // If neither sibling contains signal, the typed mix error still prevents
    // unsafe fallback to the broken dual-track `.mov` decoder.
    let prepared_input = prepare_transcription_input_with_format(audio_path, authorized_format)?;
    if let Some(format_extension) = prepared_input.format_extension() {
        decode_hints = decode_hints.with_audio_format_extension(format_extension);
    }
    let prepared_recording_health = prepared_input.recording_health().cloned();
    let diarization_audio_path = prepared_input
        .diarization_audio_path()
        .map(Path::to_path_buf);
    tracing::info!(step = "transcribe", file = %diagnostic_audio_path.display(), "transcribing audio");
    let step_start = std::time::Instant::now();
    let result = transcribe_prepared_input_with_hints(
        &prepared_input,
        audio_path,
        input_authority,
        content_type,
        config,
        decode_hints,
    )?;
    crate::process_trace::stage("transcribe.done");
    drop(prepared_input);
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
        &diagnostic_audio_target,
        transcribe_ms,
        serde_json::json!({"words": word_count, "diagnosis": filter_stats.diagnosis()}),
    );

    // Check minimum word threshold
    let mut status = if word_count < config.transcription.min_words {
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
    let mut degraded_ml_fallback = false;
    let mut diarization_embeddings: std::collections::HashMap<String, Vec<f32>> =
        std::collections::HashMap::new();
    let mut recording_health = prepared_recording_health;
    let diarization_audio_path = diarization_audio_path.as_deref().unwrap_or(audio_path);
    let transcript = if config.diarization.engine != "none" && content_type == ContentType::Meeting
    {
        on_progress(PipelineStage::Diarizing);
        tracing::info!(step = "diarize", "running speaker diarization");
        let audio_duration = if let Some(input) = input_authority {
            diarize::audio_duration_secs_authorized(input).unwrap_or(f64::INFINITY)
        } else {
            diarize::audio_duration_secs(diarization_audio_path).unwrap_or(f64::INFINITY)
        };
        let transcript_windows = build_transcript_windows(&transcript, audio_duration);
        let ctx = diarize::DiarizationContext {
            purpose: diarize::DiarizationPurpose::PrimaryMeeting,
            transcript_windows: Some(&transcript_windows),
        };
        let outcome = if descriptor_bound {
            diarize::diarize_proof_bound_audio(
                input_authority.expect("descriptor-bound pipeline retained its capability"),
                config,
                ctx,
            )
        } else {
            diarize::diarize_with_context(diarization_audio_path, config, ctx)
        };
        match outcome {
            diarize::DiarizationOutcome::Result(result) => {
                diarization_num_speakers = result.num_speakers;
                diarization_from_stems = result.source_aware;
                degraded_ml_fallback = is_degraded_ml_fallback_result(&result);
                if degraded_ml_fallback {
                    if let Some(reason) = result.degraded_capture.clone() {
                        recording_health = merge_recording_health(
                            Some(degraded_ml_recording_health(reason)),
                            recording_health,
                        );
                    }
                }
                diarization_embeddings = result.speaker_embeddings.clone();
                let transcript = diarize::apply_speakers(&transcript, &result);
                log_rendered_label_collapse_diagnostic(diagnostic_audio_path, &result, &transcript);
                transcript
            }
            diarize::DiarizationOutcome::Skipped { reason } => {
                recording_health = merge_recording_health(Some(reason.into()), recording_health);
                transcript
            }
            diarize::DiarizationOutcome::NotConfigured => transcript,
        }
    } else {
        transcript
    };

    // Suppression gate (issue #241): if the diarized transcript is nothing
    // but hallucinated non-speech markers AND both capture stems were sparse,
    // replace the body with a diagnostic message and force `status: NoSpeech`.
    // Routed through `should_suppress_transcript` so this path and
    // `write_transcript_artifact` share the exact same gate (codex blocker
    // #2 on PR #246) - users see identical behavior regardless of which
    // entry point produced the artifact. The original noisy text is dropped
    // - the source WAV is preserved and `minutes process` is the canonical
    // retry path.
    let (transcript, forced_no_speech_diagnosis) =
        match should_suppress_transcript(&transcript, recording_health.as_ref()) {
            Some(outcome) => {
                tracing::warn!(
                    step = "transcribe",
                    diagnosis = %outcome.diagnosis,
                    "all-noise suppression fired on process path — replacing transcript body"
                );
                // Force NoSpeech status: we know the body is fabricated even
                // if the original `word_count` cleared `min_words`.
                status = Some(OutputStatus::NoSpeech);
                (outcome.body, Some(outcome.diagnosis))
            }
            None => (transcript, None),
        };

    // Step 3: Summarize (optional — depends on config.summarization.engine)
    // Pass user notes to the summarizer as high-priority context
    // Step 3: Summarize + extract structured intent
    let mut structured_actions: Vec<markdown::ActionItem> = Vec::new();
    let mut structured_decisions: Vec<markdown::Decision> = Vec::new();
    let mut structured_intents: Vec<markdown::Intent> = Vec::new();

    // Collect screen context screenshots (if any were captured)
    let screen_dir = (!descriptor_bound).then(|| crate::screen::screens_dir_for(audio_path));
    let screen_files = screen_dir
        .as_ref()
        .filter(|dir| dir.exists())
        .map(|dir| crate::screen::list_screenshots(dir))
        .unwrap_or_default();
    if !screen_files.is_empty() {
        tracing::info!(
            count = screen_files.len(),
            "screen context screenshots found"
        );
    }

    let mut summary_participants: Vec<String> = Vec::new();
    let audio_log_target = diagnostic_audio_target.clone();
    let summary_model = summarize::summarization_model_hint(config, !screen_files.is_empty());

    let mut raw_summary: Option<summarize::Summary> = None;
    // Skip summarization when the all-noise gate replaced the transcript body:
    // the LLM has nothing to summarize, and we'd just burn tokens / surface
    // a hallucinated summary on top of a hallucinated transcript.
    let summary: Option<String> = if forced_no_speech_diagnosis.is_some() {
        None
    } else if config.summarization.engine != "none" {
        on_progress(PipelineStage::Summarizing);
        tracing::info!(step = "summarize", "generating summary");

        // Send screenshots as actual images to vision-capable LLMs. Notes are
        // passed separately so the agent path can byte-budget them apart from
        // the transcript and keep the screenshot coverage bound derived from
        // transcript text only.
        summarize::summarize_with_template(
            &transcript,
            user_notes.as_deref(),
            &screen_files,
            config,
            template,
            Some(&audio_log_target),
        )
        .map(|s| {
            let summary_chars = summary_signal_chars(&s);

            let actions_started = std::time::Instant::now();
            structured_actions = extract_action_items(&s);
            log_structured_llm_step(
                "action_items",
                diagnostic_audio_path,
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
                diagnostic_audio_path,
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
            diagnostic_audio_path,
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
            diagnostic_audio_path,
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
    if !config.screen_context.keep_after_summary {
        if let Some(screen_dir) = screen_dir.as_ref() {
            if screen_dir.exists() && std::fs::remove_dir_all(screen_dir).is_ok() {
                tracing::info!(dir = %screen_dir.display(), "screen captures cleaned up");
            }
        }
    }

    // Step 4: Match calendar event + merge attendees

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
        diagnostic_audio_path,
        diagnostic_audio_path,
        !descriptor_bound,
        content_type,
        sidecar.and_then(|metadata| metadata.source.as_deref()),
        config,
        &calendar_attendees,
        &attendees,
        diarization_num_speakers,
        diarization_from_stems,
        degraded_ml_fallback,
        &diarization_embeddings,
        transcript,
    );
    let attribution_ms = attribution_start.elapsed().as_millis() as u64;
    // Speaker labels the summarizer saw (pre name-application), for the #392 guard.
    let effective_speaker_labels =
        count_non_unknown_labels(&attribution.debug.effective_transcript_speaker_labels);
    let mut transcript = attribution.transcript;
    let speaker_map = attribution.speaker_map;
    let attendees = normalize_attendees_with_speaker_map(&attendees, &speaker_map);
    let name_corrections = if config.transcription.name_correction != NameCorrectionMode::Off
        && content_type == ContentType::Meeting
    {
        let name_pool = crate::name_correction::build_name_pool(
            &attendees,
            Some(&config.identity),
            load_vocabulary_for_decode_hints().as_ref(),
        );
        // Confirmed participants gate the aggressive (name-position) tier:
        // attendees plus High-confidence attributed speakers.
        let mut participants = attendees.clone();
        participants.extend(
            speaker_map
                .iter()
                .filter(|a| a.confidence == crate::diarize::Confidence::High)
                .map(|a| a.name.clone()),
        );
        let (corrected_transcript, corrections) =
            crate::name_correction::correct_names_with_participants(
                &transcript,
                &name_pool,
                &participants,
            );
        transcript = corrected_transcript;
        corrections
    } else {
        Vec::new()
    };
    let mut structured_actions =
        normalize_action_items_with_speaker_map(structured_actions, &speaker_map);
    let mut structured_intents =
        normalize_intents_with_speaker_map(structured_intents, &speaker_map);
    let structured_decisions =
        normalize_decisions_with_speaker_map(structured_decisions, &speaker_map);

    // #392: withhold assignees if diarization collapsed this multi-party meeting
    // to <= 1 rendered speaker (trusted attendees = calendar attendees). Runs
    // before entity extraction so a mis-attributed owner never reaches the graph.
    let collapse_warning = withhold_assignees_on_collapse(
        &mut structured_actions,
        &mut structured_intents,
        content_type == ContentType::Meeting,
        diarization_num_speakers,
        effective_speaker_labels,
        calendar_attendees.len(),
        attendees.len(),
    );

    // Step 5: Write markdown (always)
    let duration = format_duration_secs(filter_stats.audio_duration_secs);
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
        diagnostic_audio_path,
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

    // Issue #243: detect post-transcript degradation (e.g. summarization
    // failed or timed out) and promote status to `Degraded` so the file
    // itself is honest about what's missing. The initial `status` set
    // above didn't yet know whether summarization would succeed; this
    // is the corrective pass.
    //
    // Summarization was *attempted* only when both the all-noise gate
    // did NOT fire (forced_no_speech_diagnosis is None) AND engine is
    // not "none". Without the all-noise guard, an empty summary on a
    // no-speech recording would falsely look like a summarize failure.
    let summarization_attempted =
        forced_no_speech_diagnosis.is_none() && config.summarization.engine != "none";
    let mut summarization_warnings = detect_summarization_warnings(
        summary.as_deref(),
        &config.summarization.engine,
        &config.summarization.agent_command,
        config.summarization.agent_timeout_secs,
        summarization_attempted,
    );
    if let Some(warning) = collapse_warning {
        summarization_warnings.push(warning);
    }
    if let Some(warning) = detect_silent_remote_stem_warning(
        content_type,
        audio_path,
        source.as_deref(),
        recording_health.as_ref(),
    ) {
        summarization_warnings.push(warning);
    }
    let status = if !summarization_warnings.is_empty() && status != Some(OutputStatus::NoSpeech) {
        Some(OutputStatus::Degraded)
    } else {
        status
    };

    let mut frontmatter = Frontmatter {
        title: auto_title,
        r#type: content_type,
        date: recording_date,
        duration,
        source,
        status,
        processing_warnings: summarization_warnings,
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
        capture: None,
        sensitivity: None,
        debrief: None,
        consent: None,
        consent_notice: None,
        visibility: None,
        speaker_map,
        name_corrections,
        recording_health,
        speaker_mapping: None,
        template: template.map(|t| t.slug().to_string()),
        filter_diagnosis: if status == Some(OutputStatus::NoSpeech) {
            // Prefer the all-noise-suppression diagnosis when it fired; it
            // describes a different failure mode (whisper produced only
            // non-speech markers on sparse stems) than the standard
            // min_words / no_speech filter path. Identical preference order
            // to `write_transcript_artifact` so both entry points produce
            // matching frontmatter.
            Some(
                forced_no_speech_diagnosis
                    .clone()
                    .unwrap_or_else(|| filter_stats.diagnosis()),
            )
        } else {
            None
        },
    };

    tracing::info!(step = "write", "writing markdown");
    crate::process_trace::stage("pipeline.write.start");
    let step_start = std::time::Instant::now();
    let mut result = if descriptor_bound {
        markdown::write_without_retry_path(
            &frontmatter,
            &transcript,
            summary.as_deref(),
            user_notes.as_deref(),
            config,
        )?
    } else {
        markdown::write_with_retry_path(
            &frontmatter,
            &transcript,
            summary.as_deref(),
            user_notes.as_deref(),
            Some(audio_path),
            config,
        )?
    };
    on_progress(PipelineStage::Saving);
    apply_title_generation(
        diagnostic_audio_path,
        &mut result,
        &mut frontmatter,
        title_generation,
        |duration_ms, extra| {
            logging::log_step(
                "title_generation",
                &diagnostic_audio_target,
                duration_ms,
                extra,
            );
        },
    );

    if frontmatter.r#type == ContentType::Meeting {
        log_attribution_decision(
            diagnostic_audio_path,
            &result.path,
            attribution_ms,
            &attribution.debug,
        );
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
        &diagnostic_audio_target,
        write_ms,
        serde_json::json!({"output": result.path.display().to_string(), "words": result.word_count}),
    );
    crate::process_trace::stage_with_extra(
        "pipeline.write.done",
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
        &diagnostic_audio_target,
    ));

    let elapsed = start.elapsed();
    logging::log_step(
        "pipeline_complete",
        &diagnostic_audio_target,
        elapsed.as_millis() as u64,
        serde_json::json!({"output": result.path.display().to_string(), "words": result.word_count, "content_type": format!("{:?}", content_type)}),
    );
    tracing::info!(
        file = %result.path.display(),
        words = result.word_count,
        elapsed_ms = elapsed.as_millis() as u64,
        "pipeline complete"
    );
    crate::process_trace::stage_with_extra(
        "pipeline.done",
        serde_json::json!({"output": result.path.display().to_string(), "words": result.word_count}),
    );

    Ok(result)
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

fn parse_transcript_line_start_secs(line: &str) -> Option<f32> {
    let rest = line.strip_prefix('[')?;
    let bracket_end = rest.find(']')?;
    let timestamp = &rest[..bracket_end];
    let parts: Vec<&str> = timestamp.split(':').collect();
    let secs = match parts.as_slice() {
        [minutes, seconds] => minutes.parse::<u64>().ok()? * 60 + seconds.parse::<u64>().ok()?,
        [hours, minutes, seconds] => {
            hours.parse::<u64>().ok()? * 3600
                + minutes.parse::<u64>().ok()? * 60
                + seconds.parse::<u64>().ok()?
        }
        _ => return None,
    };
    Some(secs as f32)
}

fn parse_transcript_line_starts(transcript: &str) -> Vec<f32> {
    transcript
        .lines()
        .filter_map(parse_transcript_line_start_secs)
        .collect()
}

fn build_transcript_windows(
    transcript: &str,
    audio_duration_secs: f64,
) -> Vec<diarize::TranscriptWindow> {
    let starts = parse_transcript_line_starts(transcript);
    let duration = audio_duration_secs as f32;
    let mut windows = Vec::new();

    for (index, start) in starts.iter().copied().enumerate() {
        let next_start = starts.get(index + 1).copied().unwrap_or(f32::INFINITY);
        let natural_end = start + 8.0;
        let mut end = next_start.min(natural_end);
        if duration.is_finite() {
            end = end.min(duration);
        }
        if end > start {
            windows.push(diarize::TranscriptWindow {
                start_secs: start,
                end_secs: end,
            });
        }
    }

    windows
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
                let reason = if looks_like_instruction_echo(&cleaned) {
                    "rejected-echo"
                } else {
                    "rejected-title"
                };
                TitleGenerationDecision {
                    final_title: fallback_title.to_string(),
                    refined_title: None,
                    outcome: "fallback",
                    model: Some(refined.model),
                    input_chars: refined.input_chars,
                    detail: Some(format!("{}: {}", reason, cleaned)),
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

    // #401: the model sometimes answers the title prompt conversationally
    // ("I'll create a concise meeting title...") instead of returning a bare
    // title, and the echo was slugified straight into the filename. Reject
    // instruction-echo / meta-talk so the deterministic fallback title is used.
    if looks_like_instruction_echo(candidate) {
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

/// #401: detect when an LLM answered the title prompt conversationally or
/// echoed the instructions back, instead of returning a bare title. These
/// patterns are meta-talk about the task, vanishingly unlikely in a real
/// 3-8 word meeting title, so matching one means the candidate should be
/// rejected in favor of the deterministic fallback title.
fn looks_like_instruction_echo(candidate: &str) -> bool {
    let lower = candidate.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }

    // Phrases lifted straight from the title prompt, or clear meta-talk.
    const ECHO_PHRASES: &[&str] = &[
        "concise meeting title",
        "the title text",
        "here is the title",
        "here's the title",
        "as an ai",
        "meeting title:",
    ];
    if ECHO_PHRASES.iter().any(|p| lower.contains(p)) {
        return true;
    }

    // Conversational lead-ins a model uses when it replies in prose rather
    // than emitting a bare title. Kept deliberately narrow to avoid rejecting
    // legitimate titles (e.g. "Here" or "Sure" as standalone words are fine).
    const ECHO_PREFIXES: &[&str] = &[
        "i'll ",
        "i will ",
        "i'd ",
        "i can ",
        "i have ",
        "i've ",
        "here is ",
        "here's ",
        "here are ",
        "sure,",
        "sure ",
        "certainly",
        "of course",
        "let me ",
        "based on ",
    ];
    ECHO_PREFIXES.iter().any(|p| lower.starts_with(p))
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
            .filter(|&&c| {
                c.is_ascii_alphabetic()
                    || ('\u{00C0}'..='\u{024F}').contains(&c) // Latin-1 Supplement + Extended-A/B
                    || ('\u{1E00}'..='\u{1EFF}').contains(&c) // Latin Extended Additional
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
    vocabulary: Option<&crate::vocabulary::VocabularyStore>,
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

    if let Some(vocabulary) = vocabulary {
        priority.extend(vocabulary.decode_phrases(8));
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

fn load_vocabulary_for_decode_hints() -> Option<crate::vocabulary::VocabularyStore> {
    match crate::vocabulary::load() {
        Ok(store) if !store.entries.is_empty() => Some(store),
        Ok(_) => None,
        Err(error) => {
            tracing::debug!(error = %error, "could not load vocabulary for decode hints");
            None
        }
    }
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

/// Count the distinct real speaker labels (excluding `UNKNOWN`) from an already
/// extracted label list. For #392 this must be fed the labels the SUMMARIZER saw
/// (`attribution.debug.effective_transcript_speaker_labels`, captured before
/// speaker-map name application), NOT the final written transcript: two distinct
/// diarized labels that later map to one name still gave the summarizer two
/// speakers to attribute, so that is not a collapse.
fn count_non_unknown_labels(labels: &[String]) -> usize {
    labels
        .iter()
        .filter(|label| label.as_str() != "UNKNOWN")
        .count()
}

/// Guard against action-item / commitment assignee INVERSION when diarization
/// collapses a multi-party meeting into a single rendered speaker (#392).
///
/// When the summarizer saw <= 1 real speaker label for a meeting that had
/// multiple participants, it cannot reliably attribute commitments and
/// empirically defaults ownership to the recorder, silently reversing who owes
/// whom. In that state we withhold assignees (a wrong owner is worse than none):
/// action-item assignees become the existing `unassigned` sentinel, and
/// `ActionItem`/`Commitment` intent owners are cleared. Open questions and
/// decisions are left alone. Returns a [`markdown::ProcessingWarning`] when
/// anything was withheld so the frontmatter is honest (and status degrades).
///
/// Gate:
/// - the file is a meeting,
/// - diarization produced a result (`diarization_num_speakers >= 1`) — so this
///   never fires for users who run with diarization disabled,
/// - but the rendered transcript had `<= 1` effective speaker label, and
/// - the meeting is expected to be multi-party: `trusted` (calendar/frontmatter)
///   attendees `>= 2`, or merged/detected participants `>= 2` only when there is
///   no trusted list (so the LLM that guessed ownership cannot also supply the
///   multi-party proof).
fn withhold_assignees_on_collapse(
    action_items: &mut [markdown::ActionItem],
    intents: &mut [markdown::Intent],
    is_meeting: bool,
    diarization_num_speakers: usize,
    rendered_speaker_labels: usize,
    trusted_attendee_count: usize,
    merged_attendee_count: usize,
) -> Option<markdown::ProcessingWarning> {
    let multi_party =
        trusted_attendee_count >= 2 || (trusted_attendee_count == 0 && merged_attendee_count >= 2);
    let collapsed =
        is_meeting && diarization_num_speakers >= 1 && rendered_speaker_labels <= 1 && multi_party;
    if !collapsed {
        return None;
    }

    let mut actions_withheld = 0usize;
    for item in action_items.iter_mut() {
        if item.assignee != "unassigned" {
            item.assignee = "unassigned".to_string();
            actions_withheld += 1;
        }
    }
    let mut owners_withheld = 0usize;
    for intent in intents.iter_mut() {
        let ownable = matches!(
            intent.kind,
            markdown::IntentKind::ActionItem | markdown::IntentKind::Commitment
        );
        if ownable && intent.who.is_some() {
            intent.who = None;
            owners_withheld += 1;
        }
    }
    if actions_withheld == 0 && owners_withheld == 0 {
        return None;
    }

    Some(markdown::ProcessingWarning {
        step: "attribution".to_string(),
        reason: "diarization_collapsed".to_string(),
        timeout_secs: None,
        message: Some(format!(
            "Diarization rendered {rendered_speaker_labels} speaker label(s) (raw {diarization_num_speakers}) \
             for a meeting with {trusted_attendee_count} listed / {merged_attendee_count} detected participant(s). \
             Attribution is unreliable, so {actions_withheld} action-item assignee(s) and {owners_withheld} \
             action-item/commitment owner(s) were withheld (set to unassigned) rather than risk mis-attributing who owes whom."
        )),
    })
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
/// and each disposable graph projection end up with duplicate Person rows.
/// Non-user entities are unaffected.
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
    // #385 class 2: split multi-person strings ("Gert and Liam") into separate
    // references before each is stripped, gated, and slugged individually.
    for reference in crate::person_identity::split_person_references(raw) {
        add_single_person_entity(entities, &reference);
    }
}

fn add_single_person_entity(
    entities: &mut BTreeMap<String, (String, BTreeSet<String>)>,
    raw: &str,
) {
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

    let name_part = strip_email_domain(strip_name_disambiguation(&trimmed)).trim();
    // Strip role/title and diarization speaker-label contamination (to a fixpoint),
    // then reject non-person tokens (groups/roles/generics) before creating the
    // entity (#385).
    let canonical = strip_contamination(name_part);
    if canonical.is_empty() || !is_plausible_person_name(canonical) {
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
            match crate::engine_process::command("sh")
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

    fn sparse_health(voice: f32, system: f32) -> markdown::RecordingHealth {
        markdown::RecordingHealth {
            voice_stem_active_ratio: Some(voice),
            system_stem_active_ratio: Some(system),
            system_dominant_ratio: None,
            capture_warnings: vec![],
            diarization_path: None,
        }
    }

    fn mic_only_health() -> markdown::RecordingHealth {
        markdown::RecordingHealth {
            voice_stem_active_ratio: None,
            system_stem_active_ratio: None,
            system_dominant_ratio: None,
            capture_warnings: vec![],
            diarization_path: None,
        }
    }

    fn in_person_system_silent_health() -> markdown::RecordingHealth {
        markdown::RecordingHealth {
            voice_stem_active_ratio: Some(1.0),
            system_stem_active_ratio: Some(0.0),
            system_dominant_ratio: None,
            capture_warnings: vec![markdown::CaptureWarning {
                kind: diarize::FailureKind::Silent,
                source: diarize::CaptureSource::System,
                message: "System audio was silent during capture; speaker labels were recovered from degraded mic bleed with low confidence.".into(),
                diagnostic_confidence: diarize::DiagnosticConfidence::Inferred,
            }],
            diarization_path: Some(markdown::DiarizationPath::MlBleedDegraded),
        }
    }

    fn native_call_system_recovery_health() -> markdown::RecordingHealth {
        crate::health::recording_health_for_native_call_stem_recovery(diarize::CaptureSource::Voice)
    }

    fn native_call_microphone_recovery_health() -> markdown::RecordingHealth {
        crate::health::recording_health_for_native_call_stem_recovery(
            diarize::CaptureSource::System,
        )
    }

    #[test]
    fn suppress_if_all_noise_fires_on_all_noise_with_sparse_stems() {
        // The exact failure case from issue #241.
        let transcript = "[0:07] (crying)\n[1:52] [Growling]\n";
        let health = sparse_health(0.005, 0.001);
        let diagnosis = suppress_if_all_noise(transcript, Some(&health));
        assert!(diagnosis.is_some(), "expected suppression diagnosis");
        let msg = diagnosis.unwrap();
        assert!(msg.contains("all-noise"), "msg: {}", msg);
        assert!(msg.contains("threshold"), "msg: {}", msg);
    }

    #[test]
    fn suppress_if_all_noise_holds_off_when_stems_have_signal() {
        // Stems are above the sparse threshold - we lack the corroborating
        // capture-side evidence, so let the transcript through even if it
        // looks all-noise. Better to surface the suspicious lines than to
        // hide real (if brief) capture.
        let transcript = "[0:07] (crying)\n[1:52] [Growling]\n";
        let health = sparse_health(0.5, 0.4);
        assert!(suppress_if_all_noise(transcript, Some(&health)).is_none());
    }

    #[test]
    fn suppress_if_all_noise_holds_off_when_only_one_stem_is_sparse() {
        // Asymmetric capture (one side silent, one side active) is a
        // different failure mode - we trust the active side and don't
        // suppress here.
        let transcript = "[0:07] (crying)\n[1:52] [Growling]\n";
        let health = sparse_health(0.001, 0.5);
        assert!(suppress_if_all_noise(transcript, Some(&health)).is_none());
    }

    #[test]
    fn suppress_if_all_noise_holds_off_with_real_content() {
        // Real speech, even with a noise marker mixed in, is left alone.
        let transcript = "[0:00] Hello world\n[0:05] (crying)\n[0:10] Goodbye\n";
        let health = sparse_health(0.001, 0.001);
        assert!(suppress_if_all_noise(transcript, Some(&health)).is_none());
    }

    #[test]
    fn suppress_if_all_noise_holds_off_without_health() {
        // No recording_health (e.g. dictation, or a test fixture) means we
        // can't confirm the stems were sparse. Be conservative.
        let transcript = "[0:07] (crying)\n[1:52] [Growling]\n";
        assert!(suppress_if_all_noise(transcript, None).is_none());
    }

    #[test]
    fn suppress_if_all_noise_holds_off_with_partial_health() {
        // Only one stem ratio captured - inconclusive, don't suppress.
        let mut health = sparse_health(0.001, 0.001);
        health.system_stem_active_ratio = None;
        let transcript = "[0:07] (crying)\n[1:52] [Growling]\n";
        assert!(suppress_if_all_noise(transcript, Some(&health)).is_none());
    }

    #[test]
    fn should_suppress_transcript_wraps_decision_in_outcome() {
        // The shared helper returns the same body+diagnosis used by BOTH
        // `write_transcript_artifact` and the `process` path. This is the
        // single source of truth that closes codex blocker #2 - both call
        // sites must produce identical suppression output.
        let transcript = "[0:07] (crying)\n[1:52] [Growling]\n";
        let health = sparse_health(0.005, 0.001);
        let outcome = should_suppress_transcript(transcript, Some(&health))
            .expect("expected suppression outcome");
        assert_eq!(outcome.body, ALL_NOISE_SUPPRESSED_BODY);
        assert!(outcome.diagnosis.contains("all-noise"));
        assert!(outcome.diagnosis.contains("threshold"));
    }

    #[test]
    fn should_suppress_transcript_returns_none_with_real_content() {
        let transcript = "[0:00] Hello world\n[0:05] Goodbye\n";
        let health = sparse_health(0.001, 0.001);
        assert!(should_suppress_transcript(transcript, Some(&health)).is_none());
    }

    // ── Summarization-degradation detection (issue #243) ──

    #[test]
    fn detect_summarization_warnings_returns_empty_when_engine_none() {
        // When summarization is disabled by config, an absent summary is
        // expected behavior, not a degradation.
        let warnings = detect_summarization_warnings(None, "none", "claude", 300, false);
        assert!(warnings.is_empty());
    }

    #[test]
    fn detect_summarization_warnings_returns_empty_when_summary_present() {
        let summary = "Some real summary content";
        let warnings = detect_summarization_warnings(Some(summary), "agent", "opencode", 300, true);
        assert!(warnings.is_empty());
    }

    #[test]
    fn detect_summarization_warnings_returns_empty_when_not_attempted() {
        // Codex review of v1 (PR #249) caught this: when the no-speech /
        // all-noise gate prevents summarization from running, summary is
        // None but that is expected, not a degradation. The helper must
        // not emit a bogus `summarize_failed` warning in that case.
        let warnings = detect_summarization_warnings(None, "agent", "opencode", 300, false);
        assert!(warnings.is_empty());
    }

    #[test]
    fn detect_summarization_warnings_stays_silent_for_every_engine_when_not_attempted() {
        // This is a contract test on the helper itself, not an end-to-end
        // integration test. Both call sites (write_transcript_artifact and
        // process_with_progress_and_sidecar) rely on this invariant when
        // their upstream no-speech / all-noise gate fires: pass
        // `summarization_attempted = false` and trust the helper to return
        // zero warnings regardless of the configured engine. If any engine
        // value leaked a warning here, the upstream short-circuit would be
        // insufficient and the frontmatter would gain a bogus
        // `summarize_failed` entry on no-speech recordings.
        for (engine, agent_cmd) in [
            ("agent", "opencode"),
            ("auto", "claude"),
            ("claude", "claude"),
        ] {
            let warnings = detect_summarization_warnings(None, engine, agent_cmd, 300, false);
            assert!(
                warnings.is_empty(),
                "engine={} produced warnings when not attempted: {:?}",
                engine,
                warnings
            );
        }
    }

    #[test]
    fn detect_summarization_warnings_flags_agent_failure_with_timeout_context() {
        // The #243 failure shape: engine = "agent", summary is None,
        // summarization was actually attempted.
        let warnings = detect_summarization_warnings(None, "agent", "opencode", 300, true);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].step, "summarize");
        assert_eq!(warnings[0].reason, "summarize_failed");
        assert_eq!(warnings[0].timeout_secs, Some(300));
        let msg = warnings[0].message.as_ref().expect("message set");
        assert!(msg.contains("opencode"));
        assert!(msg.contains("300s"));
    }

    #[test]
    fn detect_summarization_warnings_auto_engine_message_explains_indirection() {
        // Codex review of v1 (PR #249) caught this: when engine = "auto",
        // the warning previously printed `agent_command` even though auto
        // detects a CLI at runtime. The message must surface the auto
        // indirection and tell the user to check audio.log for which
        // agent was selected.
        let warnings = detect_summarization_warnings(None, "auto", "claude", 600, true);
        assert_eq!(warnings.len(), 1);
        let msg = warnings[0].message.as_ref().unwrap();
        assert!(msg.contains("auto"));
        assert!(msg.contains("600s"));
        assert!(msg.contains("audio.log"));
        assert_eq!(warnings[0].timeout_secs, Some(600));
    }

    #[test]
    fn detect_summarization_warnings_flags_non_agent_engine_without_timeout() {
        // Non-agent engines (claude, ollama, mistral, etc.) don't have a
        // single agent_timeout_secs knob, so the warning carries no
        // timeout_secs field but still flags the degradation.
        let warnings = detect_summarization_warnings(None, "claude", "claude", 300, true);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].step, "summarize");
        assert_eq!(warnings[0].timeout_secs, None);
        assert!(warnings[0]
            .message
            .as_ref()
            .unwrap()
            .contains("engine `claude`"));
    }

    fn apply_silent_remote_warning_on_batch_path(
        content_type: ContentType,
        audio_path: &Path,
        source: Option<&str>,
        health: Option<&markdown::RecordingHealth>,
        initial_status: Option<OutputStatus>,
    ) -> (Vec<ProcessingWarning>, Option<OutputStatus>) {
        let mut warnings = Vec::new();
        if let Some(warning) =
            detect_silent_remote_stem_warning(content_type, audio_path, source, health)
        {
            warnings.push(warning);
        }
        let status = if !warnings.is_empty() && initial_status != Some(OutputStatus::NoSpeech) {
            Some(OutputStatus::Degraded)
        } else {
            initial_status
        };
        (warnings, status)
    }

    #[test]
    fn in_person_system_silent_capture_does_not_warn_without_recovery_marker() {
        let health = in_person_system_silent_health();
        let (warnings, status) = apply_silent_remote_warning_on_batch_path(
            ContentType::Meeting,
            Path::new("/tmp/in-person-meeting.wav"),
            None,
            Some(&health),
            Some(OutputStatus::Complete),
        );

        assert!(warnings.is_empty());
        assert_eq!(status, Some(OutputStatus::Complete));
    }

    #[test]
    fn active_voice_zero_system_ratio_does_not_warn_without_recovery_marker() {
        let health = sparse_health(0.90, 0.0);
        let (warnings, status) = apply_silent_remote_warning_on_batch_path(
            ContentType::Meeting,
            Path::new("/tmp/native-captures/call.wav"),
            Some("native-call"),
            Some(&health),
            Some(OutputStatus::Complete),
        );

        assert!(warnings.is_empty());
        assert_eq!(status, Some(OutputStatus::Complete));
    }

    #[test]
    fn native_call_recovery_marker_warns_and_degrades() {
        let health = native_call_system_recovery_health();
        let (warnings, status) = apply_silent_remote_warning_on_batch_path(
            ContentType::Meeting,
            Path::new("/Users/test/.minutes/jobs/job-123.voice.wav"),
            None,
            Some(&health),
            Some(OutputStatus::TranscriptOnly),
        );

        assert_eq!(status, Some(OutputStatus::Degraded));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].step, "capture");
        assert_eq!(warnings[0].reason, "remote_audio_not_captured");
        assert_eq!(
            warnings[0].message.as_deref(),
            Some(SILENT_REMOTE_WARNING_MESSAGE)
        );
    }

    #[test]
    fn native_call_microphone_recovery_warns_and_degrades() {
        let health = native_call_microphone_recovery_health();
        let (warnings, status) = apply_silent_remote_warning_on_batch_path(
            ContentType::Meeting,
            Path::new("/Users/test/.minutes/jobs/job-123.system.wav"),
            None,
            Some(&health),
            Some(OutputStatus::TranscriptOnly),
        );

        assert_eq!(status, Some(OutputStatus::Degraded));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].step, "capture");
        assert_eq!(warnings[0].reason, "microphone_audio_not_captured");
        assert_eq!(
            warnings[0].message.as_deref(),
            Some(SILENT_MICROPHONE_WARNING_MESSAGE)
        );
    }

    #[test]
    fn sparse_but_captured_remote_does_not_warn() {
        let health = sparse_health(0.90, 0.015);
        let (warnings, status) = apply_silent_remote_warning_on_batch_path(
            ContentType::Meeting,
            Path::new("/tmp/native-captures/call.wav"),
            Some("native-call"),
            Some(&health),
            Some(OutputStatus::Complete),
        );

        assert!(warnings.is_empty());
        assert_eq!(status, Some(OutputStatus::Complete));
    }

    #[test]
    fn non_call_mic_only_recording_does_not_warn() {
        let health = mic_only_health();
        let (warnings, status) = apply_silent_remote_warning_on_batch_path(
            ContentType::Meeting,
            Path::new("/tmp/in-person-meeting.wav"),
            None,
            Some(&health),
            Some(OutputStatus::Complete),
        );

        assert!(warnings.is_empty());
        assert_eq!(status, Some(OutputStatus::Complete));
    }

    #[test]
    fn healthy_call_with_both_stems_active_does_not_warn() {
        let health = sparse_health(0.90, 0.85);
        let (warnings, status) = apply_silent_remote_warning_on_batch_path(
            ContentType::Meeting,
            Path::new("/tmp/native-captures/call.wav"),
            Some("native-call"),
            Some(&health),
            Some(OutputStatus::Complete),
        );

        assert!(warnings.is_empty());
        assert_eq!(status, Some(OutputStatus::Complete));
    }

    /// Simulates the branch logic the `process` path applies after
    /// diarization: if `should_suppress_transcript` fires, the transcript
    /// body, status, and forced filter_diagnosis are all updated together.
    /// This mirrors lines around 1790 of `process_with_progress_and_sidecar`.
    /// We can't run the full pipeline in a unit test (whisper model, audio
    /// file, calendar lookup), but we CAN assert the decision-and-apply
    /// logic produces exactly the same observable state as the
    /// `write_transcript_artifact` path for the same input.
    fn apply_suppression_on_process_path(
        transcript: String,
        recording_health: Option<&markdown::RecordingHealth>,
        initial_status: Option<OutputStatus>,
    ) -> (String, Option<OutputStatus>, Option<String>) {
        let mut status = initial_status;
        let (transcript, forced) = match should_suppress_transcript(&transcript, recording_health) {
            Some(outcome) => {
                status = Some(OutputStatus::NoSpeech);
                (outcome.body, Some(outcome.diagnosis))
            }
            None => (transcript, None),
        };
        (transcript, status, forced)
    }

    #[test]
    fn process_path_suppresses_all_noise_with_sparse_stems() {
        // Acceptance criterion 4 (issue #241): the `minutes process <wav>`
        // path must show the "no audible content" message and a NoSpeech
        // status, not the raw hallucinated lines.
        let transcript = "[0:07] (crying)\n[1:52] [Growling]\n".to_string();
        let health = sparse_health(0.005, 0.001);
        let (body, status, forced) = apply_suppression_on_process_path(
            transcript,
            Some(&health),
            // Start from `Complete` to prove the gate downgrades the status
            // even when word_count cleared min_words.
            Some(OutputStatus::Complete),
        );
        assert_eq!(body, ALL_NOISE_SUPPRESSED_BODY);
        assert!(
            body.contains("No audible content"),
            "body should surface the diagnostic message, got: {}",
            body
        );
        // The raw hallucinated lines must NOT appear in the rendered body.
        assert!(
            !body.contains("(crying)"),
            "raw hallucination leaked: {}",
            body
        );
        assert!(
            !body.contains("[Growling]"),
            "raw hallucination leaked: {}",
            body
        );
        assert_eq!(status, Some(OutputStatus::NoSpeech));
        let diag = forced.expect("expected forced filter_diagnosis");
        assert!(diag.contains("all-noise"));
        assert!(diag.contains("body suppressed"));
    }

    #[test]
    fn process_path_leaves_real_content_alone() {
        // Real (if brief) speech must flow through both paths unchanged.
        let transcript = "[0:00] Hello world\n[0:05] Goodbye\n".to_string();
        let health = sparse_health(0.001, 0.001);
        let initial = Some(OutputStatus::Complete);
        let (body, status, forced) =
            apply_suppression_on_process_path(transcript.clone(), Some(&health), initial);
        assert_eq!(body, transcript, "real content was clobbered");
        assert_eq!(status, initial, "status was downgraded without cause");
        assert!(forced.is_none(), "forced diagnosis set without suppression");
    }

    #[test]
    fn process_path_holds_off_without_recording_health() {
        // No diarization / no health captured (e.g. config.diarization.engine
        // = "none") must NOT suppress, even on an all-noise transcript: we
        // lack the corroborating evidence to override.
        let transcript = "[0:07] (crying)\n[1:52] [Growling]\n".to_string();
        let initial = Some(OutputStatus::TranscriptOnly);
        let (body, status, forced) =
            apply_suppression_on_process_path(transcript.clone(), None, initial);
        assert_eq!(body, transcript);
        assert_eq!(status, initial);
        assert!(forced.is_none());
    }

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
            processing_warnings: Vec::new(),
            template: None,
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
    fn instruction_echo_titles_are_detected() {
        // #401: these are meta-talk / prompt echoes, not titles.
        assert!(looks_like_instruction_echo(
            "I'll create a concise meeting title"
        ));
        assert!(looks_like_instruction_echo(
            "Here is a concise meeting title for you"
        ));
        assert!(looks_like_instruction_echo("Sure, here's a good title"));
        assert!(looks_like_instruction_echo(
            "Based on the summary, Q3 Planning"
        ));
        assert!(looks_like_instruction_echo("Meeting title: Q3 Planning"));
        assert!(looks_like_instruction_echo(
            "As an AI, I'd suggest Roadmap Review"
        ));
        // Legitimate titles must pass through untouched.
        assert!(!looks_like_instruction_echo(
            "Q3 Pricing Experiment Decision"
        ));
        assert!(!looks_like_instruction_echo("Consultant Billing Switch"));
        assert!(!looks_like_instruction_echo("Roadmap Review"));
        // "Here Comes..." is not the "here is/are" lead-in — must not trip.
        assert!(!looks_like_instruction_echo(
            "Here Comes the Sun Launch Plan"
        ));
    }

    #[test]
    fn instruction_echo_title_falls_back_and_logs_rejected_echo() {
        // #401 end-to-end: the exact echo that became a filename must be
        // rejected, fall back to the deterministic title, and log greppably.
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
                    title: "I'll create a concise meeting title:".into(),
                    model: "agent:claude".into(),
                    input_chars: 64,
                })
            },
        );

        assert_eq!(decision.final_title, "Roadmap Review");
        assert_eq!(decision.refined_title, None);
        assert_eq!(decision.outcome, "fallback");
        assert!(decision
            .detail
            .as_deref()
            .unwrap_or_default()
            .starts_with("rejected-echo:"));
    }

    #[test]
    fn algorithmic_fallback_still_works_standalone() {
        let title = generate_title("Hello.", None);
        assert_eq!(title, "Untitled Recording");
    }

    #[test]
    fn format_duration_secs_rounds_to_nearest_second() {
        assert_eq!(format_duration_secs(4313.6), "71m 54s");
        assert_eq!(format_duration_secs(59.6), "1m 0s");
        assert_eq!(format_duration_secs(0.4), "0s");
    }

    #[test]
    fn parse_transcript_line_starts_reads_minutes_and_hours() {
        let transcript = "[0:05] Intro\n[12:34] Update\n[1:02:03] Long call\nnot timestamped";

        assert_eq!(
            parse_transcript_line_starts(transcript),
            vec![5.0, 754.0, 3723.0]
        );
    }

    #[test]
    fn build_transcript_windows_synthesizes_end_times() {
        let transcript = "[0:00] One\n[0:03] Two\n[0:20] Three\n";

        let windows = build_transcript_windows(transcript, 24.0);

        assert_eq!(
            windows,
            vec![
                diarize::TranscriptWindow {
                    start_secs: 0.0,
                    end_secs: 3.0,
                },
                diarize::TranscriptWindow {
                    start_secs: 3.0,
                    end_secs: 11.0,
                },
                diarize::TranscriptWindow {
                    start_secs: 20.0,
                    end_secs: 24.0,
                },
            ]
        );
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

    fn action(assignee: &str) -> markdown::ActionItem {
        markdown::ActionItem {
            assignee: assignee.into(),
            task: "do a thing".into(),
            due: None,
            status: "open".into(),
        }
    }

    fn intent(kind: markdown::IntentKind, who: Option<&str>) -> markdown::Intent {
        markdown::Intent {
            kind,
            what: "something".into(),
            who: who.map(|w| w.to_string()),
            status: "open".into(),
            by_date: None,
        }
    }

    #[test]
    fn count_non_unknown_labels_excludes_unknown() {
        let labels = vec![
            "SPEAKER_1".to_string(),
            "Bobby".to_string(),
            "UNKNOWN".to_string(),
        ];
        assert_eq!(count_non_unknown_labels(&labels), 2);
        assert_eq!(count_non_unknown_labels(&["SPEAKER_1".to_string()]), 1);
        assert_eq!(count_non_unknown_labels(&[]), 0);
    }

    #[test]
    fn withhold_does_not_fire_when_two_labels_map_to_one_name() {
        // Codex false-positive class: diarization separated two speakers (the
        // summarizer saw SPEAKER_1 + SPEAKER_2), but both later map to one name.
        // The guard is fed the pre-mapping effective-label count (2), so it must
        // NOT fire even though the FINAL transcript would render one name.
        let mut actions = vec![action("Matt")];
        let mut intents = vec![intent(markdown::IntentKind::Commitment, Some("Matt"))];
        let warning = withhold_assignees_on_collapse(&mut actions, &mut intents, true, 2, 2, 2, 2);
        assert!(warning.is_none());
        assert_eq!(actions[0].assignee, "Matt");
    }

    #[test]
    fn withhold_fires_on_collapse_and_withholds_only_owned_intents() {
        let mut actions = vec![action("Matt"), action("unassigned")];
        let mut intents = vec![
            intent(markdown::IntentKind::ActionItem, Some("Matt")),
            intent(markdown::IntentKind::Commitment, Some("Matt")),
            intent(markdown::IntentKind::OpenQuestion, Some("Matt")),
            intent(markdown::IntentKind::Decision, Some("Matt")),
        ];
        // Meeting, diarization produced a result (2 raw), but rendered 1 label,
        // and 2 trusted attendees -> collapse.
        let warning = withhold_assignees_on_collapse(&mut actions, &mut intents, true, 2, 1, 2, 2);
        assert!(warning.is_some(), "expected a collapse warning");
        let warning = warning.unwrap();
        assert_eq!(warning.reason, "diarization_collapsed");
        assert_eq!(warning.step, "attribution");

        assert_eq!(actions[0].assignee, "unassigned", "named assignee withheld");
        assert_eq!(actions[1].assignee, "unassigned");
        assert_eq!(intents[0].who, None, "action-item owner cleared");
        assert_eq!(intents[1].who, None, "commitment owner cleared");
        assert_eq!(
            intents[2].who.as_deref(),
            Some("Matt"),
            "open-question owner is NOT in scope for #392"
        );
        assert_eq!(
            intents[3].who.as_deref(),
            Some("Matt"),
            "decision owner is NOT in scope for #392"
        );
    }

    #[test]
    fn withhold_does_not_fire_when_diarization_separated_speakers() {
        let mut actions = vec![action("Matt")];
        let mut intents = vec![intent(markdown::IntentKind::Commitment, Some("Matt"))];
        // 2 rendered labels = healthy attribution.
        let warning = withhold_assignees_on_collapse(&mut actions, &mut intents, true, 2, 2, 2, 2);
        assert!(warning.is_none());
        assert_eq!(actions[0].assignee, "Matt");
        assert_eq!(intents[0].who.as_deref(), Some("Matt"));
    }

    #[test]
    fn withhold_does_not_fire_when_diarization_disabled() {
        let mut actions = vec![action("Matt")];
        let mut intents = vec![intent(markdown::IntentKind::Commitment, Some("Matt"))];
        // diarization_num_speakers == 0 -> no diarization result -> do not fire.
        let warning = withhold_assignees_on_collapse(&mut actions, &mut intents, true, 0, 0, 2, 2);
        assert!(warning.is_none());
        assert_eq!(actions[0].assignee, "Matt");
    }

    #[test]
    fn withhold_does_not_fire_for_non_meeting_or_single_party() {
        // Not a meeting (e.g. memo/dictation).
        let mut a = vec![action("Matt")];
        let mut i: Vec<markdown::Intent> = vec![];
        assert!(withhold_assignees_on_collapse(&mut a, &mut i, false, 2, 1, 2, 2).is_none());
        assert_eq!(a[0].assignee, "Matt");
        // Single trusted attendee and single merged attendee -> not multi-party.
        let mut a = vec![action("Matt")];
        assert!(withhold_assignees_on_collapse(&mut a, &mut i, true, 2, 1, 1, 1).is_none());
        assert_eq!(a[0].assignee, "Matt");
    }

    #[test]
    fn withhold_uses_merged_attendees_only_when_no_trusted_list() {
        // No calendar/trusted attendees, but the LLM detected 2 participants.
        let mut actions = vec![action("Matt")];
        let mut intents: Vec<markdown::Intent> = vec![];
        let warning = withhold_assignees_on_collapse(&mut actions, &mut intents, true, 1, 1, 0, 2);
        assert!(
            warning.is_some(),
            "merged fallback fires when trusted list is empty"
        );
        assert_eq!(actions[0].assignee, "unassigned");
        // But a trusted list of 1 does NOT fall back to merged (avoids LLM self-proof).
        let mut actions = vec![action("Matt")];
        assert!(
            withhold_assignees_on_collapse(&mut actions, &mut intents, true, 1, 1, 1, 5).is_none(),
            "trusted list of 1 must not use merged fallback"
        );
        assert_eq!(actions[0].assignee, "Matt");
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
            Path::new("/fake.wav"),
            true,
            ContentType::Meeting,
            None,
            &config,
            &["Mat".into(), "Alex".into()],
            &["Mat".into(), "Alex".into()],
            2,
            true,
            false,
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
    fn degraded_ml_fallback_attribution_is_low_confidence_and_marked() {
        let mut config = Config::default();
        config.identity.name = Some("Mat".into());

        let result = attribute_meeting_speakers(
            Path::new("/fake.wav"),
            Path::new("/fake.wav"),
            true,
            ContentType::Meeting,
            None,
            &config,
            &["Mat".into(), "Alex".into()],
            &["Mat".into(), "Alex".into()],
            2,
            false,
            true,
            &std::collections::HashMap::new(),
            "[SPEAKER_0 0:00] hello\n[SPEAKER_1 0:01] hi\n".into(),
        );

        assert_eq!(result.speaker_map.len(), 2);
        assert!(result.speaker_map.iter().all(|entry| {
            entry.confidence == diarize::Confidence::Low
                && entry.source == diarize::AttributionSource::MlBleedDegraded
        }));
    }

    #[test]
    fn degraded_ml_recording_health_sets_recovery_path() {
        let health = degraded_ml_recording_health(diarize::DegradedCapture {
            failure_kind: diarize::FailureKind::Silent,
            capture_backend: "cpal".into(),
            capture_source: diarize::CaptureSource::System,
            voice_active_ratio: Some(0.9),
            system_active_ratio: Some(0.0),
            observed_signal: diarize::ObservedSignal {
                frames_captured: 120,
                max_rms: 0.0,
                avg_rms: 0.0,
            },
            diagnostic_confidence: diarize::DiagnosticConfidence::Inferred,
        });

        assert_eq!(
            health.diarization_path,
            Some(markdown::DiarizationPath::MlBleedDegraded)
        );
        assert_eq!(health.capture_warnings.len(), 1);
        assert_eq!(
            health.capture_warnings[0].source,
            diarize::CaptureSource::System
        );
        assert!(health.capture_warnings[0]
            .message
            .contains("low confidence"));
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
            Path::new("/Users/test/.minutes/native-captures/fake-call.mov"),
            true,
            ContentType::Meeting,
            Some("native-call"),
            &config,
            &[],
            &[],
            2,
            true,
            false,
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
    fn add_person_entity_strips_role_suffix_before_slug() {
        // Issue #370: "Junlei, tech lead" must produce slug "junlei", not "junlei-tech-lead".
        // The fix lives in pipeline.rs (extraction point) so the contaminated slug never
        // reaches the frontmatter file on disk.
        let entities = build_entity_links(
            "meeting",
            None,
            &[
                "Junlei, tech lead".into(),
                "Junrei (core team)".into(),
                "Sam - engineering lead".into(),
            ],
            &[],
            &[],
            &[],
            &[],
            None,
        );

        let slugs: Vec<&str> = entities.people.iter().map(|e| e.slug.as_str()).collect();
        assert!(slugs.contains(&"junlei"), "bare name present: {:?}", slugs);
        assert!(slugs.contains(&"junrei"), "bare name present: {:?}", slugs);
        assert!(slugs.contains(&"sam"), "bare name present: {:?}", slugs);
        assert!(
            !slugs.contains(&"junlei-tech-lead"),
            "role suffix must not appear in slug: {:?}",
            slugs
        );
        assert!(
            !slugs.contains(&"junrei-core-team"),
            "role suffix must not appear in slug: {:?}",
            slugs
        );
        assert!(
            !slugs.contains(&"sam-engineering-lead"),
            "role suffix must not appear in slug: {:?}",
            slugs
        );
    }

    #[test]
    fn add_person_entity_splits_multi_person_strings() {
        // #385 class 2: "Gert and Liam" must become two entities, not "gert-liam".
        let entities = build_entity_links(
            "meeting",
            None,
            &[
                "Gert and Liam".into(),
                "Liam & Joe".into(),
                "Sarah Chen".into(),
            ],
            &[],
            &[],
            &[],
            &[],
            None,
        );

        let slugs: Vec<&str> = entities.people.iter().map(|e| e.slug.as_str()).collect();
        assert!(slugs.contains(&"gert"), "gert present: {:?}", slugs);
        assert!(slugs.contains(&"liam"), "liam present: {:?}", slugs);
        assert!(slugs.contains(&"joe"), "joe present: {:?}", slugs);
        assert!(
            !slugs.contains(&"gert-liam"),
            "must not fuse multi-person: {:?}",
            slugs
        );
        assert!(
            !slugs.contains(&"liam-joe"),
            "must not fuse multi-person: {:?}",
            slugs
        );
        // A genuine two-word name must NOT be split (would appear as sarah + chen).
        assert!(
            slugs.contains(&"sarah-chen"),
            "real two-word name must stay intact: {:?}",
            slugs
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
            None,
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
            None,
        );

        let prompt = hints.whisper_initial_prompt().expect("prompt");
        assert!(!prompt.contains("Mathieu"));
        assert!(!prompt.contains("Matthew"));
        assert!(!prompt.contains("Mat,"));
        assert!(prompt.contains("Alex Chen"));
        assert!(prompt.contains("Casey"));
    }

    #[test]
    fn build_decode_hints_includes_bounded_vocabulary_terms() {
        let vocabulary = crate::vocabulary::VocabularyStore {
            entries: vec![
                crate::vocabulary::VocabularyEntry {
                    kind: crate::vocabulary::VocabularyKind::Organization,
                    canonical: "Automattic".into(),
                    aliases: vec!["Automatic".into()],
                    priority: crate::vocabulary::VocabularyPriority::High,
                    ..crate::vocabulary::VocabularyEntry::default()
                },
                crate::vocabulary::VocabularyEntry {
                    kind: crate::vocabulary::VocabularyKind::Project,
                    canonical: "Harper".into(),
                    priority: crate::vocabulary::VocabularyPriority::Normal,
                    ..crate::vocabulary::VocabularyEntry::default()
                },
            ],
        }
        .normalized()
        .unwrap();

        let hints = build_decode_hints(
            Some("Writing tools"),
            None,
            None,
            &["Elijah Potter".into()],
            None,
            Some(&vocabulary),
        );

        let prompt = hints.whisper_initial_prompt().expect("prompt");
        assert!(prompt.contains("Elijah Potter"));
        assert!(prompt.contains("Automattic"));
        assert!(prompt.contains("Automatic"));
        assert!(prompt.contains("Harper"));
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

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            identity: IdentityConfig {
                name: Some("Mat".into()),
                aliases: vec!["Matt".into()],
                ..IdentityConfig::default()
            },
            ..Config::default()
        };

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
    fn write_transcript_artifact_writes_consent_frontmatter() {
        let dir = tempfile::TempDir::new().unwrap();
        let audio_path = dir.path().join("consent.wav");
        std::fs::write(&audio_path, vec![0u8; 64_044]).unwrap();

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let context = BackgroundPipelineContext {
            consent: Some(crate::markdown::ConsentBasis::RecordedDisclosed),
            consent_notice: Some("Announced before recording.".into()),
            ..BackgroundPipelineContext::default()
        };

        let artifact = write_transcript_artifact(
            &audio_path,
            ContentType::Meeting,
            None,
            &config,
            &context,
            None,
            "[SPEAKER_1 0:00] We discussed the roadmap.\n".into(),
            crate::transcribe::FilterStats::default(),
            0,
        )
        .unwrap();

        assert_eq!(
            artifact.frontmatter.consent,
            Some(crate::markdown::ConsentBasis::RecordedDisclosed)
        );
        let written = std::fs::read_to_string(&artifact.write_result.path).unwrap();
        assert!(written.contains("consent: recorded_disclosed"));
        assert!(written.contains("consent_notice: Announced before recording."));
    }

    #[test]
    fn enrich_transcript_artifact_writes_degraded_native_call_banner() {
        let dir = tempfile::TempDir::new().unwrap();
        let audio_path = dir.path().join("recovered.system.wav");
        std::fs::write(&audio_path, vec![0u8; 64_044]).unwrap();

        let mut config = Config {
            output_dir: dir.path().join("meetings"),
            ..Config::default()
        };
        config.transcription.min_words = 1;
        config.summarization.engine = "none".into();
        config.diarization.engine = "none".into();
        let context = BackgroundPipelineContext {
            recording_health: Some(
                crate::health::recording_health_for_native_call_stem_recovery(
                    crate::diarize::CaptureSource::System,
                ),
            ),
            ..BackgroundPipelineContext::default()
        };

        let artifact = write_transcript_artifact(
            &audio_path,
            ContentType::Meeting,
            Some("Recovered call"),
            &config,
            &context,
            None,
            "[0:00] The remote participant confirmed the pricing decision.\n".into(),
            crate::transcribe::FilterStats::default(),
            0,
        )
        .unwrap();
        let result =
            enrich_transcript_artifact(&audio_path, &artifact, &config, &context, |_| {}).unwrap();

        let written = std::fs::read_to_string(&result.path).unwrap();
        assert!(written.contains("status: degraded"), "{written}");
        assert!(
            written.contains(crate::health::NATIVE_CALL_MICROPHONE_RECOVERY_CODE),
            "{written}"
        );
        assert!(written.contains("call/remote audio only"), "{written}");
        assert!(
            written.contains("reason: microphone_audio_not_captured"),
            "{written}"
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
            capture: None,
            sensitivity: None,
            debrief: None,
            consent: None,
            consent_notice: None,
            visibility: None,
            speaker_map: vec![diarize::SpeakerAttribution {
                speaker_label: "SPEAKER_1".into(),
                name: "Samantha".into(),
                confidence: diarize::Confidence::Medium,
                source: diarize::AttributionSource::Llm,
            }],
            name_corrections: Vec::new(),
            recording_health: None,
            speaker_mapping: None,
            processing_warnings: Vec::new(),
            template: None,
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
        let output = crate::engine_process::command("sh")
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

#[cfg(test)]
mod private_audio_temp_tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn external_same_uid_process_cannot_reopen_private_audio_through_proc() {
        use std::os::fd::AsRawFd;

        let ambient = tempfile::TempDir::new().unwrap();
        let mut audio =
            PrivateAudioTempFile::new_in(ambient.path(), "minutes-private-audio-", ".wav").unwrap();
        audio
            .prepare_for_write()
            .unwrap()
            .write_all(b"PRIVATE_AUDIO_PROC_CANARY")
            .unwrap();
        audio.finish_write().unwrap();

        let proc_path = format!("/proc/{}/fd/{}", std::process::id(), audio.file.as_raw_fd());
        let read = crate::engine_process::command(Path::new("/bin/sh"))
            .args([
                "-c",
                "exec 3< \"$1\"; dd bs=1 count=1 <&3 >/dev/null 2>&1",
                "sh",
                &proc_path,
            ])
            .status()
            .unwrap();
        assert!(
            !read.success(),
            "same-UID process reopened private audio for reading"
        );

        let write = crate::engine_process::command(Path::new("/bin/sh"))
            .args(["-c", "exec 3<> \"$1\"; printf X >&3", "sh", &proc_path])
            .status()
            .unwrap();
        assert!(
            !write.success(),
            "same-UID process reopened private audio for writing"
        );
        assert_eq!(
            read_registered_private_audio(&audio.processing_path()).unwrap(),
            b"PRIVATE_AUDIO_PROC_CANARY"
        );
    }

    #[test]
    #[cfg(windows)]
    fn windows_private_audio_uses_sealed_registry_storage_and_cleans_up() {
        assert!(private_audio_processing_supported());
        let ambient = tempfile::TempDir::new().unwrap();
        let mut audio =
            PrivateAudioTempFile::new_in(ambient.path(), "minutes-private-audio-", ".wav").unwrap();
        audio
            .prepare_for_write()
            .unwrap()
            .write_all(b"WINDOWS_SEALED_AUDIO_CANARY")
            .unwrap();
        audio.finish_write().unwrap();
        audio.verify_private_identity().unwrap();

        let processing_path = audio.processing_path();
        assert!(is_reserved_private_audio_path(&processing_path));
        assert_eq!(
            read_registered_private_audio(&processing_path).unwrap(),
            b"WINDOWS_SEALED_AUDIO_CANARY"
        );
        assert_eq!(
            private_audio_len(&processing_path).unwrap(),
            b"WINDOWS_SEALED_AUDIO_CANARY".len() as u64
        );

        drop(audio);
        assert!(private_audio_len(&processing_path).is_err());
        assert_eq!(std::fs::read_dir(ambient.path()).unwrap().count(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn private_audio_temps_are_unique_private_and_cleanup_exactly() {
        use std::os::unix::fs::MetadataExt;
        #[cfg(target_os = "linux")]
        use std::os::unix::fs::PermissionsExt;

        let ambient = tempfile::TempDir::new().unwrap();
        let mut first =
            PrivateAudioTempFile::new_in(ambient.path(), "minutes-private-audio-", ".wav").unwrap();
        let mut second =
            PrivateAudioTempFile::new_in(ambient.path(), "minutes-private-audio-", ".wav").unwrap();
        assert_ne!(first.as_path(), second.as_path());

        first
            .prepare_for_write()
            .unwrap()
            .write_all(b"first synthetic private audio")
            .unwrap();
        first.finish_write().unwrap();
        second
            .prepare_for_write()
            .unwrap()
            .write_all(b"second synthetic private audio")
            .unwrap();
        second.finish_write().unwrap();

        let first_path = first.as_path().to_path_buf();
        let second_path = second.as_path().to_path_buf();
        #[cfg(target_os = "linux")]
        assert_eq!(
            private_audio_metadata(&first_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(private_audio_metadata(&first_path).unwrap().nlink(), 0);
        first.verify_private_identity().unwrap();
        second.verify_private_identity().unwrap();
        assert_eq!(std::fs::read_dir(ambient.path()).unwrap().count(), 0);
        assert!(!first_path.exists());
        assert!(!second_path.exists());
        assert!(authorized_audio_stdin(&first_path).unwrap().is_some());
        assert!(authorized_audio_stdin(&second_path).unwrap().is_some());

        drop(first);
        let retired_error = match authorized_audio_stdin(&first_path) {
            Ok(_) => panic!("retired private-audio capability must be denied"),
            Err(error) => error,
        };
        assert_eq!(retired_error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!retired_error
            .to_string()
            .contains(&first_path.display().to_string()));
        assert_eq!(private_audio_diagnostic_label(&first_path), "private-audio");
        assert!(
            !crate::diarize::stem_has_audio(&first_path),
            "a retired capability must not fall through to an ambient pathname probe"
        );
        assert!(authorized_audio_stdin(&second_path).unwrap().is_some());
        drop(second);
        assert_eq!(
            authorized_audio_stdin(&second_path).err().unwrap().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(std::fs::read_dir(ambient.path()).unwrap().count(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn private_audio_readers_have_independent_positional_cursors() {
        let ambient = tempfile::TempDir::new().unwrap();
        let mut temp =
            PrivateAudioTempFile::new_in(ambient.path(), "minutes-private-audio-", ".wav").unwrap();
        temp.prepare_for_write()
            .unwrap()
            .write_all(b"0123456789")
            .unwrap();
        temp.finish_write().unwrap();

        let mut first = authorized_audio_stdin(temp.as_path()).unwrap().unwrap();
        let mut second = authorized_audio_stdin(temp.as_path()).unwrap().unwrap();
        let mut first_prefix = [0_u8; 4];
        let mut second_prefix = [0_u8; 4];
        first.read_exact(&mut first_prefix).unwrap();
        second.read_exact(&mut second_prefix).unwrap();
        assert_eq!(&first_prefix, b"0123");
        assert_eq!(&second_prefix, b"0123");

        first.seek(std::io::SeekFrom::Start(8)).unwrap();
        let mut tail = [0_u8; 2];
        first.read_exact(&mut tail).unwrap();
        assert_eq!(&tail, b"89");

        let mut second_next = [0_u8; 2];
        second.read_exact(&mut second_next).unwrap();
        assert_eq!(&second_next, b"45");
        assert_eq!(std::fs::read_dir(ambient.path()).unwrap().count(), 0);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn anonymous_generation_rejects_finish_reset_and_stale_reader_overlap() {
        let ambient = tempfile::TempDir::new().unwrap();
        let mut temp =
            PrivateAudioTempFile::new_in(ambient.path(), "minutes-generation-", ".wav").unwrap();
        let path = temp.processing_path();
        let mut writer = temp.prepare_for_write().unwrap();
        writer.write_all(b"sealed generation").unwrap();

        assert!(
            temp.finish_write().is_err(),
            "an active writer must block seal"
        );
        drop(writer);
        temp.finish_write().unwrap();

        let mut reader = temp.try_clone_reader().unwrap();
        assert!(
            temp.prepare_for_write().is_err(),
            "an active reader must block generation reset"
        );
        let retirement = temp
            .discard_failed_write()
            .expect_err("failed exclusive reset must retire the capability");
        assert!(retirement.to_string().contains("capability retired"));
        assert_eq!(
            private_audio_len(&path).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        let mut byte = [0_u8; 1];
        assert_eq!(
            reader.read(&mut byte).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied,
            "a reader from the retired generation must fail closed"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn anonymous_failed_reset_revokes_detached_writer_lease() {
        let ambient = tempfile::TempDir::new().unwrap();
        let mut temp =
            PrivateAudioTempFile::new_in(ambient.path(), "minutes-writer-retire-", ".wav").unwrap();
        let path = temp.processing_path();
        let mut writer = temp.prepare_for_write().unwrap();
        writer.write_all(b"partial").unwrap();

        let retirement = temp
            .discard_failed_write()
            .expect_err("an active detached writer must force capability retirement");
        assert!(retirement.to_string().contains("capability retired"));
        assert_eq!(
            writer.write_all(b"must-not-land").unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        let retired = match registered_private_audio_reader(&path) {
            Ok(_) => panic!("retired writer capability must not resolve"),
            Err(error) => error,
        };
        assert_eq!(retired.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn private_audio_child_lease_fails_closed_across_exec() {
        let ambient = tempfile::TempDir::new().unwrap();
        let mut temp =
            PrivateAudioTempFile::new_in(ambient.path(), "minutes-private-audio-", ".wav").unwrap();
        temp.prepare_for_write()
            .unwrap()
            .write_all(b"PRIVATE_AUDIO_CANARY")
            .unwrap();
        temp.finish_write().unwrap();
        let error = match temp.child_lease() {
            Ok(_) => panic!("raw private audio must never cross ordinary exec"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("across exec"));
        assert_eq!(std::fs::read_dir(ambient.path()).unwrap().count(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn private_audio_output_has_no_swappable_leaf_and_ignores_legacy_symlink() {
        use std::os::unix::fs::symlink;

        let ambient = tempfile::TempDir::new().unwrap();
        let canary = ambient.path().join("RAW_AUDIO_CANARY");
        std::fs::write(&canary, b"private").unwrap();
        let legacy = ambient
            .path()
            .join(format!("minutes-ffmpeg-{}.wav", std::process::id()));
        symlink(&canary, &legacy).unwrap();

        let mut temp =
            PrivateAudioTempFile::new_in(ambient.path(), "minutes-ffmpeg-", ".wav").unwrap();
        assert_ne!(temp.as_path(), legacy);

        // There is no temporary leaf to replace before the simulated decoder
        // emits bytes: only the retained descriptor capability exists.
        assert_eq!(std::fs::read_dir(ambient.path()).unwrap().count(), 2);
        let mut command = crate::bounded_child::BoundedCommand::new("sh");
        command.args([
            "-c",
            "i=0; while [ $i -lt 6000 ]; do printf 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef >&2; i=$((i + 1)); done; printf decoded",
        ]);
        let output = output_with_authorized_audio_stdin_to_private_file_with_budget(
            &mut command,
            None,
            &mut temp,
            MAX_AUTHORIZED_PROCESS_AUDIO_BYTES,
            std::time::Duration::from_secs(30 * 60),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stderr.len(), MAX_PRIVATE_AUDIO_CHILD_STDERR_BYTES);
        assert_eq!(std::fs::read(&canary).unwrap(), b"private");

        let mut retained = temp.try_clone_reader().unwrap();
        let mut decoded = Vec::new();
        retained.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, b"decoded");

        drop(temp);
        assert!(std::fs::symlink_metadata(&legacy)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&canary).unwrap(), b"private");
    }

    #[test]
    #[cfg(unix)]
    fn private_audio_output_budget_failure_zeroes_the_exact_destination() {
        let ambient = tempfile::TempDir::new().unwrap();
        let mut temp =
            PrivateAudioTempFile::new_in(ambient.path(), "minutes-ffmpeg-", ".wav").unwrap();
        let mut command = crate::bounded_child::BoundedCommand::new("sh");
        command.args(["-c", "printf too-many-bytes"]);

        let error = output_with_authorized_audio_stdin_to_private_file_with_budget(
            &mut command,
            None,
            &mut temp,
            4,
            std::time::Duration::from_secs(5),
        )
        .unwrap_err();

        assert!(error.to_string().contains("resource budget"));
        assert_eq!(private_audio_len(temp.as_path()).unwrap(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn private_audio_child_nonzero_exit_discards_partial_output() {
        let ambient = tempfile::TempDir::new().unwrap();
        let mut temp =
            PrivateAudioTempFile::new_in(ambient.path(), "minutes-ffmpeg-", ".wav").unwrap();
        let mut command = crate::bounded_child::BoundedCommand::new("sh");
        command.args(["-c", "printf partial-output; exit 7"]);

        let output = output_with_authorized_audio_stdin_to_private_file_with_budget(
            &mut command,
            None,
            &mut temp,
            MAX_AUTHORIZED_PROCESS_AUDIO_BYTES,
            std::time::Duration::from_secs(30 * 60),
        )
        .unwrap();

        assert_eq!(output.status.code(), Some(7));
        assert_eq!(private_audio_len(temp.as_path()).unwrap(), 0);
        assert!(
            temp.prepare_for_write().is_ok(),
            "discarded output must be reusable"
        );
    }

    #[test]
    #[cfg(any(target_os = "macos", windows))]
    fn failed_destination_reset_retires_the_opaque_capability() {
        let ambient = tempfile::TempDir::new().unwrap();
        let mut temp =
            PrivateAudioTempFile::new_in(ambient.path(), "minutes-retire-failed-", ".wav").unwrap();
        let path = temp.processing_path();
        let writer = temp.prepare_for_write().unwrap();

        let error = temp
            .discard_failed_write()
            .expect_err("an active sealed writer must prevent reset");
        assert!(error.to_string().contains("capability retired"));
        let retired = match registered_private_audio_reader(&path) {
            Ok(_) => panic!("retired capability must not resolve"),
            Err(error) => error,
        };
        assert_eq!(retired.kind(), std::io::ErrorKind::PermissionDenied);
        drop(writer);
    }
}

/// Tests for `prepare_transcription_input`: the helper that decides whether
/// the input `.mov` needs stem-mixing before transcription (#234 fix, #235 v2
/// review items #3 stem-lookup correctness, #4 typed error, #6 shared between
/// `process_with_progress_and_sidecar` and `transcribe_to_artifact`).
#[cfg(test)]
mod prepare_transcription_input_tests {
    use super::*;
    use std::fs;

    #[cfg(all(unix, not(feature = "whisper")))]
    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    #[cfg(all(unix, not(feature = "whisper")))]
    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    #[cfg(all(unix, not(feature = "whisper")))]
    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    /// Write a 1-second audible-tone WAV at 16kHz mono s16. We need a non-
    /// silent signal because `stem_has_audio` (via `discover_stem_plan`)
    /// probes RMS and rejects anything below 0.001, which pure silence
    /// fails. A 440 Hz sine at amplitude 5000 (s16) gives an RMS of
    /// ~0.108 normalized, well above the floor.
    fn write_audible_wav(path: &std::path::Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        let two_pi_over_period = 2.0 * std::f32::consts::PI * 440.0 / 16_000.0;
        for n in 0..16_000 {
            let sample = (5000.0 * (n as f32 * two_pi_over_period).sin()) as i16;
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[cfg(all(unix, not(feature = "whisper")))]
    fn write_audible_pcm_s16le(path: &std::path::Path) {
        let two_pi_over_period = 2.0 * std::f32::consts::PI * 440.0 / 16_000.0;
        let mut bytes = Vec::with_capacity(16_000 * std::mem::size_of::<i16>());
        for n in 0..16_000 {
            let sample = (5000.0 * (n as f32 * two_pi_over_period).sin()) as i16;
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        fs::write(path, bytes).unwrap();
    }

    fn write_digital_silence_wav(path: &std::path::Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..16_000 {
            writer.write_sample(-0.000_028_f32).unwrap();
        }
        writer.finalize().unwrap();
    }

    /// Build the `<name>.mov` + `<name>.voice.wav` + `<name>.system.wav`
    /// trio that a native-call capture produces. The `.mov` itself is a
    /// 1-byte stub because `prepare_transcription_input` sniffs only the
    /// extension and the sibling stems, never the `.mov` content.
    fn fake_native_call_capture(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let mov = dir.path().join(format!("{}.mov", name));
        let voice = dir.path().join(format!("{}.voice.wav", name));
        let system = dir.path().join(format!("{}.system.wav", name));
        fs::write(&mov, b"x").unwrap();
        write_audible_wav(&voice);
        write_audible_wav(&system);
        (dir, mov)
    }

    fn ffmpeg_available() -> bool {
        let Ok(ffmpeg) = crate::ffmpeg::resolve_ffmpeg() else {
            return false;
        };
        crate::engine_process::command(ffmpeg)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(all(unix, not(feature = "whisper")))]
    fn with_deterministic_native_mix(run: impl FnOnce(&Path, &Config)) {
        use std::os::unix::fs::PermissionsExt;

        let _env_lock = crate::test_home_env_lock();
        let (capture_dir, mov) = fake_native_call_capture("typed-dispatch");
        let mixed_fixture = capture_dir.path().join("mixed-fixture.s16le");
        write_audible_pcm_s16le(&mixed_fixture);

        let fake_ffmpeg = capture_dir.path().join("fake-ffmpeg");
        let fixture = mixed_fixture.to_string_lossy().replace('\'', "'\\''");
        fs::write(
            &fake_ffmpeg,
            format!("#!/bin/sh\nexec /bin/cat -- '{fixture}'\n"),
        )
        .unwrap();
        fs::set_permissions(&fake_ffmpeg, fs::Permissions::from_mode(0o700)).unwrap();

        let home = capture_dir.path().join("home");
        fs::create_dir(&home).unwrap();
        let _home = EnvVarGuard::set("HOME", &home);
        let _ffmpeg = EnvVarGuard::set("MINUTES_FFMPEG", &fake_ffmpeg);
        let mut config = Config {
            output_dir: capture_dir.path().join("meetings"),
            ..Config::default()
        };
        config.summarization.engine = "none".into();
        config.diarization.engine = "none".into();

        run(&mov, &config);
    }

    #[cfg(all(unix, not(feature = "whisper")))]
    fn assert_typed_mix_reached_transcriber(transcript: &str) {
        assert!(transcript.contains("Transcription placeholder"));
        assert!(transcript.contains("Audio file: private-audio"));
        assert!(!transcript.contains("typed authorized entry point"));
    }

    #[test]
    fn returns_ok_none_for_non_mov_input() {
        let dir = tempfile::TempDir::new().unwrap();
        let wav = dir.path().join("voice-memo.wav");
        write_audible_wav(&wav);
        let result = prepare_transcription_input(&wav).expect("non-.mov should not error");
        assert!(
            matches!(result, PreparedTranscriptionInput::Original),
            ".wav input must remain unchanged"
        );
    }

    #[test]
    fn returns_ok_none_for_mov_with_no_stems() {
        // Plain `.mov` with no sibling stems: could be a screen recording,
        // downloaded file, or a native-call capture whose stems were
        // cleaned up. We cannot distinguish, so we let it through to the
        // existing decoder rather than hard-erroring on every stemless
        // `.mov` (which would break legitimate non-native-call use cases).
        let dir = tempfile::TempDir::new().unwrap();
        let mov = dir.path().join("screen-recording.mov");
        fs::write(&mov, b"x").unwrap();
        let result = prepare_transcription_input(&mov).expect("stemless .mov should not error");
        assert!(
            matches!(result, PreparedTranscriptionInput::Original),
            "stemless .mov must remain unchanged; hard-erroring would break non-native-call .mov use"
        );
    }

    #[test]
    fn mixes_stems_when_both_present_and_valid() {
        let _env_lock = crate::test_home_env_lock();
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        let (_dir, mov) = fake_native_call_capture("call-clean");
        let result = prepare_transcription_input(&mov)
            .expect("mix must succeed when both stems are valid PCM");
        let PreparedTranscriptionInput::Mixed(handle) = result else {
            panic!("both valid stems must be mixed")
        };
        let mixed_capability = handle.processing_path().to_path_buf();
        assert!(is_reserved_private_audio_path(&mixed_capability));
        assert!(
            !mixed_capability.exists(),
            "mixed PCM must not be published as a plaintext filesystem path"
        );

        // Parse the exact retained capability with the same WAV parser used by
        // transcription. Checking only RIFF/WAVE magic misses FFmpeg's
        // non-seekable `0xffff_ffff` data-length placeholder, which is the
        // signed-Mac runtime regression this test exists to prevent.
        let reader = authorized_audio_stdin(handle.processing_path())
            .unwrap()
            .expect("mixed authority must resolve its retained reader");
        let mut wav = hound::WavReader::new(reader)
            .expect("two-stem FFmpeg output must be a bounded, parseable WAV");
        assert_eq!(wav.spec().channels, 1);
        assert_eq!(wav.spec().sample_rate, 16_000);
        assert_eq!(wav.spec().bits_per_sample, 16);
        assert_eq!(wav.duration(), 16_000);
        let mut samples = 0_u32;
        for sample in wav.samples::<i16>() {
            sample.expect("every mixed sample must decode");
            samples += 1;
        }
        assert_eq!(samples, 16_000);
        drop(wav);

        // Drop must retire the opaque registry capability.
        drop(handle);
        assert!(
            registered_private_audio_reader(&mixed_capability).is_err(),
            "dropped mixed audio must not remain resolvable"
        );
    }

    #[test]
    fn private_pcm_wrapper_rejects_partial_s16_sample() {
        let mut raw = PrivateAudioTempFile::new("minutes-odd-pcm-", ".s16le").unwrap();
        {
            let mut writer = raw.prepare_for_write().unwrap();
            writer.write_all(&[1_u8, 2, 3]).unwrap();
        }
        raw.finish_write().unwrap();

        let error = private_pcm_s16le_mono_to_wav(&raw, 16_000)
            .err()
            .expect("partial samples must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("complete sample"));
    }

    #[test]
    fn private_pcm_wrapper_rejects_empty_output_and_raw_capability_retires() {
        let mut raw = PrivateAudioTempFile::new("minutes-empty-pcm-", ".s16le").unwrap();
        {
            let writer = raw.prepare_for_write().unwrap();
            drop(writer);
        }
        raw.finish_write().unwrap();
        let raw_capability = raw.processing_path();

        let error = private_pcm_s16le_mono_to_wav(&raw, 16_000)
            .err()
            .expect("empty PCM must fail closed before WAV authorization");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("empty"));
        assert!(
            registered_private_audio_reader(&raw_capability)
                .unwrap()
                .is_some(),
            "the caller-owned raw capability remains exact until its owner drops"
        );

        drop(raw);
        assert!(
            registered_private_audio_reader(&raw_capability).is_err(),
            "failed raw capability must retire when its owner drops"
        );
    }

    #[test]
    #[cfg(all(unix, not(feature = "whisper")))]
    fn foreground_two_stem_mix_reaches_typed_authorized_transcription() {
        with_deterministic_native_mix(|mov, config| {
            let result = process(mov, ContentType::Memo, Some("Typed foreground mix"), config)
                .expect("typed mixed audio must pass the ambient-token guard");
            let rendered = fs::read_to_string(result.path).unwrap();
            assert_typed_mix_reached_transcriber(&rendered);
        });
    }

    #[test]
    #[cfg(all(unix, not(feature = "whisper")))]
    fn background_two_stem_mix_reaches_typed_authorized_transcription() {
        with_deterministic_native_mix(|mov, config| {
            let artifact = transcribe_to_artifact(
                mov,
                ContentType::Memo,
                Some("Typed background mix"),
                config,
                &BackgroundPipelineContext::default(),
                None,
            )
            .expect("typed mixed audio must pass the ambient-token guard");
            assert_typed_mix_reached_transcriber(&artifact.transcript);
        });
    }

    #[test]
    fn degrades_to_system_stem_when_voice_stem_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let mov = dir.path().join("partial-voice.mov");
        let system = dir.path().join("partial-voice.system.wav");
        fs::write(&mov, b"x").unwrap();
        write_audible_wav(&system);
        // voice.wav deliberately not created — simulates a partial-crash
        // where the system side survived but the mic side did not.

        let result = prepare_transcription_input(&mov).expect("system stem is recoverable");
        let canonical_system = system.canonicalize().unwrap();
        assert_eq!(
            result.diarization_audio_path(),
            Some(canonical_system.as_path()),
            "downstream diarization must reuse the selected survivor"
        );
        let PreparedTranscriptionInput::SingleStem {
            path,
            recording_health,
        } = result
        else {
            panic!("missing voice must select surviving system stem")
        };
        assert_eq!(path, canonical_system);
        assert_eq!(
            recording_health.capture_warnings[0].source,
            crate::diarize::CaptureSource::System
        );
        assert!(recording_health.capture_warnings[0]
            .message
            .contains("call/remote audio only"));
    }

    #[test]
    fn degrades_to_voice_stem_when_system_stem_file_absent() {
        // Codex review of PR #235 v2 caught this: `discover_stem_plan`
        // returns None for both the "no stems at all" case AND the
        // "voice ok, system absent from disk" case. The second case is
        // a partial-crash native capture where the system stem was lost
        // during recording, and falling through to the broken `.mov`
        // decoder reproduces the exact 2x bug this helper prevents.
        //
        // The fix distinguishes the two None cases by independently
        // checking for a usable sibling voice stem in
        // `prepare_transcription_input`. This test pins that contract.
        let dir = tempfile::TempDir::new().unwrap();
        let mov = dir.path().join("partial-system-missing.mov");
        let voice = dir.path().join("partial-system-missing.voice.wav");
        fs::write(&mov, b"x").unwrap();
        write_audible_wav(&voice);
        // system.wav deliberately not created (not even zero-byte; the
        // file doesn't exist on disk at all). This is the case that
        // returned None from discover_stem_plan and would have silently
        // fallen through to the broken `.mov` decode without this fix.

        let result = prepare_transcription_input(&mov).expect("voice stem is recoverable");
        let canonical_voice = voice.canonicalize().unwrap();
        assert_eq!(
            result.diarization_audio_path(),
            Some(canonical_voice.as_path()),
            "voice-only recovery must not diarize the original .mov"
        );
        let PreparedTranscriptionInput::SingleStem {
            path,
            recording_health,
        } = result
        else {
            panic!("missing system must select surviving voice stem")
        };
        assert_eq!(path, canonical_voice);
        assert_eq!(
            recording_health.capture_warnings[0].source,
            crate::diarize::CaptureSource::Voice
        );
    }

    #[test]
    fn degrades_to_voice_when_system_stem_is_zero_byte_partial_crash() {
        let dir = tempfile::TempDir::new().unwrap();
        let mov = dir.path().join("partial-system.mov");
        let voice = dir.path().join("partial-system.voice.wav");
        let system = dir.path().join("partial-system.system.wav");
        fs::write(&mov, b"x").unwrap();
        write_audible_wav(&voice);
        // Zero-byte system stem: simulates a partial-crash where the file
        // got created but never finalized. `.exists()` would accept this;
        // `stem_has_audio` (which discover_stem_plan invokes) catches it.
        fs::write(&system, b"").unwrap();

        let result = prepare_transcription_input(&mov).expect("voice stem is recoverable");
        let canonical_voice = voice.canonicalize().unwrap();
        assert!(matches!(
            result,
            PreparedTranscriptionInput::SingleStem { path, .. } if path == canonical_voice
        ));
    }

    #[test]
    fn full_duration_digital_silence_voice_degrades_to_system() {
        let dir = tempfile::TempDir::new().unwrap();
        let mov = dir.path().join("silent-microphone.mov");
        let voice = dir.path().join("silent-microphone.voice.wav");
        let system = dir.path().join("silent-microphone.system.wav");
        fs::write(&mov, b"x").unwrap();
        write_digital_silence_wav(&voice);
        write_audible_wav(&system);

        let result = prepare_transcription_input(&mov).expect("system stem is recoverable");
        let PreparedTranscriptionInput::SingleStem {
            path,
            recording_health,
        } = result
        else {
            panic!("digital-silence microphone must select system stem")
        };
        assert_eq!(
            path,
            system.canonicalize().unwrap(),
            "transcriber must receive the audible stem"
        );
        let warning = &recording_health.capture_warnings[0];
        assert_eq!(warning.source, crate::diarize::CaptureSource::System);
        assert!(matches!(
            &warning.kind,
            crate::diarize::FailureKind::Other { code }
                if code == crate::health::NATIVE_CALL_MICROPHONE_RECOVERY_CODE
        ));
    }

    #[test]
    fn full_duration_digital_silence_system_degrades_to_voice() {
        let dir = tempfile::TempDir::new().unwrap();
        let mov = dir.path().join("silent-system.mov");
        let voice = dir.path().join("silent-system.voice.wav");
        let system = dir.path().join("silent-system.system.wav");
        fs::write(&mov, b"x").unwrap();
        write_audible_wav(&voice);
        write_digital_silence_wav(&system);

        let result = prepare_transcription_input(&mov).expect("voice stem is recoverable");
        let PreparedTranscriptionInput::SingleStem {
            path,
            recording_health,
        } = result
        else {
            panic!("digital-silence system stem must select microphone stem")
        };
        assert_eq!(path, voice.canonicalize().unwrap());
        let warning = &recording_health.capture_warnings[0];
        assert_eq!(warning.source, crate::diarize::CaptureSource::Voice);
        assert!(matches!(
            &warning.kind,
            crate::diarize::FailureKind::Other { code }
                if code == crate::health::NATIVE_CALL_SYSTEM_RECOVERY_CODE
        ));
    }

    #[test]
    fn errors_when_both_stems_are_digitally_silent() {
        let dir = tempfile::TempDir::new().unwrap();
        let mov = dir.path().join("both-silent.mov");
        let voice = dir.path().join("both-silent.voice.wav");
        let system = dir.path().join("both-silent.system.wav");
        fs::write(&mov, b"x").unwrap();
        write_digital_silence_wav(&voice);
        write_digital_silence_wav(&system);

        let result = prepare_transcription_input(&mov);
        assert!(matches!(
            result,
            Err(MinutesError::Transcribe(
                crate::error::TranscribeError::NativeCaptureStemMixUnavailable { .. }
            ))
        ));
    }

    #[test]
    fn background_stem_validation_recovers_valid_system_sibling_without_mov_fallback() {
        let dir = tempfile::TempDir::new().unwrap();
        let mov = dir.path().join("invalid-member.mov");
        let voice = dir.path().join("invalid-member.voice.wav");
        let system = dir.path().join("invalid-member.system.wav");
        fs::write(&mov, b"mov must never be decoded").unwrap();
        fs::write(&voice, vec![b'x'; 200_000]).unwrap();
        write_audible_wav(&system);

        let result = prepare_transcription_input(&mov).expect("valid system survivor");
        let PreparedTranscriptionInput::SingleStem {
            path,
            recording_health,
        } = result
        else {
            panic!("invalid voice plus valid system must select system survivor")
        };
        assert_eq!(path, system.canonicalize().unwrap());
        assert!(recording_health.capture_warnings.iter().any(|warning| {
            matches!(&warning.kind, crate::diarize::FailureKind::Other { code }
                if code == crate::health::NATIVE_CALL_INVALID_STEM_CODE)
                && warning.source == crate::diarize::CaptureSource::Voice
                && warning.message.contains("invalid or corrupt")
        }));
        assert!(mov.exists());
        assert!(voice.exists());
        assert!(system.exists());
    }

    #[test]
    fn background_stem_validation_recovers_valid_voice_sibling_without_mov_fallback() {
        let dir = tempfile::TempDir::new().unwrap();
        let mov = dir.path().join("invalid-system.mov");
        let voice = dir.path().join("invalid-system.voice.wav");
        let system = dir.path().join("invalid-system.system.wav");
        fs::write(&mov, b"mov must never be decoded").unwrap();
        write_audible_wav(&voice);
        fs::write(&system, vec![b'x'; 200_000]).unwrap();

        let result = prepare_transcription_input(&mov).expect("valid voice survivor");
        let PreparedTranscriptionInput::SingleStem {
            path,
            recording_health,
        } = result
        else {
            panic!("valid voice plus invalid system must select voice survivor")
        };
        assert_eq!(path, voice.canonicalize().unwrap());
        assert!(recording_health.capture_warnings.iter().any(|warning| {
            matches!(&warning.kind, crate::diarize::FailureKind::Other { code }
                if code == crate::health::NATIVE_CALL_INVALID_STEM_CODE)
                && warning.source == crate::diarize::CaptureSource::System
                && warning.message.contains("invalid or corrupt")
        }));
        assert!(mov.exists());
        assert!(voice.exists());
        assert!(system.exists());
    }

    #[cfg(unix)]
    #[test]
    fn canonicalizes_symlinked_mov_to_find_stems() {
        let _env_lock = crate::test_home_env_lock();
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not on PATH");
            return;
        }
        // The .mov plus its stems live in one tempdir; the symlink lives
        // in another. discover_stem_plan called on the un-canonicalized
        // symlink path would look for stems in the wrong directory and
        // return None, which would map to Ok(None) and bypass the fix.
        // Canonicalize must run before stem lookup.
        let (target_dir, target_mov) = fake_native_call_capture("real-call");
        let link_dir = tempfile::TempDir::new().unwrap();
        let link = link_dir.path().join("aliased.mov");
        std::os::unix::fs::symlink(&target_mov, &link)
            .expect("symlink creation must succeed on unix");

        let result = prepare_transcription_input(&link)
            .expect("symlink resolution must succeed and stems must be found");
        let PreparedTranscriptionInput::Mixed(handle) = result else {
            panic!("symlinked .mov must resolve and mix its stems")
        };
        assert!(
            authorized_audio_stdin(handle.processing_path())
                .unwrap()
                .is_some(),
            "mixed PCM capability must remain registered post-mix"
        );

        // Drop the handle before the tempdirs so cleanup is observable.
        drop(handle);
        drop(target_dir);
        drop(link_dir);
    }
}
