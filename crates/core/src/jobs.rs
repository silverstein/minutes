use crate::calendar::CalendarEvent;
use crate::config::Config;
use crate::error::MinutesError;
use crate::markdown::{ContentType, OutputStatus};
use crate::pid::{self, CaptureMode, PidGuard};
use crate::pipeline::{self, BackgroundPipelineContext, PipelineStage};
use chrono::{DateTime, Local};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static JOB_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    pub recording_started_at: Option<DateTime<Local>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_finished_at: Option<DateTime<Local>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_event: Option<CalendarEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_pid: Option<u32>,
}

fn next_job_id() -> String {
    let ts = Local::now().format("%Y%m%d%H%M%S%3f");
    let pid = std::process::id();
    let counter = JOB_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("job-{}-{}-{}", ts, pid, counter)
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
    calendar_event: Option<CalendarEvent>,
) -> std::io::Result<ProcessingJob> {
    let job_id = next_job_id();
    let old_screen_dir = crate::screen::screens_dir_for(current_wav);
    let audio_path = move_capture_into_job(&job_id, current_wav)?;
    let new_screen_dir = crate::screen::screens_dir_for(&audio_path);
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
        recording_started_at,
        recording_finished_at,
        user_notes,
        pre_context,
        calendar_event,
        word_count: None,
        error: None,
        owner_pid: None,
    };
    if let Err(error) = write_job(&job) {
        if audio_path.exists() {
            fs::rename(&audio_path, current_wav).ok();
        }
        if new_screen_dir.exists() {
            if old_screen_dir.exists() {
                fs::remove_dir_all(&old_screen_dir).ok();
            }
            fs::rename(&new_screen_dir, &old_screen_dir).ok();
        }
        return Err(error);
    }
    Ok(job)
}

pub fn jobs_dir() -> PathBuf {
    Config::minutes_dir().join("jobs")
}

pub fn worker_pid_path() -> PathBuf {
    Config::minutes_dir().join("processing-worker.pid")
}

pub fn job_path(job_id: &str) -> PathBuf {
    jobs_dir().join(format!("{}.json", job_id))
}

pub fn job_capture_path(job_id: &str) -> PathBuf {
    jobs_dir().join(format!("{}.wav", job_id))
}

pub fn create_worker_guard() -> Result<PidGuard, crate::error::PidError> {
    pid::create_pid_guard(&worker_pid_path())
}

pub fn current_worker_pid() -> Option<u32> {
    pid::check_pid_file(&worker_pid_path()).ok().flatten()
}

pub fn move_capture_into_job(job_id: &str, current_wav: &Path) -> std::io::Result<PathBuf> {
    let dest = job_capture_path(job_id);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(current_wav, &dest)?;

    let old_screen_dir = crate::screen::screens_dir_for(current_wav);
    if old_screen_dir.exists() {
        let new_screen_dir = crate::screen::screens_dir_for(&dest);
        if let Some(parent) = new_screen_dir.parent() {
            fs::create_dir_all(parent)?;
        }
        if new_screen_dir.exists() {
            fs::remove_dir_all(&new_screen_dir).ok();
        }
        fs::rename(old_screen_dir, new_screen_dir)?;
    }

    Ok(dest)
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
    calendar_event: Option<CalendarEvent>,
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
        recording_started_at,
        recording_finished_at,
        user_notes,
        pre_context,
        calendar_event,
        word_count: None,
        error: None,
        owner_pid: None,
    };
    write_job(&job)?;
    Ok(job)
}

pub fn write_job(job: &ProcessingJob) -> std::io::Result<()> {
    let path = job_path(&job.id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(job)?;
    fs::write(&tmp, json)?;
    fs::rename(tmp, path)?;
    Ok(())
}

pub fn load_job(job_id: &str) -> Option<ProcessingJob> {
    let path = job_path(job_id);
    if !path.exists() {
        return None;
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<ProcessingJob>(&text).ok())
}

fn list_jobs_raw() -> Vec<ProcessingJob> {
    let mut jobs = Vec::new();
    let dir = jobs_dir();
    if !dir.exists() {
        return jobs;
    }

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

pub fn list_jobs() -> Vec<ProcessingJob> {
    let mut changed = false;
    let jobs = list_jobs_raw()
        .into_iter()
        .map(|mut job| {
            if !job.state.is_terminal()
                && job.owner_pid.is_some()
                && !job.owner_pid.map(pid::is_process_alive).unwrap_or(false)
            {
                job.state = JobState::Queued;
                job.stage = JobState::Queued.default_stage();
                job.owner_pid = None;
                job.started_at = None;
                changed = true;
            }
            job
        })
        .collect::<Vec<_>>();

    if changed {
        for job in &jobs {
            let _ = write_job(job);
        }
    }

    jobs
}

pub fn display_jobs(limit: Option<usize>, include_terminal: bool) -> Vec<ProcessingJob> {
    let mut jobs = list_jobs();
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

    let audio_path = PathBuf::from(&job.audio_path);
    if !audio_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("audio file missing for job {}", job_id),
        ));
    }

    let Some(requeued) = update_job_state(job_id, |job| {
        job.state = JobState::Queued;
        job.stage = JobState::Queued.default_stage();
        job.started_at = None;
        job.finished_at = None;
        job.error = None;
        job.owner_pid = None;
    })?
    else {
        return Ok(None);
    };
    sync_processing_status();
    Ok(Some(requeued))
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

pub fn update_job_state<F>(job_id: &str, update: F) -> std::io::Result<Option<ProcessingJob>>
where
    F: FnOnce(&mut ProcessingJob),
{
    let Some(mut job) = load_job(job_id) else {
        return Ok(None);
    };
    update(&mut job);
    write_job(&job)?;
    Ok(Some(job))
}

pub fn remove_capture_artifacts(job: &ProcessingJob) {
    let audio_path = PathBuf::from(&job.audio_path);
    if audio_path.exists() {
        fs::remove_file(&audio_path).ok();
    }
    let screens_dir = crate::screen::screens_dir_for(&audio_path);
    if screens_dir.exists() {
        fs::remove_dir_all(screens_dir).ok();
    }
}

fn terminal_state_for_artifact(artifact: &pipeline::TranscriptArtifact) -> JobState {
    if artifact.frontmatter.status == Some(OutputStatus::NoSpeech) {
        JobState::NeedsReview
    } else {
        JobState::Complete
    }
}

fn should_preserve_capture(state: JobState) -> bool {
    matches!(state, JobState::NeedsReview | JobState::Failed)
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
    let status = std::process::Command::new("qmd")
        .args(["update", "-c", collection])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if let Err(error) = status {
        tracing::debug!(error = %error, collection = %collection, "qmd update skipped");
    }
}

fn job_context(job: &ProcessingJob) -> BackgroundPipelineContext {
    BackgroundPipelineContext {
        sidecar: None,
        user_notes: job.user_notes.clone(),
        pre_context: job.pre_context.clone(),
        calendar_event: job.calendar_event.clone(),
        recorded_at: job.recording_finished_at.or(job.recording_started_at),
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

        let artifact = match pipeline::transcribe_to_artifact(
            &audio_path,
            job.content_type,
            job.title.as_deref(),
            config,
            &context,
            job.output_path.as_deref().map(Path::new),
        ) {
            Ok(artifact) => artifact,
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
                job.output_path = Some(artifact.write_result.path.display().to_string());
                job.title = Some(artifact.write_result.title.clone());
                job.word_count = Some(artifact.write_result.word_count);
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
            job.output_path = Some(artifact.write_result.path.display().to_string());
            job.title = Some(artifact.write_result.title.clone());
            job.word_count = Some(artifact.write_result.word_count);
        })?
        else {
            sync_processing_status();
            continue;
        };
        job = updated_job;
        sync_processing_status();
        on_job_update(&job);

        let enrich_result = pipeline::enrich_transcript_artifact(
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
        );

        match enrich_result {
            Ok(result) => {
                let terminal_state = terminal_state_for_artifact(&artifact);
                let Some(completed_job) = update_job_state(&job.id, |job| {
                    job.state = terminal_state;
                    job.stage = terminal_state.default_stage();
                    job.output_path = Some(result.path.display().to_string());
                    job.title = Some(result.title.clone());
                    job.word_count = Some(result.word_count);
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
                if !should_preserve_capture(completed_job.state) {
                    remove_capture_artifacts(&completed_job);
                }
                sync_processing_status();
                on_job_update(&completed_job);
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
            )
            .unwrap();

            assert!(!current_wav.exists());
            assert!(job_path(&job.id).exists());
            assert!(PathBuf::from(&job.audio_path).exists());
            assert!(crate::screen::screens_dir_for(Path::new(&job.audio_path)).exists());
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
                visibility: None,
                speaker_map: vec![],
                filter_diagnosis: Some("silence strip removed ALL audio".into()),
            },
            transcript: String::new(),
        };

        assert_eq!(
            terminal_state_for_artifact(&artifact),
            JobState::NeedsReview
        );
        assert!(JobState::NeedsReview.is_terminal());
        assert!(should_preserve_capture(JobState::NeedsReview));
        assert!(!should_preserve_capture(JobState::Complete));
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
                recording_started_at: None,
                recording_finished_at: None,
                user_notes: None,
                pre_context: None,
                calendar_event: None,
                word_count: None,
                error: None,
                owner_pid: Some(99_999_999),
            };
            write_job(&job).unwrap();

            let jobs = list_jobs();
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].state, JobState::Queued);
            assert_eq!(jobs[0].owner_pid, None);
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
                recording_started_at: None,
                recording_finished_at: None,
                user_notes: None,
                pre_context: None,
                calendar_event: None,
                word_count: Some(42),
                error: Some("boom".into()),
                owner_pid: None,
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
                recording_started_at: None,
                recording_finished_at: None,
                user_notes: None,
                pre_context: None,
                calendar_event: None,
                word_count: None,
                error: None,
                owner_pid: None,
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
                recording_started_at: None,
                recording_finished_at: None,
                user_notes: None,
                pre_context: None,
                calendar_event: None,
                word_count: None,
                error: None,
                owner_pid: None,
            };

            write_job(&active).unwrap();
            write_job(&queued).unwrap();

            let summary = processing_summary().unwrap();
            assert_eq!(summary.id, "job-active");
            assert_eq!(summary.state, JobState::Transcribing);
        });
    }
}
