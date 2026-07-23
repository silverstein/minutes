use crate::calendar::CalendarEvent;
use crate::config::Config;
use crate::error::{MinutesError, TranscribeError};
use crate::markdown::{ContentType, OutputStatus};
use crate::pid::{self, CaptureMode, PidGuard};
use crate::pipeline::{self, BackgroundPipelineContext, PipelineStage};
use chrono::{DateTime, Local};
use serde_json::json;
use std::fs;
use std::io::ErrorKind;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Upper bound on automatic stale-claim retries before a job is marked
/// permanently `Failed`. Protects against deterministic crashes (issue #229:
/// a SIGABRT inside whisper.cpp / `__cxa_finalize_ranges` left the job
/// orphaned with `owner_pid` set; `list_jobs()` would reset to `Queued` and
/// the next worker would crash the same way, in a loop).
///
/// The counter advances each time a non-terminal job is observed with a dead
/// `owner_pid`. A user-initiated `requeue_job` resets it so a manual retry
/// after fixing the underlying issue always gets a fresh budget.
pub const MAX_AUTO_RETRIES: u32 = 2;

static JOB_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Sentinel filename inside `jobs_dir()` recording that the one-shot upgrade
/// migration of pre-existing terminal jobs into `archive/` has run. Sentinel
/// file rather than a `OnceLock` because (a) the marker survives process
/// restarts so a CLI invocation followed by an app launch don't both run the
/// sweep, and (b) per-test temp `HOME`s get fresh migration semantics
/// without sharing process-global state. Bumped if the migration ever needs
/// to re-run for a future schema change.
const MIGRATION_MARKER: &str = ".archive-initialized-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobState {
    Queued,
    Transcribing,
    TranscriptOnly,
    Diarizing,
    Summarizing,
    Saving,
    NeedsReview,
    Complete,
    Failed,
}

impl JobState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::NeedsReview | Self::Complete | Self::Failed)
    }

    pub fn default_stage(self) -> Option<String> {
        match self {
            Self::Queued => Some("Queued for processing".into()),
            Self::Transcribing => Some("Transcribing meeting".into()),
            Self::TranscriptOnly => Some("Transcript ready, enriching artifact".into()),
            Self::Diarizing => Some("Separating speakers".into()),
            Self::Summarizing => Some("Generating summary".into()),
            Self::Saving => Some("Saving artifact".into()),
            Self::NeedsReview => Some("Needs review — raw capture preserved".into()),
            Self::Complete => None,
            Self::Failed => Some("Processing failed".into()),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProcessingJob {
    pub id: String,
    pub mode: CaptureMode,
    pub content_type: ContentType,
    pub title: Option<String>,
    pub audio_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    pub state: JobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    pub created_at: DateTime<Local>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Local>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Local>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice_dismissed_at: Option<DateTime<Local>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_started_at: Option<DateTime<Local>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_finished_at: Option<DateTime<Local>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_context: Option<String>,
    /// Consent basis captured at record start, if one was provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<crate::markdown::ConsentBasis>,
    /// Exact disclosure text captured at record start, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_notice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_event: Option<CalendarEvent>,
    /// Slug of the template selected at record time, if any. Read by the
    /// queue worker so the pipeline applies the same template the user
    /// chose when starting the recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_slug: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_health: Option<crate::markdown::RecordingHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_pid: Option<u32>,
    /// Number of times this job has been auto-recovered from a stale
    /// `owner_pid` claim. `list_jobs()` increments this each time it observes
    /// a non-terminal job whose worker is dead; once it reaches
    /// `MAX_AUTO_RETRIES` the job is demoted to `Failed` instead of being
    /// reset to `Queued`, so a deterministic worker crash cannot loop
    /// forever (issue #229). Reset to 0 by `requeue_job` on a manual retry.
    #[serde(default)]
    pub retry_count: u32,
}

fn next_job_id() -> String {
    let ts = Local::now().format("%Y%m%d%H%M%S%3f");
    let pid = std::process::id();
    let counter = JOB_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("job-{}-{}-{}", ts, pid, counter)
}

/// Best-effort string form of a `catch_unwind` payload. Most panics carry a
/// `&'static str` or a `String`; fall back to a generic label otherwise so
/// the failed-job record still has something meaningful for the user.
fn describe_panic_payload(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic payload was not a string".to_string()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn queue_live_capture(
    mode: CaptureMode,
    title: Option<String>,
    current_wav: &Path,
    user_notes: Option<String>,
    pre_context: Option<String>,
    recording_started_at: Option<DateTime<Local>>,
    recording_finished_at: Option<DateTime<Local>>,
    context_session_id: Option<String>,
    calendar_event: Option<CalendarEvent>,
    template_slug: Option<String>,
) -> std::io::Result<ProcessingJob> {
    queue_live_capture_with_recording_health(
        mode,
        title,
        current_wav,
        user_notes,
        pre_context,
        recording_started_at,
        recording_finished_at,
        context_session_id,
        calendar_event,
        template_slug,
        None,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn queue_live_capture_with_recording_health(
    mode: CaptureMode,
    title: Option<String>,
    current_wav: &Path,
    user_notes: Option<String>,
    pre_context: Option<String>,
    recording_started_at: Option<DateTime<Local>>,
    recording_finished_at: Option<DateTime<Local>>,
    context_session_id: Option<String>,
    calendar_event: Option<CalendarEvent>,
    template_slug: Option<String>,
    consent: Option<crate::markdown::ConsentBasis>,
    consent_notice: Option<String>,
    recording_health: Option<crate::markdown::RecordingHealth>,
) -> std::io::Result<ProcessingJob> {
    let job_id = next_job_id();
    let old_screen_anchor = native_stem_group_for_survivor(current_wav)
        .map(|group| group.anchor)
        .unwrap_or_else(|| current_wav.to_path_buf());
    let old_screen_dir = crate::screen::screens_dir_for(&old_screen_anchor);
    let audio_path = move_capture_into_job(&job_id, current_wav)?;
    let new_screen_anchor = native_stem_group_for_survivor(&audio_path)
        .map(|group| group.anchor)
        .unwrap_or_else(|| audio_path.clone());
    let new_screen_dir = crate::screen::screens_dir_for(&new_screen_anchor);
    let job = ProcessingJob {
        id: job_id,
        mode,
        content_type: mode.content_type(),
        title,
        audio_path: audio_path.display().to_string(),
        output_path: None,
        state: JobState::Queued,
        stage: JobState::Queued.default_stage(),
        created_at: Local::now(),
        started_at: None,
        finished_at: None,
        notice_dismissed_at: None,
        recording_started_at,
        recording_finished_at,
        context_session_id,
        user_notes,
        pre_context,
        consent,
        consent_notice,
        calendar_event,
        template_slug,
        recording_health,
        word_count: None,
        error: None,
        owner_pid: None,
        retry_count: 0,
    };
    if let Err(error) = write_job(&job) {
        if audio_path.exists() {
            move_capture_artifacts(&audio_path, current_wav).ok();
        }
        if new_screen_dir.exists() {
            if old_screen_dir.exists() {
                fs::remove_dir_all(&old_screen_dir).ok();
            }
            fs::rename(&new_screen_dir, &old_screen_dir).ok();
        }
        return Err(error);
    }
    if new_screen_dir.exists() {
        if let Some(session_id) = job.context_session_id.as_deref() {
            if let Err(error) =
                crate::context_store::relink_screen_context_directory(session_id, &new_screen_dir)
            {
                tracing::warn!(
                    session_id,
                    directory = %new_screen_dir.display(),
                    error = %error,
                    "failed to relink moved screen context directory"
                );
            }
        }
    }
    maybe_mark_context_session_processing(&job, &audio_path);
    Ok(job)
}

pub fn jobs_dir() -> PathBuf {
    Config::minutes_dir().join("jobs")
}

/// Subdirectory inside `jobs/` that holds finalized (Complete/Failed/NeedsReview)
/// job records. Terminal jobs are moved here on state transition so the
/// hot-path `list_jobs_raw` scan never has to read or parse them — one
/// directory walk + N small JSON parses per UI status poll otherwise scales
/// linearly with the user's lifetime meeting count.
///
/// The boundary is enforced by `update_job_state` (write-then-`hard_link`-
/// then-unlink for to-terminal, write-new-then-remove-old for from-terminal;
/// see `move_to_archive` for why we use `hard_link` instead of `rename`)
/// and by a one-shot lazy migration of pre-existing terminal jobs (see
/// `migrate_terminal_jobs_to_archive`).
pub fn archive_dir() -> PathBuf {
    jobs_dir().join("archive")
}

pub fn worker_pid_path() -> PathBuf {
    Config::minutes_dir().join("processing-worker.pid")
}

pub fn job_path(job_id: &str) -> PathBuf {
    jobs_dir().join(format!("{}.json", job_id))
}

/// Path of a job's archived JSON (after terminal state transition).
pub fn job_archive_path(job_id: &str) -> PathBuf {
    archive_dir().join(format!("{}.json", job_id))
}

pub fn job_capture_path(job_id: &str) -> PathBuf {
    jobs_dir().join(format!("{}.wav", job_id))
}

fn job_capture_path_for_source(job_id: &str, source: &Path) -> PathBuf {
    if let Some(role) = native_stem_role(source) {
        return jobs_dir().join(format!("{}.{}.wav", job_id, role));
    }
    let ext = source
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("wav");
    jobs_dir().join(format!("{}.{}", job_id, ext))
}

#[derive(Debug, Clone)]
struct NativeStemGroup {
    anchor: PathBuf,
    voice: PathBuf,
    system: PathBuf,
}

fn native_stem_role(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".voice.wav") {
        Some("voice")
    } else if name.ends_with(".system.wav") {
        Some("system")
    } else {
        None
    }
}

fn native_stem_group_for_survivor(path: &Path) -> Option<NativeStemGroup> {
    let role = native_stem_role(path)?;
    let name = path.file_name()?.to_str()?;
    let suffix = format!(".{role}.wav");
    let base = name.strip_suffix(&suffix)?;
    let dir = path.parent()?;
    Some(NativeStemGroup {
        anchor: dir.join(format!("{base}.mov")),
        voice: dir.join(format!("{base}.voice.wav")),
        system: dir.join(format!("{base}.system.wav")),
    })
}

fn move_native_stem_group(src_primary: &Path, dest_primary: &Path) -> std::io::Result<()> {
    if !src_primary.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("native-call survivor is missing: {}", src_primary.display()),
        ));
    }
    let src = native_stem_group_for_survivor(src_primary).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("not a native-call stem path: {}", src_primary.display()),
        )
    })?;
    let dest = native_stem_group_for_survivor(dest_primary).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("not a native-call destination: {}", dest_primary.display()),
        )
    })?;
    let pairs = [
        (src.anchor, dest.anchor),
        (src.voice, dest.voice),
        (src.system, dest.system),
    ];
    let mut moved = Vec::new();
    for (source, destination) in pairs {
        if !source.exists() {
            continue;
        }
        if let Err(error) = fs::rename(&source, &destination) {
            for (rollback_source, rollback_destination) in moved.into_iter().rev() {
                fs::rename(rollback_destination, rollback_source).ok();
            }
            return Err(error);
        }
        moved.push((source, destination));
    }
    Ok(())
}

fn move_capture_artifacts(src_audio: &Path, dest_audio: &Path) -> std::io::Result<()> {
    if native_stem_role(src_audio).is_some() {
        return move_native_stem_group(src_audio, dest_audio);
    }

    fs::rename(src_audio, dest_audio)?;
    if let Err(error) = move_stems_with_audio(src_audio, dest_audio) {
        fs::rename(dest_audio, src_audio).ok();
        move_stems_with_audio(dest_audio, src_audio).ok();
        return Err(error);
    }
    Ok(())
}

pub fn create_worker_guard() -> Result<PidGuard, crate::error::PidError> {
    pid::create_pid_guard(&worker_pid_path())
}

/// The worker PID when readable. NOTE: this cannot detect a `LockedAlive` worker
/// on Windows (the lock makes the PID unreadable). For a presence check, prefer
/// [`worker_active`], which is correct on every platform.
pub fn current_worker_pid() -> Option<u32> {
    pid::check_pid_file(&worker_pid_path()).ok().flatten()
}

/// Whether a background worker is currently running. Uses `inspect_pid_file` so a
/// worker holding its PID file under a mandatory Windows lock is still detected —
/// `current_worker_pid` would read the locked file as absent and let a duplicate
/// worker spawn. See #258 and `pid::PidFileState`.
pub fn worker_active() -> bool {
    pid::inspect_pid_file(&worker_pid_path()).is_active()
}

pub fn move_capture_into_job(job_id: &str, current_wav: &Path) -> std::io::Result<PathBuf> {
    move_capture_into_job_with_screen_mover(job_id, current_wav, |source, destination| {
        fs::rename(source, destination)
    })
}

fn move_capture_into_job_with_screen_mover(
    job_id: &str,
    current_wav: &Path,
    mut move_screen_dir: impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<PathBuf> {
    let dest = job_capture_path_for_source(job_id, current_wav);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    move_capture_artifacts(current_wav, &dest)?;

    let screen_source = native_stem_group_for_survivor(current_wav)
        .map(|group| group.anchor)
        .unwrap_or_else(|| current_wav.to_path_buf());
    let screen_dest = native_stem_group_for_survivor(&dest)
        .map(|group| group.anchor)
        .unwrap_or_else(|| dest.clone());
    let old_screen_dir = crate::screen::screens_dir_for(&screen_source);
    if old_screen_dir.exists() {
        let new_screen_dir = crate::screen::screens_dir_for(&screen_dest);
        let screen_move_result = (|| -> std::io::Result<()> {
            if let Some(parent) = new_screen_dir.parent() {
                fs::create_dir_all(parent)?;
            }
            if new_screen_dir.exists() {
                fs::remove_dir_all(&new_screen_dir)?;
            }
            move_screen_dir(&old_screen_dir, &new_screen_dir)
        })();
        if let Err(error) = screen_move_result {
            if let Err(rollback_error) = move_capture_artifacts(&dest, current_wav) {
                tracing::error!(
                    source = %dest.display(),
                    destination = %current_wav.display(),
                    error = %rollback_error,
                    "failed to roll capture artifacts back after screen-directory move failure"
                );
            }
            return Err(error);
        }
    }

    Ok(dest)
}

fn move_stems_with_audio(src_audio: &Path, dest_audio: &Path) -> std::io::Result<()> {
    let Some(src_stems) = crate::capture::stem_paths_for(src_audio) else {
        return Ok(());
    };
    let Some(dest_stems) = crate::capture::stem_paths_for(dest_audio) else {
        return Ok(());
    };

    if src_stems.voice.exists() {
        fs::rename(&src_stems.voice, &dest_stems.voice)?;
    }
    if src_stems.system.exists() {
        fs::rename(&src_stems.system, &dest_stems.system)?;
    }

    Ok(())
}

fn preserve_sidecar_stems(audio_src: &Path, audio_dest: &Path) {
    let Some(src_stems) = crate::capture::stem_paths_for(audio_src) else {
        return;
    };
    let Some(dest_stems) = crate::capture::stem_paths_for(audio_dest) else {
        return;
    };

    for (src, dest) in [
        (src_stems.voice, dest_stems.voice),
        (src_stems.system, dest_stems.system),
    ] {
        if !src.exists() {
            continue;
        }
        if let Err(rename_error) = fs::rename(&src, &dest) {
            if let Err(copy_error) = fs::copy(&src, &dest) {
                tracing::warn!(
                    src = %src.display(),
                    dest = %dest.display(),
                    rename_error = %rename_error,
                    copy_error = %copy_error,
                    "failed to preserve capture stem alongside output"
                );
                continue;
            }
            fs::remove_file(&src).ok();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).ok();
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn enqueue_capture_job(
    mode: CaptureMode,
    title: Option<String>,
    audio_path: PathBuf,
    user_notes: Option<String>,
    pre_context: Option<String>,
    recording_started_at: Option<DateTime<Local>>,
    recording_finished_at: Option<DateTime<Local>>,
    context_session_id: Option<String>,
    calendar_event: Option<CalendarEvent>,
    template_slug: Option<String>,
) -> std::io::Result<ProcessingJob> {
    let job = ProcessingJob {
        id: next_job_id(),
        mode,
        content_type: mode.content_type(),
        title,
        audio_path: audio_path.display().to_string(),
        output_path: None,
        state: JobState::Queued,
        stage: JobState::Queued.default_stage(),
        created_at: Local::now(),
        started_at: None,
        finished_at: None,
        notice_dismissed_at: None,
        recording_started_at,
        recording_finished_at,
        context_session_id,
        user_notes,
        pre_context,
        consent: None,
        consent_notice: None,
        calendar_event,
        template_slug,
        recording_health: None,
        word_count: None,
        error: None,
        owner_pid: None,
        retry_count: 0,
    };
    write_job(&job)?;
    maybe_mark_context_session_processing(&job, Path::new(&job.audio_path));
    Ok(job)
}

fn maybe_mark_context_session_processing(job: &ProcessingJob, audio_path: &Path) {
    let Some(session_id) = job.context_session_id.as_deref() else {
        return;
    };

    if let Err(error) = crate::context_store::mark_capture_session_processing(
        session_id,
        &job.id,
        audio_path,
        job.recording_finished_at,
    ) {
        tracing::warn!(
            session_id,
            job_id = %job.id,
            error = %error,
            "failed to mark context session as processing"
        );
    }
}

fn maybe_mark_context_session_complete(job: &ProcessingJob, content_type: ContentType) {
    let Some(session_id) = job.context_session_id.as_deref() else {
        return;
    };
    let Some(output_path) = job.output_path.as_deref() else {
        return;
    };

    let metadata = json!({
        "job_id": job.id,
        "job_state": match job.state {
            JobState::NeedsReview => "needs-review",
            JobState::Complete => "complete",
            JobState::Failed => "failed",
            JobState::Queued => "queued",
            JobState::Transcribing => "transcribing",
            JobState::TranscriptOnly => "transcript-only",
            JobState::Diarizing => "diarizing",
            JobState::Summarizing => "summarizing",
            JobState::Saving => "saving",
        },
    });

    if let Err(error) = crate::context_store::mark_capture_session_complete(
        session_id,
        Path::new(output_path),
        Some(Path::new(&job.audio_path)),
        content_type,
        job.finished_at,
        metadata,
    ) {
        tracing::warn!(
            session_id,
            job_id = %job.id,
            error = %error,
            "failed to finalize context session"
        );
    }
}

fn maybe_mark_context_session_failed(
    job: &ProcessingJob,
    diagnostic: &str,
    preserved_path: Option<&Path>,
) {
    let Some(session_id) = job.context_session_id.as_deref() else {
        return;
    };

    if let Err(error) = crate::context_store::mark_capture_session_failed(
        session_id,
        job.finished_at.or(job.recording_finished_at),
        diagnostic,
        preserved_path,
    ) {
        tracing::warn!(
            session_id,
            job_id = %job.id,
            error = %error,
            "failed to mark context session as failed"
        );
    }
}

pub fn write_job(job: &ProcessingJob) -> std::io::Result<()> {
    write_job_to(job, &job_path(&job.id))
}

/// Atomically write a job's JSON to the given destination path (temp file +
/// rename). Used by `write_job` (always active path) and by `update_job_state`
/// when a state transition crosses the active/archive boundary.
fn write_job_to(job: &ProcessingJob, dest: &Path) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(job)?;
    fs::write(&tmp, json)?;
    fs::rename(tmp, dest)?;
    Ok(())
}

pub fn load_job(job_id: &str) -> Option<ProcessingJob> {
    load_job_with_source(job_id).map(|(job, _)| job)
}

/// Load a job and return the path it was loaded from. Active dir is checked
/// first so the common (in-flight) case stays single-stat; the archive is the
/// fallback.
///
/// The returned path is what `update_job_state` uses to decide whether a
/// state transition crosses the active/archive boundary — the file's actual
/// current location is the source of truth, not what its `state` field
/// implies (which can disagree mid-update).
fn load_job_with_source(job_id: &str) -> Option<(ProcessingJob, PathBuf)> {
    let active = job_path(job_id);
    if let Ok(text) = fs::read_to_string(&active) {
        if let Ok(job) = serde_json::from_str::<ProcessingJob>(&text) {
            return Some((job, active));
        }
    }
    let archive = job_archive_path(job_id);
    if let Ok(text) = fs::read_to_string(&archive) {
        if let Ok(job) = serde_json::from_str::<ProcessingJob>(&text) {
            return Some((job, archive));
        }
    }
    None
}

fn list_jobs_raw() -> Vec<ProcessingJob> {
    ensure_archive_initialized();
    let mut jobs = Vec::new();
    let dir = jobs_dir();
    if !dir.exists() {
        return jobs;
    }

    // Only top-level `.json` files. The `archive/` subdir entry has no
    // `.json` extension and is skipped by the filter below; this is the
    // critical guarantee that terminal jobs stay off the hot path.
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(job) = serde_json::from_str::<ProcessingJob>(&text) {
                jobs.push(job);
            }
        }
    }

    jobs.sort_by_key(|job| job.created_at);
    jobs
}

/// Return the archived job with the most recent `finished_at` timestamp,
/// falling back to `created_at` for jobs that predate finished_at being
/// populated. Designed for the "show notification about what just
/// completed" path (`spawn_processing_worker`'s post-exit handler in the
/// Tauri commands layer).
///
/// Reads only `archive_dir()`, never the active dir. The status-poll hot
/// path lives in `list_jobs_raw` (active only); this helper lives in the
/// rare-event post-worker path where scanning the archive is acceptable.
/// Using `finished_at` rather than `created_at` matters for reprocessed
/// jobs: a long-running re-process can finish a job whose `created_at` is
/// hours older than a freshly-queued recording, and the user wants the
/// notification to reflect what actually just transitioned, not whichever
/// terminal record was queued most recently.
pub fn latest_terminal_job() -> Option<ProcessingJob> {
    let mut latest: Option<ProcessingJob> = None;
    for job in list_archive_jobs() {
        // Defensive: archive should only ever hold terminal-state jobs,
        // but a future bug or an external tool could plant a non-terminal
        // record here. Filter explicitly so the caller never observes
        // wrong-state output.
        if !job.state.is_terminal() {
            continue;
        }
        let candidate_key = job.finished_at.unwrap_or(job.created_at);
        match latest.as_ref() {
            None => latest = Some(job),
            Some(current) => {
                let current_key = current.finished_at.unwrap_or(current.created_at);
                if candidate_key > current_key {
                    latest = Some(job);
                }
            }
        }
    }
    latest
}

/// List archived (terminal-state) jobs from `archive_dir`. Used by
/// `display_jobs(_, include_terminal=true)` for full UI listings and by
/// `latest_terminal_job`; never called from the 1Hz status poll path.
fn list_archive_jobs() -> Vec<ProcessingJob> {
    // Defensive — `display_jobs` already triggers migration via list_jobs(),
    // but a future caller hitting this directly would otherwise miss the
    // upgrade sweep on a pre-migration install.
    ensure_archive_initialized();
    let dir = archive_dir();
    if !dir.exists() {
        return Vec::new();
    }
    let mut jobs = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(job) = serde_json::from_str::<ProcessingJob>(&text) {
                jobs.push(job);
            }
        }
    }
    jobs
}

/// Run the one-shot upgrade migration unless the sentinel marker says it
/// already happened in this `~/.minutes/` installation. The marker survives
/// process restarts; tests get fresh state because each `with_temp_home`
/// gives them an isolated `~/.minutes/`.
fn ensure_archive_initialized() {
    let root = jobs_dir();
    if let Err(error) = fs::create_dir_all(&root) {
        tracing::warn!(error = %error, "failed to initialize jobs directory");
        return;
    }
    let marker = root.join(MIGRATION_MARKER);
    if marker.exists() {
        return;
    }
    if let Err(error) = migrate_terminal_jobs_to_archive() {
        tracing::warn!(error = %error, "jobs archive migration failed");
        return;
    }
    if let Err(error) = fs::write(&marker, b"v1") {
        tracing::warn!(
            error = %error,
            "failed to write archive migration marker; sweep will retry next call"
        );
    }
}

/// Sweep pre-existing terminal jobs from `jobs/` into `jobs/archive/`.
///
/// Idempotent and race-tolerant. The critical correctness property: when two
/// processes race the same job, neither can clobber the canonical archive
/// copy with stale content. Achieved via `fs::hard_link` — POSIX `rename(2)`
/// silently overwrites the destination, so `fs::rename` is unsafe here even
/// though it would otherwise be the natural primitive.
///
/// `fs::hard_link` reserves the dest path atomically: it succeeds only when
/// dest does not exist, returning `AlreadyExists` otherwise. After a
/// successful link, source and dest point to the same inode, so removing
/// source completes the move. If the link fails because dest already
/// exists, we trust the existing archive copy (placed there by another
/// process or a prior migration) and drop the active duplicate.
fn migrate_terminal_jobs_to_archive() -> std::io::Result<()> {
    let active = jobs_dir();
    if !active.exists() {
        return Ok(());
    }
    fs::create_dir_all(archive_dir())?;

    for entry in fs::read_dir(&active).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        // Mirror the prior `list_jobs_raw` behavior: read_to_string fails
        // for directories or unreadable entries, so no separate is_file()
        // check (which would silently skip valid `.json` symlinks).
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(job) = serde_json::from_str::<ProcessingJob>(&text) else {
            continue;
        };
        if !job.state.is_terminal() {
            continue;
        }
        let dest = job_archive_path(&job.id);
        if let Err(error) = move_to_archive(&path, &dest) {
            tracing::warn!(
                job_id = %job.id,
                error = %error,
                "migrate: failed to move job to archive"
            );
        }
    }
    Ok(())
}

/// Move a job JSON file from active to archive with `AlreadyExists`
/// detection. Used by both the upgrade migration and `update_job_state`'s
/// to-archive transition. The hard-link-then-unlink dance prevents the
/// `fs::rename` silent-overwrite hazard described above.
fn move_to_archive(source: &Path, dest: &Path) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::hard_link(source, dest) {
        Ok(()) => {
            // Both names point to the same inode — drop the active name.
            if let Err(error) = fs::remove_file(source) {
                if error.kind() != ErrorKind::NotFound {
                    tracing::warn!(
                        source = %source.display(),
                        error = %error,
                        "move_to_archive: failed to remove source after link"
                    );
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            // A concurrent process or earlier run already archived this
            // job. Trust the existing archive copy and drop our active
            // duplicate — losing our potentially-stale update is the
            // correct behavior (the on-disk archive is the canonical
            // version; ours may have been built from an older snapshot).
            tracing::debug!(
                source = %source.display(),
                dest = %dest.display(),
                "archive already populated; dropping active duplicate"
            );
            if let Err(remove_error) = fs::remove_file(source) {
                if remove_error.kind() != ErrorKind::NotFound {
                    tracing::warn!(
                        source = %source.display(),
                        error = %remove_error,
                        "move_to_archive: failed to drop duplicate"
                    );
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            // Source disappeared mid-flight (concurrent migration moved
            // it). If dest exists, treat as success; otherwise propagate.
            if dest.exists() {
                Ok(())
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

pub fn list_jobs() -> Vec<ProcessingJob> {
    // Collect just the IDs that look stale in this snapshot. Recovery
    // itself is delegated to `update_job_state`, which reloads the latest
    // disk state inside its read-modify-write — so a worker that claimed
    // the job between this scan and the recovery call is not silently
    // clobbered (codex review of issue #229: two races where a stale
    // in-memory copy could overwrite a concurrent claim or an
    // intervening update).
    let snapshot = list_jobs_raw();
    let recovery_candidates: Vec<String> = snapshot
        .iter()
        .filter(|job| {
            !job.state.is_terminal()
                && job.owner_pid.is_some()
                && !job.owner_pid.map(pid::is_process_alive).unwrap_or(false)
        })
        .map(|job| job.id.clone())
        .collect();

    for id in &recovery_candidates {
        let _ = update_job_state(id, |current| {
            // Re-check on the fresh disk record: a concurrent worker may
            // have claimed it after our snapshot, or another `list_jobs`
            // caller may have already recovered it. Both are no-ops here.
            if current.state.is_terminal()
                || current.owner_pid.is_none()
                || current
                    .owner_pid
                    .map(pid::is_process_alive)
                    .unwrap_or(false)
            {
                return;
            }
            current.retry_count = current.retry_count.saturating_add(1);
            current.owner_pid = None;
            current.started_at = None;
            if current.retry_count > MAX_AUTO_RETRIES {
                current.state = JobState::Failed;
                current.stage = JobState::Failed.default_stage();
                current.finished_at = Some(Local::now());
                if current.error.is_none() {
                    current.error = Some(format!(
                        "transcription worker crashed {} times without producing output; left as Failed for manual retry",
                        current.retry_count
                    ));
                }
            } else {
                current.state = JobState::Queued;
                current.stage = JobState::Queued.default_stage();
            }
        });
    }

    // If we recovered anything, re-read so callers see the post-recovery
    // state. Most polls have an empty candidate list so this branch is
    // skipped and we return the original cheap snapshot.
    if recovery_candidates.is_empty() {
        snapshot
    } else {
        list_jobs_raw()
    }
}

pub fn display_jobs(limit: Option<usize>, include_terminal: bool) -> Vec<ProcessingJob> {
    let mut jobs = list_jobs();
    if include_terminal {
        // The hot path (`list_jobs_raw`) skips the archive subdir entirely.
        // Full UI listings still want terminal history, so pull it in here.
        jobs.extend(list_archive_jobs());
    }
    jobs.sort_by(|a, b| {
        job_sort_bucket(a)
            .cmp(&job_sort_bucket(b))
            .then_with(|| b.created_at.cmp(&a.created_at))
    });

    if !include_terminal {
        jobs.retain(|job| !job.state.is_terminal());
    }

    if let Some(limit) = limit {
        jobs.truncate(limit);
    }

    jobs
}

pub fn active_jobs() -> Vec<ProcessingJob> {
    display_jobs(None, false)
}

pub fn active_job_count() -> usize {
    active_jobs().len()
}

pub fn requeue_job(job_id: &str) -> std::io::Result<Option<ProcessingJob>> {
    let Some(job) = load_job(job_id) else {
        return Ok(None);
    };

    if !matches!(job.state, JobState::Failed | JobState::NeedsReview) {
        return Ok(None);
    }

    let audio_path = PathBuf::from(&job.audio_path);
    if !audio_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("audio file missing for job {}", job_id),
        ));
    }

    let Some(requeued) = update_job_state(job_id, |job| {
        if !matches!(job.state, JobState::Failed | JobState::NeedsReview) {
            return;
        }
        job.state = JobState::Queued;
        job.stage = JobState::Queued.default_stage();
        job.started_at = None;
        job.finished_at = None;
        job.notice_dismissed_at = None;
        job.error = None;
        job.owner_pid = None;
        // Manual requeue is the user explicitly retrying after looking at
        // the failure; give them a fresh auto-retry budget so a future
        // transient crash doesn't get masked by leftover counter state.
        job.retry_count = 0;
    })?
    else {
        return Ok(None);
    };

    if requeued.state != JobState::Queued {
        return Ok(None);
    }

    sync_processing_status();
    Ok(Some(requeued))
}

pub fn dismiss_job_notice(job_id: &str) -> std::io::Result<Option<ProcessingJob>> {
    update_job_state(job_id, |job| {
        if matches!(job.state, JobState::Failed | JobState::NeedsReview) {
            job.notice_dismissed_at = Some(Local::now());
        }
    })
}

pub fn processing_summary() -> Option<ProcessingJob> {
    active_jobs().into_iter().next()
}

fn job_sort_bucket(job: &ProcessingJob) -> u8 {
    if job.state.is_terminal() {
        2
    } else if job.state == JobState::Queued {
        1
    } else {
        0
    }
}

pub fn next_pending_job() -> Option<ProcessingJob> {
    list_jobs()
        .into_iter()
        .find(|job| job.state == JobState::Queued)
}

/// Atomically update a job's state, moving the file across the
/// active/archive boundary if (and only if) the state transition crosses it.
///
/// Boundary logic, per the codex review of fix B:
/// * **Same dir** (active→active or archive→archive): atomic write to the
///   destination via `write_job_to`.
/// * **Active → archive** (job became terminal): write the updated JSON to
///   the active path first, then call `move_to_archive` (which uses
///   `hard_link` + `remove_file` rather than `rename` so we never silently
///   clobber an existing archive copy on a race). If the move fails, the
///   job stays in active with its terminal state — `display_jobs(_, false)`
///   filters terminal jobs out so this is invisible to the user; the file
///   is in the wrong place but not duplicated and gets re-tried next update.
/// * **Archive → active** (requeue): write the new (non-terminal) JSON
///   directly to the active path, then remove the archive copy. If the
///   write fails, the archive copy preserves the original terminal state
///   and the caller surfaces the error. If the remove fails, both copies
///   exist briefly — `load_job_with_source` checks active first so the
///   non-terminal record wins lookups; the leftover archive copy is
///   reaped on the next update touching this job.
pub fn update_job_state<F>(job_id: &str, update: F) -> std::io::Result<Option<ProcessingJob>>
where
    F: FnOnce(&mut ProcessingJob),
{
    let Some((mut job, source)) = load_job_with_source(job_id) else {
        return Ok(None);
    };
    update(&mut job);

    let dest = if job.state.is_terminal() {
        job_archive_path(&job.id)
    } else {
        job_path(&job.id)
    };

    if source == dest {
        write_job_to(&job, &dest)?;
        return Ok(Some(job));
    }

    let source_in_active = source == job_path(&job.id);

    if !source_in_active {
        // Archive → active (requeue). Write new state to active first;
        // archive copy preserves the original until removal succeeds.
        write_job_to(&job, &dest)?;
        if let Err(e) = fs::remove_file(&source) {
            if e.kind() != ErrorKind::NotFound {
                tracing::warn!(
                    job_id = %job.id,
                    error = %e,
                    "update_job_state: failed to remove archive copy after requeue"
                );
            }
        }
        return Ok(Some(job));
    }

    // Active → archive (job became terminal). Write updated JSON to the
    // active path first so the on-disk record matches what we hand back;
    // then move it into archive via `move_to_archive` (hard_link + unlink,
    // which surfaces AlreadyExists rather than silently overwriting). If
    // the move fails for an unexpected reason, the terminal-state record
    // sits in the active dir — `display_jobs(_, false)` filters terminal
    // jobs out so this is invisible to the user; self-healing on the next
    // update.
    write_job_to(&job, &source)?;
    if let Err(error) = move_to_archive(&source, &dest) {
        tracing::warn!(
            job_id = %job.id,
            error = %error,
            "update_job_state: move_to_archive failed (job stays in active dir but is terminal-state)"
        );
    }
    Ok(Some(job))
}

pub fn remove_capture_artifacts(job: &ProcessingJob) {
    let audio_path = PathBuf::from(&job.audio_path);
    let screen_anchor = native_stem_group_for_survivor(&audio_path)
        .map(|group| group.anchor)
        .unwrap_or_else(|| audio_path.clone());
    if let Some(group) = native_stem_group_for_survivor(&audio_path) {
        for path in [group.anchor, group.voice, group.system] {
            if path.exists() {
                fs::remove_file(path).ok();
            }
        }
    } else if audio_path.exists() {
        fs::remove_file(&audio_path).ok();
    }
    let screens_dir = crate::screen::screens_dir_for(&screen_anchor);
    if screens_dir.exists() {
        fs::remove_dir_all(screens_dir).ok();
    }
    if let Some(session_id) = job.context_session_id.as_deref() {
        if let Ok(status) = crate::context_store::cleanup_screen_context(session_id) {
            crate::screen::write_current_session_status(&status).ok();
        }
    }
}

fn terminal_state_for_artifact(artifact: &pipeline::TranscriptArtifact) -> JobState {
    if artifact.frontmatter.status == Some(OutputStatus::NoSpeech) {
        JobState::NeedsReview
    } else {
        JobState::Complete
    }
}

fn apply_transcript_artifact_metadata(
    job: &mut ProcessingJob,
    artifact: &pipeline::TranscriptArtifact,
) {
    job.output_path = Some(artifact.write_result.path.display().to_string());
    job.title = Some(artifact.write_result.title.clone());
    job.word_count = Some(artifact.write_result.word_count);
    // Core-only `.mov` recovery discovers degradation after the job was
    // queued, so the artifact is authoritative. Persist it back to the job;
    // the desktop completion notice reads this field rather than reparsing
    // Markdown (#463).
    job.recording_health = artifact.frontmatter.recording_health.clone();
}

/// Move the captured audio alongside the output markdown so users can reprocess later.
/// e.g. ~/meetings/2026-04-02-standup.md → ~/meetings/2026-04-02-standup.wav
/// or, for native call captures, ~/meetings/2026-04-02-call.mov.
fn preserve_audio_alongside_output(job: &ProcessingJob, config: &Config) {
    let Some(ref output_path) = job.output_path else {
        return;
    };
    let output = PathBuf::from(output_path);
    let audio_src = PathBuf::from(&job.audio_path);
    if !audio_src.exists() {
        return;
    }
    if let Some(role) = native_stem_role(&audio_src) {
        let audio_dest = output.with_extension(format!("{role}.wav"));
        let Some(src_group) = native_stem_group_for_survivor(&audio_src) else {
            return;
        };
        let Some(dest_group) = native_stem_group_for_survivor(&audio_dest) else {
            return;
        };
        let source_screens_dir = crate::screen::screens_dir_for(&src_group.anchor);
        let preserved_anchor = dest_group.anchor.clone();
        for (src, dest) in [
            (src_group.anchor, dest_group.anchor),
            (src_group.voice, dest_group.voice),
            (src_group.system, dest_group.system),
        ] {
            if !src.exists() {
                continue;
            }
            if let Err(rename_error) = fs::rename(&src, &dest) {
                if let Err(copy_error) = fs::copy(&src, &dest) {
                    tracing::warn!(
                        src = %src.display(),
                        dest = %dest.display(),
                        error = %copy_error,
                        "failed to preserve native-call artifact (rename: {rename_error})"
                    );
                    continue;
                }
                fs::remove_file(&src).ok();
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dest, fs::Permissions::from_mode(0o600)).ok();
            }
        }
        if !config.screen_context.keep_after_summary && source_screens_dir.exists() {
            fs::remove_dir_all(source_screens_dir).ok();
        }
        update_job_state(&job.id, |j| {
            j.audio_path = audio_dest.display().to_string();
        })
        .ok();
        tracing::info!(
            path = %preserved_anchor.display(),
            survivor = %audio_dest.display(),
            "preserved native-call capture group alongside transcript"
        );
        return;
    }
    let audio_dest = match audio_src.extension().filter(|ext| !ext.is_empty()) {
        Some(ext) => output.with_extension(ext),
        None => output.with_extension("wav"),
    };
    if let Err(e) = fs::rename(&audio_src, &audio_dest) {
        // rename fails across filesystems; fall back to copy + delete
        if let Err(e2) = fs::copy(&audio_src, &audio_dest) {
            tracing::warn!(
                src = %audio_src.display(),
                dest = %audio_dest.display(),
                error = %e2,
                "failed to preserve audio alongside output (rename: {}, copy: {})", e, e2
            );
            return;
        }
        fs::remove_file(&audio_src).ok();
    }
    // Preserve same permissions as the markdown (0600)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&audio_dest, fs::Permissions::from_mode(0o600)).ok();
    }
    // Clean up any screen capture artifacts left in the jobs dir, unless the
    // user opted to keep screenshots after summarization. Without this guard we
    // would delete screenshots the pipeline deliberately preserved.
    if !config.screen_context.keep_after_summary {
        let screens_dir = crate::screen::screens_dir_for(&audio_src);
        if screens_dir.exists() {
            fs::remove_dir_all(screens_dir).ok();
        }
    }
    // Update the job record so audio_path points to the new location
    let dest_str = audio_dest.display().to_string();
    update_job_state(&job.id, |j| {
        j.audio_path = dest_str;
    })
    .ok();
    preserve_sidecar_stems(&audio_src, &audio_dest);
    tracing::info!(
        path = %audio_dest.display(),
        "preserved audio alongside transcript"
    );
}

fn sync_processing_status() {
    if let Some(job) = processing_summary() {
        let title = job.title.as_deref().or(job.output_path.as_deref());
        let _ = pid::set_processing_status(
            job.stage.as_deref(),
            Some(job.mode),
            title,
            Some(&job.id),
            active_job_count(),
        );
    } else {
        let _ = pid::clear_processing_status();
    }
}

fn recording_duration(job: &ProcessingJob) -> String {
    match (job.recording_started_at, job.recording_finished_at) {
        (Some(start), Some(end)) => {
            let secs = end.signed_duration_since(start).num_seconds().max(0);
            let mins = secs / 60;
            let rem = secs % 60;
            if mins > 0 {
                format!("{}m {}s", mins, rem)
            } else {
                format!("{}s", rem)
            }
        }
        _ => "unknown".into(),
    }
}

fn refresh_qmd_collection(config: &Config) {
    let Some(collection) = config.search.qmd_collection.as_ref() else {
        return;
    };
    let status = crate::engine_process::command("qmd")
        .args(["update", "-c", collection])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if let Err(error) = status {
        tracing::debug!(error = %error, collection = %collection, "qmd update skipped");
    }
}

fn job_context(job: &ProcessingJob) -> BackgroundPipelineContext {
    let template = job.template_slug.as_deref().and_then(|slug| {
        match crate::template::TemplateResolver::new().resolve(slug) {
            Ok(t) => Some(t),
            Err(error) => {
                tracing::warn!(slug = %slug, error = %error, "queued template could not be resolved; processing without template");
                None
            }
        }
    });
    BackgroundPipelineContext {
        sidecar: None,
        user_notes: job.user_notes.clone(),
        pre_context: job.pre_context.clone(),
        consent: job.consent,
        consent_notice: job.consent_notice.clone(),
        calendar_event: job.calendar_event.clone(),
        recorded_at: job.recording_started_at.or(job.recording_finished_at),
        requested_title: job.title.clone(),
        recording_health: job.recording_health.clone(),
        context_session_id: job.context_session_id.clone(),
        template,
    }
}

fn stage_state(stage: PipelineStage) -> JobState {
    match stage {
        PipelineStage::Transcribing => JobState::Transcribing,
        PipelineStage::Diarizing => JobState::Diarizing,
        PipelineStage::Summarizing => JobState::Summarizing,
        PipelineStage::Saving => JobState::Saving,
    }
}

pub fn process_pending_jobs<F>(config: &Config, mut on_job_update: F) -> Result<(), MinutesError>
where
    F: FnMut(&ProcessingJob),
{
    let mut transcribe = |audio_path: &Path,
                          content_type: ContentType,
                          title: Option<&str>,
                          config: &Config,
                          context: &BackgroundPipelineContext,
                          existing_output_path: Option<&Path>| {
        pipeline::transcribe_to_artifact(
            audio_path,
            content_type,
            title,
            config,
            context,
            existing_output_path,
        )
    };
    process_pending_jobs_with_transcriber(config, &mut on_job_update, &mut transcribe)
}

type JobTranscriber<'a> = dyn FnMut(
        &Path,
        ContentType,
        Option<&str>,
        &Config,
        &BackgroundPipelineContext,
        Option<&Path>,
    ) -> Result<pipeline::TranscriptArtifact, MinutesError>
    + 'a;

fn process_pending_jobs_with_transcriber(
    config: &Config,
    on_job_update: &mut dyn FnMut(&ProcessingJob),
    transcribe: &mut JobTranscriber<'_>,
) -> Result<(), MinutesError> {
    let _guard = create_worker_guard()?;

    while let Some(job) = next_pending_job() {
        let owner_pid = std::process::id();
        let Some(mut job) = update_job_state(&job.id, |job| {
            job.state = JobState::Transcribing;
            job.stage = JobState::Transcribing.default_stage();
            job.owner_pid = Some(owner_pid);
            job.started_at.get_or_insert_with(Local::now);
            job.error = None;
        })?
        else {
            continue;
        };
        sync_processing_status();
        on_job_update(&job);

        let audio_path = PathBuf::from(&job.audio_path);
        let context = job_context(&job);

        // Run transcription inside `catch_unwind` so a Rust panic in any of
        // the heavy native paths (whisper.cpp FFI, parakeet helper, audio
        // decode) surfaces as a normal `Err` here instead of unwinding
        // through the worker. The macOS `_exit` path in
        // `tauri/src-tauri/src/main.rs` separately prevents the
        // C++-static-teardown abort during process exit; the two together
        // are what stop SIGABRT from reaching the UI (issue #229).
        let transcribe_outcome = catch_unwind(AssertUnwindSafe(|| {
            transcribe(
                &audio_path,
                job.content_type,
                job.title.as_deref(),
                config,
                &context,
                job.output_path.as_deref().map(Path::new),
            )
        }));
        let artifact = match transcribe_outcome {
            Ok(Ok(artifact)) => artifact,
            Ok(Err(error)) => {
                let Some(failed_job) = update_job_state(&job.id, |job| {
                    job.state = JobState::Failed;
                    job.stage = JobState::Failed.default_stage();
                    job.finished_at = Some(Local::now());
                    job.error = Some(error.to_string());
                    job.owner_pid = None;
                })?
                else {
                    sync_processing_status();
                    continue;
                };
                maybe_mark_context_session_failed(&failed_job, &error.to_string(), None);
                sync_processing_status();
                on_job_update(&failed_job);
                continue;
            }
            Err(payload) => {
                let message = format!(
                    "transcription worker panicked: {}",
                    describe_panic_payload(payload.as_ref())
                );
                tracing::error!(
                    job_id = %job.id,
                    error = %message,
                    "transcription worker caught panic"
                );
                let Some(failed_job) = update_job_state(&job.id, |job| {
                    job.state = JobState::Failed;
                    job.stage = JobState::Failed.default_stage();
                    job.finished_at = Some(Local::now());
                    job.error = Some(message.clone());
                    job.owner_pid = None;
                })?
                else {
                    sync_processing_status();
                    continue;
                };
                maybe_mark_context_session_failed(&failed_job, &message, None);
                sync_processing_status();
                on_job_update(&failed_job);
                continue;
            }
        };

        if artifact.frontmatter.status == Some(OutputStatus::NoSpeech) {
            let terminal_state = terminal_state_for_artifact(&artifact);
            let Some(review_job) = update_job_state(&job.id, |job| {
                job.state = terminal_state;
                job.stage = terminal_state.default_stage();
                apply_transcript_artifact_metadata(job, &artifact);
                job.finished_at = Some(Local::now());
                job.owner_pid = None;
                job.error = Some(
                    artifact
                        .frontmatter
                        .filter_diagnosis
                        .clone()
                        .unwrap_or_else(|| "Transcript requires manual review.".into()),
                );
            })?
            else {
                sync_processing_status();
                continue;
            };
            crate::events::append_event(crate::events::audio_processed_event(
                &artifact.write_result,
                &audio_path.display().to_string(),
            ));
            crate::events::append_event(crate::events::recording_completed_event(
                &artifact.write_result,
                &recording_duration(&review_job),
            ));
            maybe_mark_context_session_complete(&review_job, artifact.write_result.content_type);
            if let Err(error) = crate::graph::rebuild_index(config) {
                tracing::warn!(error = %error, "graph index rebuild failed after queued job");
            }
            refresh_qmd_collection(config);
            sync_processing_status();
            on_job_update(&review_job);
            continue;
        }

        let Some(updated_job) = update_job_state(&job.id, |job| {
            job.state = JobState::TranscriptOnly;
            job.stage = JobState::TranscriptOnly.default_stage();
            apply_transcript_artifact_metadata(job, &artifact);
        })?
        else {
            sync_processing_status();
            continue;
        };
        job = updated_job;
        sync_processing_status();
        on_job_update(&job);

        // Same `catch_unwind` rationale as the transcribe call above: the
        // enrich path runs the LLM summarizer, diarization, and graph
        // updates, any of which can panic in native or third-party code.
        // We want a clean `Failed` job, not a crashed worker (issue #229).
        let enrich_outcome = catch_unwind(AssertUnwindSafe(|| {
            pipeline::enrich_transcript_artifact(
                &audio_path,
                &artifact,
                config,
                &context,
                |stage| {
                    let state = stage_state(stage);
                    if let Ok(Some(job)) = update_job_state(&job.id, |job| {
                        job.state = state;
                        job.stage = state.default_stage();
                    }) {
                        sync_processing_status();
                        on_job_update(&job);
                    }
                },
            )
        }));
        let enrich_result = match enrich_outcome {
            Ok(result) => result,
            Err(payload) => {
                let message = format!(
                    "enrichment worker panicked: {}",
                    describe_panic_payload(payload.as_ref())
                );
                tracing::error!(
                    job_id = %job.id,
                    error = %message,
                    "enrichment worker caught panic"
                );
                Err(MinutesError::Transcribe(
                    TranscribeError::TranscriptionFailed(message),
                ))
            }
        };

        match enrich_result {
            Ok(result) => {
                let terminal_state = terminal_state_for_artifact(&artifact);
                let Some(completed_job) = update_job_state(&job.id, |job| {
                    job.state = terminal_state;
                    job.stage = terminal_state.default_stage();
                    job.output_path = Some(result.path.display().to_string());
                    job.title = Some(result.title.clone());
                    job.word_count = Some(result.word_count);
                    job.recording_health = artifact.frontmatter.recording_health.clone();
                    job.finished_at = Some(Local::now());
                    job.owner_pid = None;
                })?
                else {
                    sync_processing_status();
                    continue;
                };
                crate::events::append_event(crate::events::audio_processed_event(
                    &result,
                    &audio_path.display().to_string(),
                ));
                crate::events::append_event(crate::events::recording_completed_event(
                    &result,
                    &recording_duration(&completed_job),
                ));
                if let Err(error) = crate::graph::rebuild_index(config) {
                    tracing::warn!(error = %error, "graph index rebuild failed after queued job");
                }
                refresh_qmd_collection(config);
                // Run post_record hook (async, non-blocking)
                pipeline::run_post_record_hook(config, &result.path);
                if completed_job.state == JobState::Complete {
                    preserve_audio_alongside_output(&completed_job, config);
                }
                // Reload job after preserve may have updated audio_path
                let final_job = load_job(&completed_job.id).unwrap_or(completed_job);
                maybe_mark_context_session_complete(&final_job, result.content_type);
                sync_processing_status();
                on_job_update(&final_job);
            }
            Err(error) => {
                let Some(failed_job) = update_job_state(&job.id, |job| {
                    job.state = JobState::Failed;
                    job.stage = JobState::Failed.default_stage();
                    job.finished_at = Some(Local::now());
                    job.error = Some(error.to_string());
                    job.owner_pid = None;
                })?
                else {
                    sync_processing_status();
                    continue;
                };
                maybe_mark_context_session_failed(&failed_job, &error.to_string(), None);
                sync_processing_status();
                on_job_update(&failed_job);
            }
        }
    }

    sync_processing_status();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::{Frontmatter, WriteResult};

    fn with_temp_home<T>(f: impl FnOnce(&tempfile::TempDir) -> T) -> T {
        let _guard = crate::test_home_env_lock();
        let dir = tempfile::tempdir().unwrap();
        // Set HOME (Unix) and USERPROFILE (Windows) so dirs::home_dir() resolves to temp
        let original_home = std::env::var_os("HOME");
        let original_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", dir.path());
        std::env::set_var("USERPROFILE", dir.path());
        let result = f(&dir);
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        if let Some(up) = original_userprofile {
            std::env::set_var("USERPROFILE", up);
        } else {
            std::env::remove_var("USERPROFILE");
        }
        result
    }

    #[derive(Debug, Clone, Copy)]
    enum FakeProcessingOutcome {
        NoSpeech,
        WorkerCrash,
        ProcessingTimeout,
        FailedTranscription,
    }

    fn create_sample_bearing_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for sample in 0..1_600_i16 {
            writer.write_sample(sample.saturating_mul(4)).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn fake_no_speech_artifact(audio_path: &Path) -> pipeline::TranscriptArtifact {
        let write_result = WriteResult {
            path: audio_path.with_extension("md"),
            title: "Capture reliability fixture".into(),
            word_count: 0,
            content_type: ContentType::Meeting,
        };
        pipeline::TranscriptArtifact {
            write_result,
            frontmatter: Frontmatter {
                title: "Capture reliability fixture".into(),
                r#type: ContentType::Meeting,
                date: Local::now(),
                duration: "0m".into(),
                source: None,
                status: Some(OutputStatus::NoSpeech),
                tags: vec![],
                attendees: vec![],
                attendees_raw: None,
                calendar_event: None,
                people: vec![],
                entities: crate::markdown::EntityLinks::default(),
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
                name_corrections: vec![],
                recording_health: None,
                speaker_mapping: None,
                summarization: None,
                processing_warnings: vec![],
                template: None,
                filter_diagnosis: Some("fake engine found no speech".into()),
            },
            transcript: String::new(),
            diarization_audio_path: None,
        }
    }

    #[test]
    fn stop_deadline_preserves_sample_bearing_wav_for_every_processing_failure_outcome() {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let test_thread = std::thread::spawn(move || {
            with_temp_home(|dir| {
                let mut config = Config {
                    output_dir: dir.path().join("output"),
                    ..Config::default()
                };
                config.summarization.engine = "none".into();
                config.diarization.engine = "none".into();

                for outcome in [
                    FakeProcessingOutcome::NoSpeech,
                    FakeProcessingOutcome::WorkerCrash,
                    FakeProcessingOutcome::ProcessingTimeout,
                    FakeProcessingOutcome::FailedTranscription,
                ] {
                    let current_wav = dir.path().join(format!("current-{outcome:?}.wav"));
                    create_sample_bearing_wav(&current_wav);

                    let stop_started = std::time::Instant::now();
                    let queued = queue_live_capture(
                        CaptureMode::Meeting,
                        Some(format!("{outcome:?}")),
                        &current_wav,
                        None,
                        None,
                        None,
                        Some(Local::now()),
                        None,
                        None,
                        None,
                    )
                    .unwrap();
                    assert!(
                        stop_started.elapsed() < std::time::Duration::from_secs(1),
                        "stop-to-queue deadline exceeded for {outcome:?}"
                    );
                    assert!(
                        !current_wav.exists(),
                        "capture must move atomically into the queue"
                    );

                    let mut transcribe =
                        move |audio_path: &Path,
                              _: ContentType,
                              _: Option<&str>,
                              _: &Config,
                              _: &BackgroundPipelineContext,
                              _: Option<&Path>| {
                            match outcome {
                                FakeProcessingOutcome::NoSpeech => {
                                    Ok(fake_no_speech_artifact(audio_path))
                                }
                                FakeProcessingOutcome::WorkerCrash => {
                                    let (output, timed_out) =
                                        crate::engine_process::output_with_timeout(
                                            crate::engine_process::aborted_fixture_command(),
                                            std::time::Duration::from_secs(2),
                                        )
                                        .unwrap();
                                    assert!(!timed_out);
                                    assert!(!output.status.success());
                                    panic!("fake engine aborted");
                                }
                                FakeProcessingOutcome::ProcessingTimeout => {
                                    let (_, timed_out) =
                                        crate::engine_process::output_with_timeout(
                                            crate::engine_process::blocked_fixture_command(),
                                            std::time::Duration::from_millis(150),
                                        )
                                        .unwrap();
                                    assert!(timed_out);
                                    Err(TranscribeError::TranscriptionFailed(
                                        "fake engine processing timeout".into(),
                                    )
                                    .into())
                                }
                                FakeProcessingOutcome::FailedTranscription => {
                                    Err(TranscribeError::TranscriptionFailed(
                                        "fake engine returned an error".into(),
                                    )
                                    .into())
                                }
                            }
                        };
                    let mut on_job_update = |_: &ProcessingJob| {};
                    process_pending_jobs_with_transcriber(
                        &config,
                        &mut on_job_update,
                        &mut transcribe,
                    )
                    .unwrap();

                    let finalized = load_job(&queued.id).expect("terminal job remains inspectable");
                    let expected_state = match outcome {
                        FakeProcessingOutcome::NoSpeech => JobState::NeedsReview,
                        _ => JobState::Failed,
                    };
                    assert_eq!(finalized.state, expected_state, "outcome {outcome:?}");

                    let preserved_wav = PathBuf::from(&finalized.audio_path);
                    assert!(preserved_wav.exists(), "WAV lost for {outcome:?}");
                    let mut reader = hound::WavReader::open(&preserved_wav).unwrap();
                    assert!(
                        reader.duration() > 0,
                        "WAV header has no samples for {outcome:?}"
                    );
                    assert!(
                        reader
                            .samples::<i16>()
                            .next()
                            .transpose()
                            .unwrap()
                            .is_some(),
                        "WAV contains no readable sample for {outcome:?}"
                    );
                }
            });
            done_tx.send(()).unwrap();
        });

        match done_rx.recv_timeout(std::time::Duration::from_secs(8)) {
            Ok(()) => test_thread.join().unwrap(),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                test_thread.join().unwrap();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                panic!("capture stop/failure suite exceeded its hard deadline")
            }
        }
    }

    #[test]
    fn queue_live_capture_moves_audio_and_writes_job_file() {
        with_temp_home(|_| {
            let current_wav = pid::current_wav_path();
            if let Some(parent) = current_wav.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&current_wav, b"fake-wav").unwrap();

            let current_screens = crate::screen::screens_dir_for(&current_wav);
            fs::create_dir_all(&current_screens).unwrap();
            fs::write(current_screens.join("screen-0000-0000s.png"), b"png").unwrap();

            let job = queue_live_capture(
                CaptureMode::Meeting,
                Some("Back to back".into()),
                &current_wav,
                Some("note".into()),
                Some("context".into()),
                Some(Local::now()),
                Some(Local::now()),
                None,
                None,
                None,
            )
            .unwrap();

            assert!(!current_wav.exists());
            assert!(job_path(&job.id).exists());
            assert!(PathBuf::from(&job.audio_path).exists());
            assert!(crate::screen::screens_dir_for(Path::new(&job.audio_path)).exists());
        });
    }

    #[test]
    fn queue_live_capture_moves_stems_with_audio() {
        with_temp_home(|_| {
            let current_wav = pid::current_wav_path();
            if let Some(parent) = current_wav.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&current_wav, b"fake-wav").unwrap();

            let stems = crate::capture::stem_paths_for(&current_wav).unwrap();
            fs::write(&stems.voice, b"voice").unwrap();
            fs::write(&stems.system, b"system").unwrap();

            let job = queue_live_capture(
                CaptureMode::Meeting,
                Some("Dual source".into()),
                &current_wav,
                None,
                None,
                Some(Local::now()),
                Some(Local::now()),
                None,
                None,
                None,
            )
            .unwrap();

            let job_audio = PathBuf::from(&job.audio_path);
            let moved_stems = crate::capture::stem_paths_for(&job_audio).unwrap();
            assert!(job_audio.exists());
            assert!(moved_stems.voice.exists());
            assert!(moved_stems.system.exists());
            assert!(!stems.voice.exists());
            assert!(!stems.system.exists());
        });
    }

    #[test]
    fn queue_live_capture_preserves_native_mov_extension_and_stems() {
        with_temp_home(|tmp| {
            let native_dir = tmp.path().join(".minutes/native-captures");
            fs::create_dir_all(&native_dir).unwrap();
            let current_mov = native_dir.join("2026-05-19-120148-call.mov");
            fs::write(&current_mov, b"mov-placeholder").unwrap();

            let stems = crate::capture::stem_paths_for(&current_mov).unwrap();
            fs::write(&stems.voice, b"voice").unwrap();
            fs::write(&stems.system, b"system").unwrap();

            let job = queue_live_capture(
                CaptureMode::Meeting,
                Some("Native call".into()),
                &current_mov,
                None,
                None,
                Some(Local::now()),
                Some(Local::now()),
                None,
                None,
                None,
            )
            .unwrap();

            let job_audio = PathBuf::from(&job.audio_path);
            assert_eq!(
                job_audio.extension().and_then(|ext| ext.to_str()),
                Some("mov")
            );
            assert!(job_audio.exists());
            assert!(!current_mov.exists());

            let moved_stems = crate::capture::stem_paths_for(&job_audio).unwrap();
            assert!(moved_stems.voice.exists());
            assert!(moved_stems.system.exists());
            assert!(!stems.voice.exists());
            assert!(!stems.system.exists());
        });
    }

    #[test]
    fn queueing_surviving_native_stem_keeps_and_preserves_capture_group() {
        with_temp_home(|tmp| {
            let native_dir = tmp.path().join(".minutes/native-captures");
            fs::create_dir_all(&native_dir).unwrap();
            let anchor = native_dir.join("2026-07-15-120148-call.mov");
            let voice = native_dir.join("2026-07-15-120148-call.voice.wav");
            let system = native_dir.join("2026-07-15-120148-call.system.wav");
            fs::write(&anchor, b"mov-anchor").unwrap();
            fs::write(&voice, b"silent-mic-placeholder").unwrap();
            fs::write(&system, b"recoverable-system-audio").unwrap();

            let mut job = queue_live_capture(
                CaptureMode::Meeting,
                Some("Recovered native call".into()),
                &system,
                None,
                None,
                Some(Local::now()),
                Some(Local::now()),
                None,
                None,
                None,
            )
            .unwrap();

            let queued_system = PathBuf::from(&job.audio_path);
            assert!(queued_system
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".system.wav")));
            let queued_group = native_stem_group_for_survivor(&queued_system).unwrap();
            assert!(
                queued_group.anchor.exists(),
                ".mov anchor must follow survivor"
            );
            assert!(
                queued_group.voice.exists(),
                "silent mic stem must remain recoverable"
            );
            assert!(
                queued_group.system.exists(),
                "surviving stem must be queued"
            );
            assert!(!anchor.exists());
            assert!(!voice.exists());
            assert!(!system.exists());

            let output_path = tmp.path().join("meetings/recovered.md");
            fs::create_dir_all(output_path.parent().unwrap()).unwrap();
            fs::write(&output_path, "# recovered").unwrap();
            job.output_path = Some(output_path.display().to_string());
            preserve_audio_alongside_output(&job, &Config::default());

            let preserved_anchor = output_path.with_extension("mov");
            let preserved_stems = crate::capture::stem_paths_for(&preserved_anchor).unwrap();
            assert!(preserved_anchor.exists());
            assert!(preserved_stems.voice.exists());
            assert!(preserved_stems.system.exists());
            assert!(!queued_group.anchor.exists());
            assert!(!queued_group.voice.exists());
            assert!(!queued_group.system.exists());
        });
    }

    #[test]
    fn screen_move_failure_rolls_surviving_native_group_back_before_queue() {
        with_temp_home(|tmp| {
            let native_dir = tmp.path().join(".minutes/native-captures");
            fs::create_dir_all(&native_dir).unwrap();
            let anchor = native_dir.join("screen-rollback.mov");
            let voice = native_dir.join("screen-rollback.voice.wav");
            let system = native_dir.join("screen-rollback.system.wav");
            fs::write(&anchor, b"mov-anchor").unwrap();
            fs::write(&voice, b"silent-mic-placeholder").unwrap();
            fs::write(&system, b"recoverable-system-audio").unwrap();
            let old_screens = crate::screen::screens_dir_for(&anchor);
            fs::create_dir_all(&old_screens).unwrap();
            fs::write(old_screens.join("source.png"), b"source").unwrap();

            let job_id = "screen-move-failure";
            let queued_system = job_capture_path_for_source(job_id, &system);
            let queued_group = native_stem_group_for_survivor(&queued_system).unwrap();
            let error = move_capture_into_job_with_screen_mover(
                job_id,
                &system,
                |_source, _destination| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected screen move failure",
                    ))
                },
            )
            .expect_err("injected screen move must fail the queue operation");

            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert!(anchor.exists(), ".mov anchor must roll back");
            assert!(voice.exists(), "voice stem must roll back");
            assert!(system.exists(), "surviving system stem must roll back");
            assert!(!queued_group.anchor.exists());
            assert!(!queued_group.voice.exists());
            assert!(!queued_group.system.exists());
            assert!(old_screens.join("source.png").exists());
            assert!(
                !job_path(job_id).exists(),
                "no manifest may exist for a rolled-back queue"
            );
        });
    }

    #[test]
    fn queue_live_capture_persists_recording_health() {
        with_temp_home(|_| {
            let current_wav = pid::current_wav_path();
            if let Some(parent) = current_wav.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&current_wav, b"fake-wav").unwrap();
            let health = crate::health::recording_health_for_skipped_system_audio_probe(
                "operator accepted mic-only call",
            );

            let job = queue_live_capture_with_recording_health(
                CaptureMode::Meeting,
                Some("Probe skipped".into()),
                &current_wav,
                None,
                None,
                Some(Local::now()),
                Some(Local::now()),
                None,
                None,
                None,
                None,
                None,
                Some(health.clone()),
            )
            .unwrap();

            let loaded = load_job(&job.id).unwrap();
            assert_eq!(loaded.recording_health, Some(health));
        });
    }

    #[test]
    fn queue_live_capture_persists_consent_metadata() {
        with_temp_home(|_| {
            let current_wav = pid::current_wav_path();
            if let Some(parent) = current_wav.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&current_wav, b"fake-wav").unwrap();

            let job = queue_live_capture_with_recording_health(
                CaptureMode::Meeting,
                Some("Consent metadata".into()),
                &current_wav,
                None,
                None,
                Some(Local::now()),
                Some(Local::now()),
                None,
                None,
                None,
                Some(crate::markdown::ConsentBasis::NoticeInInvite),
                Some("Shared in the invite.".into()),
                None,
            )
            .unwrap();

            let loaded = load_job(&job.id).unwrap();
            assert_eq!(
                loaded.consent,
                Some(crate::markdown::ConsentBasis::NoticeInInvite)
            );
            assert_eq!(
                loaded.consent_notice.as_deref(),
                Some("Shared in the invite.")
            );

            let context = job_context(&loaded);
            assert_eq!(
                context.consent,
                Some(crate::markdown::ConsentBasis::NoticeInInvite)
            );
            assert_eq!(
                context.consent_notice.as_deref(),
                Some("Shared in the invite.")
            );
        });
    }

    #[test]
    fn preserve_audio_alongside_output_moves_stems_too() {
        with_temp_home(|tmp| {
            let jobs_root = jobs_dir();
            fs::create_dir_all(&jobs_root).unwrap();

            let audio_path = jobs_root.join("job-preserve.wav");
            fs::write(&audio_path, b"mixed").unwrap();
            let stems = crate::capture::stem_paths_for(&audio_path).unwrap();
            fs::write(&stems.voice, b"voice").unwrap();
            fs::write(&stems.system, b"system").unwrap();

            let output_path = tmp.path().join("meetings/final.md");
            fs::create_dir_all(output_path.parent().unwrap()).unwrap();
            fs::write(&output_path, "# final").unwrap();

            let job = ProcessingJob {
                id: "job-preserve".into(),
                mode: CaptureMode::Meeting,
                content_type: ContentType::Meeting,
                title: Some("preserve".into()),
                audio_path: audio_path.display().to_string(),
                output_path: Some(output_path.display().to_string()),
                state: JobState::Complete,
                stage: None,
                created_at: Local::now(),
                started_at: None,
                finished_at: None,
                notice_dismissed_at: None,
                recording_started_at: None,
                recording_finished_at: None,
                context_session_id: None,
                user_notes: None,
                pre_context: None,
                consent: None,
                consent_notice: None,
                calendar_event: None,
                template_slug: None,
                recording_health: None,
                word_count: None,
                error: None,
                owner_pid: None,
                retry_count: 0,
            };
            write_job(&job).unwrap();

            preserve_audio_alongside_output(&job, &Config::default());

            let preserved_audio = output_path.with_extension("wav");
            let preserved_stems = crate::capture::stem_paths_for(&preserved_audio).unwrap();
            assert!(preserved_audio.exists());
            assert!(preserved_stems.voice.exists());
            assert!(preserved_stems.system.exists());
            assert!(!audio_path.exists());
            assert!(!stems.voice.exists());
            assert!(!stems.system.exists());
        });
    }

    #[test]
    fn preserve_audio_alongside_output_preserves_native_mov_extension_and_stems() {
        with_temp_home(|tmp| {
            let jobs_root = jobs_dir();
            fs::create_dir_all(&jobs_root).unwrap();

            let audio_path = jobs_root.join("job-preserve-native.mov");
            fs::write(&audio_path, b"mov-anchor").unwrap();
            let stems = crate::capture::stem_paths_for(&audio_path).unwrap();
            fs::write(&stems.voice, b"voice").unwrap();
            fs::write(&stems.system, b"system").unwrap();

            let output_path = tmp.path().join("meetings/native-final.md");
            fs::create_dir_all(output_path.parent().unwrap()).unwrap();
            fs::write(&output_path, "# native final").unwrap();

            let job = ProcessingJob {
                id: "job-preserve-native".into(),
                mode: CaptureMode::Meeting,
                content_type: ContentType::Meeting,
                title: Some("preserve native".into()),
                audio_path: audio_path.display().to_string(),
                output_path: Some(output_path.display().to_string()),
                state: JobState::Complete,
                stage: None,
                created_at: Local::now(),
                started_at: None,
                finished_at: None,
                notice_dismissed_at: None,
                recording_started_at: None,
                recording_finished_at: None,
                context_session_id: None,
                user_notes: None,
                pre_context: None,
                consent: None,
                consent_notice: None,
                calendar_event: None,
                template_slug: None,
                recording_health: None,
                word_count: None,
                error: None,
                owner_pid: None,
                retry_count: 0,
            };
            write_job(&job).unwrap();

            preserve_audio_alongside_output(&job, &Config::default());

            let preserved_audio = output_path.with_extension("mov");
            let preserved_stems = crate::capture::stem_paths_for(&preserved_audio).unwrap();
            assert!(preserved_audio.exists());
            assert!(!output_path.with_extension("wav").exists());
            assert!(preserved_stems.voice.exists());
            assert!(preserved_stems.system.exists());
            assert!(!audio_path.exists());
            assert!(!stems.voice.exists());
            assert!(!stems.system.exists());
        });
    }

    #[test]
    fn preserve_audio_alongside_output_respects_keep_after_summary() {
        with_temp_home(|tmp| {
            let jobs_root = jobs_dir();
            fs::create_dir_all(&jobs_root).unwrap();

            // Run the preserve step for one job per config value; return
            // whether that job's screens dir survived.
            let screens_dir_survives = |job_id: &str, keep: bool| {
                let audio_path = jobs_root.join(format!("{job_id}.wav"));
                fs::write(&audio_path, b"mixed").unwrap();

                let screens_dir = crate::screen::screens_dir_for(&audio_path);
                fs::create_dir_all(&screens_dir).unwrap();
                fs::write(screens_dir.join("0001.png"), b"png").unwrap();

                let output_path = tmp.path().join(format!("meetings/{job_id}.md"));
                fs::create_dir_all(output_path.parent().unwrap()).unwrap();
                fs::write(&output_path, "# out").unwrap();

                let job = ProcessingJob {
                    id: job_id.into(),
                    mode: CaptureMode::Meeting,
                    content_type: ContentType::Meeting,
                    title: Some("keep screens".into()),
                    audio_path: audio_path.display().to_string(),
                    output_path: Some(output_path.display().to_string()),
                    state: JobState::Complete,
                    stage: None,
                    created_at: Local::now(),
                    started_at: None,
                    finished_at: None,
                    notice_dismissed_at: None,
                    recording_started_at: None,
                    recording_finished_at: None,
                    context_session_id: None,
                    user_notes: None,
                    pre_context: None,
                    consent: None,
                    consent_notice: None,
                    calendar_event: None,
                    template_slug: None,
                    recording_health: None,
                    word_count: None,
                    error: None,
                    owner_pid: None,
                    retry_count: 0,
                };
                write_job(&job).unwrap();

                let mut config = Config::default();
                config.screen_context.keep_after_summary = keep;
                preserve_audio_alongside_output(&job, &config);

                screens_dir.exists()
            };

            assert!(
                screens_dir_survives("job-keep-screens", true),
                "screens dir must survive when keep_after_summary is on"
            );
            assert!(
                !screens_dir_survives("job-drop-screens", false),
                "screens dir must be cleaned up when keep_after_summary is off"
            );
        });
    }

    #[test]
    fn no_speech_artifacts_require_review_and_preserve_capture() {
        let artifact = pipeline::TranscriptArtifact {
            write_result: WriteResult {
                path: PathBuf::from("/tmp/review.md"),
                title: "Untitled Recording".into(),
                word_count: 0,
                content_type: ContentType::Meeting,
            },
            frontmatter: Frontmatter {
                title: "Untitled Recording".into(),
                r#type: ContentType::Meeting,
                date: Local::now(),
                duration: "5m".into(),
                source: None,
                status: Some(OutputStatus::NoSpeech),
                tags: vec![],
                attendees: vec![],
                attendees_raw: None,
                calendar_event: None,
                people: vec![],
                entities: crate::markdown::EntityLinks::default(),
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
                filter_diagnosis: Some("silence strip removed ALL audio".into()),
            },
            transcript: String::new(),
            diarization_audio_path: None,
        };

        assert_eq!(
            terminal_state_for_artifact(&artifact),
            JobState::NeedsReview
        );
        assert!(JobState::NeedsReview.is_terminal());
    }

    #[test]
    fn list_jobs_recovers_stale_worker_claims() {
        with_temp_home(|_| {
            let job = ProcessingJob {
                id: "job-stale".into(),
                mode: CaptureMode::Meeting,
                content_type: ContentType::Meeting,
                title: Some("stale".into()),
                audio_path: "/tmp/fake.wav".into(),
                output_path: None,
                state: JobState::Transcribing,
                stage: Some("Transcribing meeting".into()),
                created_at: Local::now(),
                started_at: Some(Local::now()),
                finished_at: None,
                notice_dismissed_at: None,
                recording_started_at: None,
                recording_finished_at: None,
                context_session_id: None,
                user_notes: None,
                pre_context: None,
                consent: None,
                consent_notice: None,
                calendar_event: None,
                template_slug: None,
                recording_health: None,
                word_count: None,
                error: None,
                owner_pid: Some(99_999_999),
                retry_count: 0,
            };
            write_job(&job).unwrap();

            let jobs = list_jobs();
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].state, JobState::Queued);
            assert_eq!(jobs[0].owner_pid, None);
            assert_eq!(jobs[0].retry_count, 1);
        });
    }

    #[test]
    fn list_jobs_demotes_to_failed_when_retry_cap_exceeded() {
        with_temp_home(|_| {
            // Already burned every auto-retry slot. A worker that died after
            // claiming this job should not get a fresh `Queued` chance —
            // that's exactly the "loop forever on a deterministic crash"
            // case from issue #229.
            let job = ProcessingJob {
                id: "job-burned".into(),
                mode: CaptureMode::Meeting,
                content_type: ContentType::Meeting,
                title: Some("burned".into()),
                audio_path: "/tmp/fake.wav".into(),
                output_path: None,
                state: JobState::Transcribing,
                stage: Some("Transcribing meeting".into()),
                created_at: Local::now(),
                started_at: Some(Local::now()),
                finished_at: None,
                notice_dismissed_at: None,
                recording_started_at: None,
                recording_finished_at: None,
                context_session_id: None,
                user_notes: None,
                pre_context: None,
                consent: None,
                consent_notice: None,
                calendar_event: None,
                template_slug: None,
                recording_health: None,
                word_count: None,
                error: None,
                owner_pid: Some(99_999_999),
                retry_count: MAX_AUTO_RETRIES,
            };
            write_job(&job).unwrap();

            // Trigger the recovery scan. The demoted job moves into the
            // archive so it no longer shows up in `list_jobs()` (which
            // only walks the active dir); verify the final state by
            // reading the archived record directly.
            let _ = list_jobs();

            let recovered = load_job(&job.id).expect("archived job should be loadable");
            assert_eq!(recovered.state, JobState::Failed);
            assert_eq!(recovered.owner_pid, None);
            assert_eq!(recovered.retry_count, MAX_AUTO_RETRIES + 1);
            assert!(recovered
                .error
                .as_deref()
                .is_some_and(|e| e.contains("crashed")));

            // The active copy should be gone — `update_job_state` moves it
            // across the boundary on terminal transition.
            assert!(!job_path(&job.id).exists());
            assert!(job_archive_path(&job.id).exists());
        });
    }

    #[test]
    fn manual_requeue_resets_retry_count() {
        with_temp_home(|dir| {
            let audio_path = dir.path().join("fake.wav");
            let job = ProcessingJob {
                id: "job-retry-reset".into(),
                mode: CaptureMode::Meeting,
                content_type: ContentType::Meeting,
                title: Some("retry reset".into()),
                audio_path: audio_path.display().to_string(),
                output_path: None,
                state: JobState::Failed,
                stage: Some("Processing failed".into()),
                created_at: Local::now(),
                started_at: Some(Local::now()),
                finished_at: Some(Local::now()),
                notice_dismissed_at: None,
                recording_started_at: None,
                recording_finished_at: None,
                context_session_id: None,
                user_notes: None,
                pre_context: None,
                consent: None,
                consent_notice: None,
                calendar_event: None,
                template_slug: None,
                recording_health: None,
                word_count: None,
                error: Some("crashed three times".into()),
                owner_pid: None,
                retry_count: MAX_AUTO_RETRIES + 1,
            };
            write_job(&job).unwrap();
            fs::write(&audio_path, b"fake-wav").unwrap();

            let requeued = requeue_job(&job.id).unwrap().unwrap();
            assert_eq!(requeued.state, JobState::Queued);
            assert_eq!(requeued.retry_count, 0);
        });
    }

    #[test]
    fn requeue_job_preserves_existing_output_path() {
        with_temp_home(|dir| {
            let audio_path = dir.path().join("fake.wav");
            let output_path = dir.path().join("existing.md").display().to_string();
            let job = ProcessingJob {
                id: "job-failed".into(),
                mode: CaptureMode::Meeting,
                content_type: ContentType::Meeting,
                title: Some("retry me".into()),
                audio_path: audio_path.display().to_string(),
                output_path: Some(output_path.clone()),
                state: JobState::Failed,
                stage: Some("Processing failed".into()),
                created_at: Local::now(),
                started_at: Some(Local::now()),
                finished_at: Some(Local::now()),
                notice_dismissed_at: None,
                recording_started_at: None,
                recording_finished_at: None,
                context_session_id: None,
                user_notes: None,
                pre_context: None,
                consent: None,
                consent_notice: None,
                calendar_event: None,
                template_slug: None,
                recording_health: None,
                word_count: Some(42),
                error: Some("boom".into()),
                owner_pid: None,
                retry_count: 0,
            };
            write_job(&job).unwrap();
            fs::write(&audio_path, b"fake-wav").unwrap();

            let requeued = requeue_job(&job.id).unwrap().unwrap();
            assert_eq!(requeued.id, job.id);
            assert_eq!(requeued.output_path.as_deref(), Some(output_path.as_str()));
            assert_eq!(requeued.state, JobState::Queued);
            assert_eq!(requeued.error, None);
            assert_eq!(requeued.finished_at, None);
        });
    }

    #[test]
    fn dismiss_job_notice_marks_retryable_job_and_requeue_clears_it() {
        with_temp_home(|dir| {
            let audio_path = dir.path().join("fake.wav");
            fs::write(&audio_path, b"fake-wav").unwrap();

            let job = ProcessingJob {
                id: "job-dismissed".into(),
                mode: CaptureMode::Meeting,
                content_type: ContentType::Meeting,
                title: Some("dismiss me".into()),
                audio_path: audio_path.display().to_string(),
                output_path: None,
                state: JobState::Failed,
                stage: Some("Processing failed".into()),
                created_at: Local::now(),
                started_at: Some(Local::now()),
                finished_at: Some(Local::now()),
                notice_dismissed_at: None,
                recording_started_at: None,
                recording_finished_at: None,
                context_session_id: None,
                user_notes: None,
                pre_context: None,
                consent: None,
                consent_notice: None,
                calendar_event: None,
                template_slug: None,
                recording_health: None,
                word_count: None,
                error: Some("boom".into()),
                owner_pid: None,
                retry_count: 0,
            };
            write_job(&job).unwrap();

            let dismissed = dismiss_job_notice(&job.id).unwrap().unwrap();
            assert!(dismissed.notice_dismissed_at.is_some());

            let requeued = requeue_job(&job.id).unwrap().unwrap();
            assert_eq!(requeued.state, JobState::Queued);
            assert_eq!(requeued.notice_dismissed_at, None);
            assert_eq!(requeued.error, None);
        });
    }

    #[test]
    fn requeue_job_rejects_non_retryable_state() {
        with_temp_home(|dir| {
            let audio_path = dir.path().join("fake.wav");
            fs::write(&audio_path, b"fake-wav").unwrap();

            let job = ProcessingJob {
                id: "job-complete".into(),
                mode: CaptureMode::Meeting,
                content_type: ContentType::Meeting,
                title: Some("done".into()),
                audio_path: audio_path.display().to_string(),
                output_path: Some(dir.path().join("done.md").display().to_string()),
                state: JobState::Complete,
                stage: None,
                created_at: Local::now(),
                started_at: Some(Local::now()),
                finished_at: Some(Local::now()),
                notice_dismissed_at: None,
                recording_started_at: None,
                recording_finished_at: None,
                context_session_id: None,
                user_notes: None,
                pre_context: None,
                consent: None,
                consent_notice: None,
                calendar_event: None,
                template_slug: None,
                recording_health: None,
                word_count: Some(42),
                error: None,
                owner_pid: None,
                retry_count: 0,
            };
            write_job(&job).unwrap();

            let requeued = requeue_job(&job.id).unwrap();
            assert!(requeued.is_none());

            let unchanged = load_job(&job.id).unwrap();
            assert_eq!(unchanged.state, JobState::Complete);
            assert_eq!(unchanged.output_path, job.output_path);
            assert_eq!(unchanged.finished_at, job.finished_at);
        });
    }

    #[test]
    fn processing_summary_prefers_active_work_over_newer_queued_jobs() {
        with_temp_home(|_| {
            let active = ProcessingJob {
                id: "job-active".into(),
                mode: CaptureMode::Meeting,
                content_type: ContentType::Meeting,
                title: Some("Older active job".into()),
                audio_path: "/tmp/old.wav".into(),
                output_path: None,
                state: JobState::Transcribing,
                stage: Some("Transcribing meeting".into()),
                created_at: Local::now() - chrono::Duration::minutes(5),
                started_at: Some(Local::now() - chrono::Duration::minutes(4)),
                finished_at: None,
                notice_dismissed_at: None,
                recording_started_at: None,
                recording_finished_at: None,
                context_session_id: None,
                user_notes: None,
                pre_context: None,
                consent: None,
                consent_notice: None,
                calendar_event: None,
                template_slug: None,
                recording_health: None,
                word_count: None,
                error: None,
                owner_pid: None,
                retry_count: 0,
            };
            let queued = ProcessingJob {
                id: "job-queued".into(),
                mode: CaptureMode::Meeting,
                content_type: ContentType::Meeting,
                title: Some("Newer queued job".into()),
                audio_path: "/tmp/new.wav".into(),
                output_path: None,
                state: JobState::Queued,
                stage: Some("Queued for processing".into()),
                created_at: Local::now(),
                started_at: None,
                finished_at: None,
                notice_dismissed_at: None,
                recording_started_at: None,
                recording_finished_at: None,
                context_session_id: None,
                user_notes: None,
                pre_context: None,
                consent: None,
                consent_notice: None,
                calendar_event: None,
                template_slug: None,
                recording_health: None,
                word_count: None,
                error: None,
                owner_pid: None,
                retry_count: 0,
            };

            write_job(&active).unwrap();
            write_job(&queued).unwrap();

            let summary = processing_summary().unwrap();
            assert_eq!(summary.id, "job-active");
            assert_eq!(summary.state, JobState::Transcribing);
        });
    }

    /// Build a `ProcessingJob` with sensible defaults, used by the archive-
    /// partition tests below. Inline struct literals are common in this file
    /// for readability of older tests; keeping a helper local to the new
    /// suite avoids drift while not refactoring the existing fixtures.
    fn make_test_job(id: &str, state: JobState) -> ProcessingJob {
        ProcessingJob {
            id: id.into(),
            mode: CaptureMode::Meeting,
            content_type: ContentType::Meeting,
            title: Some(format!("title-{id}")),
            audio_path: format!("/tmp/{id}.wav"),
            output_path: None,
            state,
            stage: state.default_stage(),
            created_at: Local::now(),
            started_at: None,
            finished_at: None,
            notice_dismissed_at: None,
            recording_started_at: None,
            recording_finished_at: None,
            context_session_id: None,
            user_notes: None,
            pre_context: None,
            consent: None,
            consent_notice: None,
            calendar_event: None,
            template_slug: None,
            recording_health: None,
            word_count: None,
            error: None,
            owner_pid: None,
            retry_count: 0,
        }
    }

    fn make_test_artifact(
        id: &str,
        status: OutputStatus,
        recording_health: Option<crate::markdown::RecordingHealth>,
    ) -> pipeline::TranscriptArtifact {
        pipeline::TranscriptArtifact {
            write_result: WriteResult {
                path: PathBuf::from(format!("/tmp/{id}.md")),
                title: format!("title-{id}"),
                word_count: 42,
                content_type: ContentType::Meeting,
            },
            frontmatter: Frontmatter {
                title: format!("title-{id}"),
                r#type: ContentType::Meeting,
                date: Local::now(),
                duration: "5m".into(),
                source: None,
                status: Some(status),
                tags: vec![],
                attendees: vec![],
                attendees_raw: None,
                calendar_event: None,
                people: vec![],
                entities: crate::markdown::EntityLinks::default(),
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
                recording_health,
                speaker_mapping: None,
                summarization: None,
                processing_warnings: Vec::new(),
                template: None,
                filter_diagnosis: None,
            },
            transcript: "[0:00] recovered call audio".into(),
            diarization_audio_path: None,
        }
    }

    #[test]
    fn legacy_recovery_job_persists_artifact_health_for_completion_notice() {
        let health = crate::health::recording_health_for_native_call_stem_recovery(
            crate::diarize::CaptureSource::System,
        );
        let artifact = make_test_artifact(
            "legacy-recovered-mov",
            OutputStatus::Degraded,
            Some(health.clone()),
        );
        let mut job = make_test_job("legacy-recovered-mov", JobState::Transcribing);
        assert!(job.recording_health.is_none());

        apply_transcript_artifact_metadata(&mut job, &artifact);

        assert_eq!(job.recording_health, Some(health));
        assert!(job
            .recording_health
            .as_ref()
            .and_then(|health| health.capture_warnings.first())
            .is_some_and(|warning| warning.message.contains("call/remote audio only")));
    }

    #[test]
    fn migration_moves_terminal_jobs_to_archive() {
        with_temp_home(|_| {
            let terminal = make_test_job("job-old-complete", JobState::Complete);
            let queued = make_test_job("job-still-queued", JobState::Queued);
            // write_job always targets active dir — simulates a pre-upgrade
            // user with terminal jobs cluttering the hot path.
            write_job(&terminal).unwrap();
            write_job(&queued).unwrap();
            assert!(job_path(&terminal.id).exists());

            migrate_terminal_jobs_to_archive().unwrap();

            assert!(
                !job_path(&terminal.id).exists(),
                "terminal job should be moved out of active dir"
            );
            assert!(
                job_archive_path(&terminal.id).exists(),
                "terminal job should now live in archive"
            );
            assert!(
                job_path(&queued.id).exists(),
                "queued job stays in active dir"
            );
        });
    }

    #[test]
    fn migration_is_idempotent_when_already_archived() {
        with_temp_home(|_| {
            let job = make_test_job("job-double-migrate", JobState::Failed);
            // Pre-existing archive copy (e.g. from an earlier migration in
            // another process, or from update_job_state). Migrate again with
            // the same job sitting in active — must not corrupt state.
            fs::create_dir_all(archive_dir()).unwrap();
            write_job_to(&job, &job_archive_path(&job.id)).unwrap();
            write_job(&job).unwrap();

            migrate_terminal_jobs_to_archive().unwrap();

            // Active copy is gone; archive remains and parses cleanly.
            assert!(!job_path(&job.id).exists());
            assert!(job_archive_path(&job.id).exists());
            let loaded = load_job(&job.id).unwrap();
            assert_eq!(loaded.state, JobState::Failed);
        });
    }

    #[test]
    fn move_to_archive_preserves_canonical_copy_on_already_exists() {
        // Critical correctness test: `fs::rename` silently overwrites the
        // destination on POSIX. We must not lose the canonical archive copy
        // when a stale-snapshot updater races against an already-archived
        // job. `move_to_archive` uses `fs::hard_link` to detect the conflict
        // and drops the active-side duplicate instead.
        with_temp_home(|_| {
            let job_id = "job-race";
            let canonical = make_test_job(job_id, JobState::Complete);
            // Canonical archive copy — represents what another process
            // wrote at T1.
            fs::create_dir_all(archive_dir()).unwrap();
            write_job_to(&canonical, &job_archive_path(job_id)).unwrap();
            // Stale active copy — represents what process A would write at
            // T2 from a snapshot loaded before T1. Different state to make
            // the data-loss case observable: if rename clobbered, the
            // archive would now contain Failed instead of Complete.
            let mut stale = make_test_job(job_id, JobState::Failed);
            stale.error = Some("stale snapshot".into());
            write_job(&stale).unwrap();

            move_to_archive(&job_path(job_id), &job_archive_path(job_id)).unwrap();

            assert!(
                !job_path(job_id).exists(),
                "active duplicate must be dropped after race detection"
            );
            let archived = load_job(job_id).unwrap();
            assert_eq!(
                archived.state,
                JobState::Complete,
                "canonical archive copy must survive the race"
            );
            assert!(
                archived.error.is_none(),
                "stale state must not have leaked into archive"
            );
        });
    }

    #[test]
    fn ensure_archive_initialized_writes_marker_and_skips_on_repeat() {
        with_temp_home(|_| {
            let terminal = make_test_job("job-marker", JobState::Complete);
            write_job(&terminal).unwrap();
            assert!(!jobs_dir().join(MIGRATION_MARKER).exists());

            ensure_archive_initialized();

            assert!(
                jobs_dir().join(MIGRATION_MARKER).exists(),
                "marker should be written after a successful sweep"
            );
            assert!(job_archive_path(&terminal.id).exists());

            // Second call is a no-op — drop a fresh terminal job into
            // active and confirm the marker prevents re-migration.
            let later = make_test_job("job-after-marker", JobState::Failed);
            write_job(&later).unwrap();
            ensure_archive_initialized();
            assert!(
                job_path(&later.id).exists(),
                "post-marker terminal jobs are routed by update_job_state, not by migration"
            );
        });
    }

    #[test]
    fn ensure_archive_initialized_writes_marker_when_jobs_dir_is_missing() {
        with_temp_home(|_| {
            assert!(
                !jobs_dir().exists(),
                "fresh installs should start without a jobs directory"
            );

            ensure_archive_initialized();

            assert!(
                jobs_dir().join(MIGRATION_MARKER).exists(),
                "fresh installs should still record a quiet successful migration"
            );
        });
    }

    #[test]
    fn update_job_state_moves_to_archive_on_terminal_transition() {
        with_temp_home(|_| {
            let job = make_test_job("job-becoming-terminal", JobState::Transcribing);
            write_job(&job).unwrap();
            assert!(job_path(&job.id).exists());
            assert!(!job_archive_path(&job.id).exists());

            update_job_state(&job.id, |j| j.state = JobState::Complete)
                .unwrap()
                .unwrap();

            assert!(
                !job_path(&job.id).exists(),
                "terminal job should leave active dir on transition"
            );
            assert!(
                job_archive_path(&job.id).exists(),
                "terminal job should land in archive"
            );
            // The on-disk record reflects the new state.
            let loaded = load_job(&job.id).unwrap();
            assert_eq!(loaded.state, JobState::Complete);
        });
    }

    #[test]
    fn update_job_state_moves_back_to_active_on_requeue() {
        with_temp_home(|_| {
            // Set up: terminal job already in archive (the steady-state shape
            // after either migration or a normal terminal transition).
            let job = make_test_job("job-requeue", JobState::Failed);
            fs::create_dir_all(archive_dir()).unwrap();
            write_job_to(&job, &job_archive_path(&job.id)).unwrap();

            update_job_state(&job.id, |j| {
                j.state = JobState::Queued;
                j.error = None;
            })
            .unwrap()
            .unwrap();

            assert!(
                job_path(&job.id).exists(),
                "requeued job should land back in active dir"
            );
            assert!(
                !job_archive_path(&job.id).exists(),
                "archive copy should be cleaned up after requeue"
            );
            let loaded = load_job(&job.id).unwrap();
            assert_eq!(loaded.state, JobState::Queued);
            assert_eq!(loaded.error, None);
        });
    }

    #[test]
    fn update_job_state_no_move_when_dir_unchanged() {
        with_temp_home(|_| {
            // Active → active (transitioning between non-terminal states).
            let job = make_test_job("job-no-move", JobState::Queued);
            write_job(&job).unwrap();

            update_job_state(&job.id, |j| j.state = JobState::Transcribing)
                .unwrap()
                .unwrap();

            assert!(job_path(&job.id).exists());
            assert!(!job_archive_path(&job.id).exists());
            assert_eq!(load_job(&job.id).unwrap().state, JobState::Transcribing);
        });
    }

    #[test]
    fn load_job_finds_archived_jobs_via_fallback() {
        with_temp_home(|_| {
            let job = make_test_job("job-only-in-archive", JobState::Complete);
            fs::create_dir_all(archive_dir()).unwrap();
            write_job_to(&job, &job_archive_path(&job.id)).unwrap();

            let loaded = load_job(&job.id).unwrap();
            assert_eq!(loaded.id, job.id);
            assert_eq!(loaded.state, JobState::Complete);
        });
    }

    #[test]
    fn list_jobs_raw_does_not_see_archive_subdir() {
        with_temp_home(|_| {
            let active_job = make_test_job("job-active-list", JobState::Transcribing);
            let archived_job = make_test_job("job-archived-list", JobState::Complete);
            write_job(&active_job).unwrap();
            fs::create_dir_all(archive_dir()).unwrap();
            write_job_to(&archived_job, &job_archive_path(&archived_job.id)).unwrap();

            let raw = list_jobs_raw();
            let ids: Vec<&str> = raw.iter().map(|j| j.id.as_str()).collect();
            assert!(ids.contains(&"job-active-list"));
            assert!(
                !ids.contains(&"job-archived-list"),
                "list_jobs_raw must skip archive subdir contents"
            );
        });
    }

    #[test]
    fn display_jobs_with_terminal_includes_archive_contents() {
        with_temp_home(|_| {
            let active_job = make_test_job("job-active-display", JobState::Transcribing);
            let archived_job = make_test_job("job-archived-display", JobState::Complete);
            write_job(&active_job).unwrap();
            fs::create_dir_all(archive_dir()).unwrap();
            write_job_to(&archived_job, &job_archive_path(&archived_job.id)).unwrap();

            // Without terminal: only active.
            let active_only = display_jobs(None, false);
            let active_ids: Vec<&str> = active_only.iter().map(|j| j.id.as_str()).collect();
            assert!(active_ids.contains(&"job-active-display"));
            assert!(!active_ids.contains(&"job-archived-display"));

            // With terminal: both come back.
            let all = display_jobs(None, true);
            let all_ids: Vec<&str> = all.iter().map(|j| j.id.as_str()).collect();
            assert!(all_ids.contains(&"job-active-display"));
            assert!(all_ids.contains(&"job-archived-display"));
        });
    }

    #[test]
    fn latest_terminal_job_picks_finished_at_over_created_at() {
        // The motivating regression: post-worker-exit notifications used
        // `display_jobs(Some(1), true)` sorted by `created_at` desc, which
        // surfaces the "newest queued" terminal job rather than the
        // "newest finished" one. A long-running reprocess that finishes
        // an old recording must win over a fresher recording that
        // happened to be terminated earlier.
        with_temp_home(|_| {
            fs::create_dir_all(archive_dir()).unwrap();

            let now = Local::now();
            let mut older_created = make_test_job("job-old-created", JobState::Complete);
            older_created.created_at = now - chrono::Duration::hours(3);
            older_created.finished_at = Some(now - chrono::Duration::seconds(10));

            let mut newer_created = make_test_job("job-new-created", JobState::Complete);
            newer_created.created_at = now - chrono::Duration::minutes(5);
            newer_created.finished_at = Some(now - chrono::Duration::minutes(4));

            write_job_to(&older_created, &job_archive_path(&older_created.id)).unwrap();
            write_job_to(&newer_created, &job_archive_path(&newer_created.id)).unwrap();

            let latest = latest_terminal_job().expect("at least one terminal job");
            assert_eq!(
                latest.id, "job-old-created",
                "finished_at must dominate created_at in the latest-terminal selection"
            );
        });
    }

    #[test]
    fn latest_terminal_job_falls_back_to_created_at_when_finished_at_missing() {
        // Older records (pre-finished_at field population) should still
        // surface — falling back to created_at preserves backwards compat.
        with_temp_home(|_| {
            fs::create_dir_all(archive_dir()).unwrap();

            let now = Local::now();
            let mut earlier = make_test_job("job-earlier", JobState::Complete);
            earlier.created_at = now - chrono::Duration::hours(2);
            earlier.finished_at = None;

            let mut later = make_test_job("job-later", JobState::Failed);
            later.created_at = now - chrono::Duration::minutes(1);
            later.finished_at = None;

            write_job_to(&earlier, &job_archive_path(&earlier.id)).unwrap();
            write_job_to(&later, &job_archive_path(&later.id)).unwrap();

            let latest = latest_terminal_job().expect("at least one terminal job");
            assert_eq!(latest.id, "job-later");
        });
    }

    #[test]
    fn latest_terminal_job_returns_none_when_archive_empty() {
        with_temp_home(|_| {
            // Don't create archive_dir at all — exercises the early-return
            // path for the common pre-migration case.
            assert!(latest_terminal_job().is_none());
        });
    }
}
