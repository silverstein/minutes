use crate::config::{Config, VALID_PARAKEET_MODELS};
use crate::error::TranscribeError;
use crate::health::HealthItem;
use crate::markdown::ContentType;
use crate::parakeet;
use crate::transcribe::{self, TranscribeResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use whisper_guard::segments as wg_segments;

#[derive(Debug, Clone)]
pub struct TranscriptionRequest {
    pub audio_path: PathBuf,
    pub content_type: ContentType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParakeetBackendStatus {
    pub backend_id: String,
    pub compiled: bool,
    pub model: String,
    pub warm: bool,
    pub ready: bool,
    pub binary: String,
    pub binary_found: bool,
    pub model_found: bool,
    pub tokenizer_found: bool,
    pub binary_path: Option<String>,
    pub model_path: Option<String>,
    pub tokenizer_path: Option<String>,
    pub tokenizer_label: Option<String>,
    pub install_dir: String,
    pub setup_command: String,
    pub guide_url: String,
    pub issues: Vec<String>,
    pub metadata: Option<parakeet::ParakeetInstallMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendWarmupResult {
    pub backend_id: String,
    pub model: String,
    pub elapsed_ms: u64,
    pub used_gpu: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionDiagnosticsSnapshot {
    pub backend_id: String,
    pub model: String,
    pub ready: bool,
    pub warm: bool,
    pub used_gpu: bool,
    pub chunking_strategy: String,
    pub issues: Vec<String>,
    pub metadata: Option<parakeet::ParakeetInstallMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParakeetBenchmarkReport {
    pub backend_id: String,
    pub model: String,
    pub gpu: bool,
    pub direct_elapsed_ms: u64,
    pub direct_segments: usize,
    pub helper_elapsed_ms: u64,
    pub helper_segments: usize,
}

fn warmed_backends() -> &'static Mutex<std::collections::HashSet<String>> {
    static WARMED: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    WARMED.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn warm_key(backend_id: &str, model: &str) -> String {
    format!("{backend_id}:{model}")
}

#[cfg(feature = "parakeet")]
fn mark_backend_warm(backend_id: &str, model: &str) {
    let mut warmed = warmed_backends()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    warmed.insert(warm_key(backend_id, model));
}

fn backend_is_warm(backend_id: &str, model: &str) -> bool {
    let warmed = warmed_backends()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    warmed.contains(&warm_key(backend_id, model))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptCleanupStage {
    DedupSegments,
    DedupInterleaved,
    StripForeignScript,
    CollapseNoiseMarkers,
    TrimTrailingNoise,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranscriptCleanupStageStat {
    pub stage: TranscriptCleanupStage,
    pub before: usize,
    pub after: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct TranscriptCleanupResult {
    pub lines: Vec<String>,
    pub stats: Vec<TranscriptCleanupStageStat>,
}

type TranscriptCleanupFn = fn(Vec<String>) -> Vec<String>;
type TranscriptCleanupStep = (TranscriptCleanupStage, TranscriptCleanupFn);

pub(crate) fn dedup_segments(lines: Vec<String>) -> Vec<String> {
    wg_segments::dedup_segments(&lines)
}

pub(crate) fn dedup_interleaved(lines: Vec<String>) -> Vec<String> {
    wg_segments::dedup_interleaved(&lines)
}

pub(crate) fn trim_trailing_noise(lines: Vec<String>) -> Vec<String> {
    wg_segments::trim_trailing_noise(&lines)
}

pub(crate) fn strip_foreign_script(lines: Vec<String>) -> Vec<String> {
    wg_segments::strip_foreign_script(&lines)
}

pub(crate) fn collapse_noise_markers(lines: Vec<String>) -> Vec<String> {
    wg_segments::collapse_noise_markers(&lines)
}

impl TranscriptCleanupResult {
    pub(crate) fn after(&self, stage: TranscriptCleanupStage) -> usize {
        self.stats
            .iter()
            .find(|stat| stat.stage == stage)
            .map(|stat| stat.after)
            .unwrap_or(self.lines.len())
    }
}

pub(crate) fn run_transcript_cleanup_pipeline(lines: Vec<String>) -> TranscriptCleanupResult {
    let mut stats = Vec::new();
    let mut current = lines;

    let stages: &[TranscriptCleanupStep] = &[
        (TranscriptCleanupStage::DedupSegments, dedup_segments),
        (TranscriptCleanupStage::DedupInterleaved, dedup_interleaved),
        (
            TranscriptCleanupStage::StripForeignScript,
            strip_foreign_script,
        ),
        (
            TranscriptCleanupStage::CollapseNoiseMarkers,
            collapse_noise_markers,
        ),
        (
            TranscriptCleanupStage::TrimTrailingNoise,
            trim_trailing_noise,
        ),
    ];

    for (stage, apply) in stages {
        let before = current.len();
        current = apply(current);
        stats.push(TranscriptCleanupStageStat {
            stage: *stage,
            before,
            after: current.len(),
        });
    }

    TranscriptCleanupResult {
        lines: current,
        stats,
    }
}

pub fn transcribe_request(
    request: &TranscriptionRequest,
    config: &Config,
) -> Result<TranscribeResult, TranscribeError> {
    match request.content_type {
        ContentType::Meeting => transcribe::transcribe_meeting(&request.audio_path, config),
        _ => transcribe::transcribe(&request.audio_path, config),
    }
}

pub fn transcribe_path_for_content(
    audio_path: &Path,
    content_type: ContentType,
    config: &Config,
) -> Result<TranscribeResult, TranscribeError> {
    let request = TranscriptionRequest {
        audio_path: audio_path.to_path_buf(),
        content_type,
    };
    transcribe_request(&request, config)
}

pub fn parakeet_guide_url() -> &'static str {
    "https://github.com/silverstein/minutes/blob/main/docs/PARAKEET.md"
}

pub fn parakeet_setup_command(model: &str) -> String {
    format!("minutes setup --parakeet --parakeet-model {}", model)
}

pub fn parakeet_backend_status(config: &Config) -> ParakeetBackendStatus {
    let backend_id = if config.transcription.parakeet_sidecar_enabled {
        "parakeet-sidecar".to_string()
    } else {
        "parakeet".to_string()
    };
    let compiled = cfg!(feature = "parakeet");
    let binary = config.transcription.parakeet_binary.clone();
    let model = config.transcription.parakeet_model.clone();
    let binary_path = which::which(&binary).ok();
    let resolved_model = parakeet::resolve_model_file(config, &model);
    let resolved_tokenizer =
        parakeet::resolve_tokenizer_file(config, &model, &config.transcription.parakeet_vocab);
    let metadata = parakeet::read_install_metadata(config, &model);
    let mut issues = Vec::new();

    if !compiled {
        issues.push("Parakeet support is not compiled into this build".to_string());
    }
    if !VALID_PARAKEET_MODELS.contains(&model.as_str()) {
        issues.push(format!(
            "unknown parakeet model '{}'. Valid: {}",
            model,
            VALID_PARAKEET_MODELS.join(", ")
        ));
    }
    if binary_path.is_none() {
        issues.push(format!("binary '{}' is not in PATH", binary));
    }
    if resolved_model.is_none() {
        issues.push(format!("model assets for '{}' are not installed", model));
    }
    if resolved_tokenizer.is_none() {
        issues.push("SentencePiece tokenizer is not installed".to_string());
    }
    if metadata.is_none() && resolved_model.is_some() && resolved_tokenizer.is_some() {
        issues.push("install metadata is missing; rerun setup to persist provenance".to_string());
    }
    if config.transcription.parakeet_sidecar_enabled
        && crate::parakeet_sidecar::resolve_server_binary(&binary).is_none()
    {
        issues.push("warm sidecar is enabled but example-server could not be resolved".to_string());
    }

    let tokenizer_label = resolved_tokenizer.as_ref().and_then(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.to_string())
    });

    ParakeetBackendStatus {
        backend_id: backend_id.clone(),
        compiled,
        model: model.clone(),
        warm: backend_is_warm(&backend_id, &model),
        ready: compiled
            && VALID_PARAKEET_MODELS.contains(&model.as_str())
            && binary_path.is_some()
            && resolved_model.is_some()
            && resolved_tokenizer.is_some(),
        binary,
        binary_found: binary_path.is_some(),
        model_found: resolved_model.is_some(),
        tokenizer_found: resolved_tokenizer.is_some(),
        binary_path: binary_path.map(|path| path.display().to_string()),
        model_path: resolved_model.map(|path| path.display().to_string()),
        tokenizer_path: resolved_tokenizer
            .as_ref()
            .map(|path| path.display().to_string()),
        tokenizer_label,
        install_dir: parakeet::install_dir(config, &model).display().to_string(),
        setup_command: parakeet_setup_command(&model),
        guide_url: parakeet_guide_url().to_string(),
        issues,
        metadata,
    }
}

pub fn parakeet_health_item(config: &Config) -> HealthItem {
    let status = parakeet_backend_status(config);
    let detail = if status.ready {
        let metadata_suffix = if let Some(metadata) = status.metadata.as_ref() {
            format!(
                " Metadata: {} from {}. Warm: {}.",
                metadata.source_artifact,
                metadata.source_repo,
                if status.warm { "yes" } else { "no" }
            )
        } else {
            format!(
                " Metadata missing; rerun `{}` after installing files to persist provenance. Warm: {}.",
                status.setup_command,
                if status.warm { "yes" } else { "no" }
            )
        };
        format!(
            "Parakeet {} ready. Model: {}. Tokenizer: {}.{}",
            status.model,
            status.model_path.as_deref().unwrap_or("unknown"),
            status.tokenizer_path.as_deref().unwrap_or("unknown"),
            metadata_suffix
        )
    } else {
        format!(
            "Parakeet not ready: {}. Run `{}` for the guided install path.",
            status.issues.join(", "),
            status.setup_command
        )
    };

    HealthItem {
        label: "Speech model".into(),
        state: if status.ready { "ready" } else { "attention" }.into(),
        detail,
        optional: false,
    }
}

pub fn diagnostics_snapshot(config: &Config) -> TranscriptionDiagnosticsSnapshot {
    let status = parakeet_backend_status(config);
    TranscriptionDiagnosticsSnapshot {
        backend_id: status.backend_id.clone(),
        model: status.model.clone(),
        ready: status.ready,
        warm: status.warm,
        used_gpu: cfg!(all(target_os = "macos", target_arch = "aarch64")),
        chunking_strategy: "meeting-vad-or-45s-fixed-chunks".into(),
        issues: status.issues.clone(),
        metadata: status.metadata.clone(),
    }
}

pub fn warmup_active_backend(config: &Config) -> Result<BackendWarmupResult, TranscribeError> {
    if config.transcription.engine != "parakeet" {
        return Err(TranscribeError::EngineNotAvailable(
            config.transcription.engine.clone(),
        ));
    }

    #[cfg(feature = "parakeet")]
    {
        if config.transcription.parakeet_sidecar_enabled {
            let model_path = transcribe::resolve_parakeet_model_path(config)?;
            let vocab_path = transcribe::resolve_parakeet_vocab_path(config)?;
            let vad_path = transcribe::resolve_parakeet_native_vad_path(config);
            let started = std::time::Instant::now();
            let _started_now = crate::parakeet_sidecar::warmup_global_sidecar(
                config,
                &model_path,
                &vocab_path,
                vad_path.as_deref(),
            )
            .map_err(|error| TranscribeError::ParakeetFailed(error.to_string()))?;
            mark_backend_warm("parakeet-sidecar", &config.transcription.parakeet_model);
            return Ok(BackendWarmupResult {
                backend_id: "parakeet-sidecar".into(),
                model: config.transcription.parakeet_model.clone(),
                elapsed_ms: started.elapsed().as_millis() as u64,
                used_gpu: cfg!(all(target_os = "macos", target_arch = "aarch64")),
            });
        }

        let stats = transcribe::warmup_parakeet(config)?;
        mark_backend_warm("parakeet", &stats.model);
        return Ok(BackendWarmupResult {
            backend_id: "parakeet".into(),
            model: stats.model,
            elapsed_ms: stats.elapsed_ms,
            used_gpu: stats.used_gpu,
        });
    }

    #[cfg(not(feature = "parakeet"))]
    {
        Err(TranscribeError::EngineNotAvailable("parakeet".into()))
    }
}

#[cfg(feature = "parakeet")]
pub fn benchmark_parakeet(
    helper_binary: &Path,
    binary: &str,
    model_path: &Path,
    audio_path: &Path,
    vocab_path: &Path,
    model_id: &str,
    gpu: bool,
    vad_path: Option<&Path>,
    vad_threshold: f32,
    config: &Config,
) -> Result<ParakeetBenchmarkReport, String> {
    let started = std::time::Instant::now();
    let direct = transcribe::run_parakeet_cli_structured(
        binary,
        model_path,
        audio_path,
        vocab_path,
        model_id,
        gpu,
        vad_path,
        vad_threshold,
        config,
    )
    .map_err(|error| error.to_string())?;
    let direct_elapsed_ms = started.elapsed().as_millis() as u64;

    let helper_started = std::time::Instant::now();
    let mut helper_command = std::process::Command::new(helper_binary);
    helper_command
        .arg("parakeet-helper")
        .args(["--binary", binary])
        .args([
            "--model-path",
            model_path
                .to_str()
                .ok_or_else(|| "model path is not valid UTF-8".to_string())?,
        ])
        .args([
            "--audio-path",
            audio_path
                .to_str()
                .ok_or_else(|| "audio path is not valid UTF-8".to_string())?,
        ])
        .args([
            "--vocab-path",
            vocab_path
                .to_str()
                .ok_or_else(|| "vocab path is not valid UTF-8".to_string())?,
        ])
        .args(["--model-id", model_id])
        .args(if gpu { vec!["--gpu"] } else { Vec::new() })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(vad_path) = vad_path.and_then(|path| path.to_str()) {
        helper_command
            .args(["--vad-path", vad_path])
            .args(["--vad-threshold", &vad_threshold.to_string()]);
    }
    let helper = helper_command.output().map_err(|error| error.to_string())?;
    if !helper.status.success() {
        return Err(format!(
            "helper benchmark failed: {}",
            String::from_utf8_lossy(&helper.stderr)
        ));
    }
    let helper_json: serde_json::Value =
        serde_json::from_slice(&helper.stdout).map_err(|error| error.to_string())?;

    Ok(ParakeetBenchmarkReport {
        backend_id: "parakeet".into(),
        model: model_id.into(),
        gpu,
        direct_elapsed_ms,
        direct_segments: direct.segments.len(),
        helper_elapsed_ms: helper_started.elapsed().as_millis() as u64,
        helper_segments: helper_json
            .get("segments")
            .and_then(|v| v.as_array())
            .map(|v| v.len())
            .unwrap_or(0),
    })
}
