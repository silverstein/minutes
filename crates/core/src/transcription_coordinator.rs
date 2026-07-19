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
    pub decode_hints: transcribe::DecodeHints,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParakeetBackendStatus {
    pub backend_id: String,
    pub compiled: bool,
    pub model: String,
    /// True once the backend has completed at least one transcription this
    /// process lifetime. NOT "a warm server is resident" — use `sidecar`
    /// for the user-facing throughput story (#295).
    pub warm: bool,
    /// Human-readable effective sidecar state: what live throughput will
    /// actually do, and why.
    pub sidecar: String,
    /// Effective sidecar decision (auto-resolved or explicit).
    pub sidecar_enabled: bool,
    /// Whether the example-server binary resolves on this machine.
    pub sidecar_binary_found: bool,
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
        ContentType::Meeting => transcribe::transcribe_meeting_with_hints(
            &request.audio_path,
            config,
            &request.decode_hints,
        ),
        _ => transcribe::transcribe_with_hints(&request.audio_path, config, &request.decode_hints),
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
        decode_hints: transcribe::DecodeHints::default(),
    };
    transcribe_request(&request, config)
}

pub fn transcribe_path_for_content_with_hints(
    audio_path: &Path,
    content_type: ContentType,
    config: &Config,
    decode_hints: transcribe::DecodeHints,
) -> Result<TranscribeResult, TranscribeError> {
    let request = TranscriptionRequest {
        audio_path: audio_path.to_path_buf(),
        content_type,
        decode_hints,
    };
    transcribe_request(&request, config)
}

pub(crate) fn transcribe_authorized_path_for_content_with_hints(
    input: &crate::pipeline::AuthorizedProcessAudioInput,
    content_type: ContentType,
    config: &Config,
    decode_hints: transcribe::DecodeHints,
) -> Result<TranscribeResult, TranscribeError> {
    transcribe::transcribe_authorized_with_hints(input, content_type, config, &decode_hints)
}

pub fn parakeet_guide_url() -> &'static str {
    "https://github.com/silverstein/minutes/blob/main/docs/architecture/parakeet.md"
}

pub fn parakeet_setup_command(model: &str) -> String {
    format!("minutes setup --parakeet --parakeet-model {}", model)
}

/// One honest sentence about what the sidecar will actually do (#295).
fn sidecar_state_label(config: &Config, effective: bool, binary_found: bool) -> String {
    let mode = match config.transcription.parakeet_sidecar_enabled {
        None => "auto",
        Some(true) => "forced on",
        Some(false) => "forced off",
    };
    if !crate::pipeline::private_audio_processing_supported() {
        format!(
            "unavailable ({mode}); Parakeet private-audio processing is unavailable on this platform"
        )
    } else if !crate::pipeline::parakeet_private_audio_transport_supported() {
        format!(
            "unavailable ({mode}); Parakeet's pathname-only CLI cannot receive sealed private audio on this platform, so transcription uses Whisper"
        )
    } else if !crate::pipeline::private_audio_processing_available() {
        format!(
            "unavailable ({mode}); secure private-audio storage is not available at runtime, so transcription uses Whisper"
        )
    } else if !crate::parakeet_sidecar::private_audio_transport_available() {
        format!(
            "unavailable ({mode}); secure direct subprocess protects private audio until exact descriptor transport is implemented"
        )
    } else if effective && binary_found {
        format!("warm server ({mode})")
    } else if effective {
        format!("{mode}, but example-server is missing; falling back to cold per-utterance runs")
    } else if binary_found {
        format!("off ({mode}); cold per-utterance runs")
    } else {
        format!(
            "unavailable ({mode}); cold per-utterance runs. Install example-server to enable (see {})",
            parakeet_guide_url()
        )
    }
}

pub fn parakeet_backend_status(config: &Config) -> ParakeetBackendStatus {
    let sidecar_effective = crate::parakeet_sidecar::sidecar_enabled_effective(config);
    let backend_id = if sidecar_effective {
        "parakeet-sidecar".to_string()
    } else {
        "parakeet".to_string()
    };
    let compiled = cfg!(feature = "parakeet");
    let binary = config.transcription.parakeet_binary.clone();
    let model = config.transcription.parakeet_model.clone();
    let resolved_binary = crate::parakeet::resolve_parakeet_binary(
        &binary,
        crate::parakeet::ResolveParakeetBinaryMode::WarnAndFallback,
    )
    .ok();
    let resolved_model = parakeet::resolve_model_file(config, &model);
    let resolved_tokenizer =
        parakeet::resolve_tokenizer_file(config, &model, &config.transcription.parakeet_vocab);
    let metadata = parakeet::read_install_metadata(config, &model);
    let mut issues = Vec::new();
    let private_audio_supported = crate::pipeline::private_audio_processing_supported();
    let parakeet_transport_supported =
        crate::pipeline::parakeet_private_audio_transport_supported();
    let private_audio_available =
        private_audio_supported && crate::pipeline::private_audio_processing_available();

    if !compiled {
        issues.push("Parakeet support is not compiled into this build".to_string());
    }
    if !private_audio_supported {
        issues.push(
            "secure private-audio processing is unavailable on this platform; Parakeet is disabled and transcription falls back to Whisper"
                .to_string(),
        );
    }
    if private_audio_supported && !parakeet_transport_supported {
        issues.push(
            "Parakeet's pathname-only CLI cannot receive Minutes' secure normalized audio on this platform; transcription uses Whisper"
                .to_string(),
        );
    }
    if private_audio_supported && parakeet_transport_supported && !private_audio_available {
        issues.push(
            "secure private-audio storage is unavailable at runtime; Parakeet is disabled and transcription falls back to Whisper"
                .to_string(),
        );
    }
    if !VALID_PARAKEET_MODELS.contains(&model.as_str()) {
        issues.push(format!(
            "unknown parakeet model '{}'. Valid: {}",
            model,
            VALID_PARAKEET_MODELS.join(", ")
        ));
    }
    if resolved_binary.is_none() {
        issues.push(format!("binary '{}' could not be resolved", binary));
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
    // Note: sidecar_enabled_effective above may also have resolved the binary;
    // status assembly is cold-path (settings/health), so the repeat lookup is
    // accepted for clarity over threading a resolved path through.
    let sidecar_binary_found = crate::parakeet_sidecar::resolve_server_binary(&binary).is_some();
    if config.transcription.parakeet_sidecar_enabled == Some(true)
        && !crate::parakeet_sidecar::private_audio_transport_available()
    {
        issues.push(if private_audio_supported && parakeet_transport_supported {
            "warm sidecar requested but its pathname-only protocol cannot receive Minutes' private-audio capability; using the secure direct subprocess".to_string()
        } else {
            "warm sidecar requested, but no secure Parakeet private-audio transport is available on this platform; using Whisper".to_string()
        });
    } else if config.transcription.parakeet_sidecar_enabled == Some(true) && !sidecar_binary_found {
        issues.push(
            "sidecar forced on in config but example-server could not be resolved; live transcription will run the cold per-utterance path".to_string(),
        );
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
        sidecar: sidecar_state_label(config, sidecar_effective, sidecar_binary_found),
        sidecar_enabled: sidecar_effective,
        sidecar_binary_found,
        ready: compiled
            && private_audio_supported
            && parakeet_transport_supported
            && private_audio_available
            && VALID_PARAKEET_MODELS.contains(&model.as_str())
            && resolved_binary.is_some()
            && resolved_model.is_some()
            && resolved_tokenizer.is_some(),
        binary,
        binary_found: resolved_binary.is_some(),
        model_found: resolved_model.is_some(),
        tokenizer_found: resolved_tokenizer.is_some(),
        binary_path: resolved_binary.map(|path| path.display().to_string()),
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
                " Metadata: {} from {}. Sidecar: {}.",
                metadata.source_artifact, metadata.source_repo, status.sidecar
            )
        } else {
            format!(
                " Metadata missing; rerun `{}` after installing files to persist provenance. Sidecar: {}.",
                status.setup_command,
                status.sidecar
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
        if !crate::pipeline::private_audio_processing_supported()
            || !crate::pipeline::parakeet_private_audio_transport_supported()
            || !crate::pipeline::private_audio_processing_available()
        {
            format!(
                "Parakeet not ready: {}. Whisper remains active on this platform.",
                status.issues.join(", ")
            )
        } else {
            format!(
                "Parakeet not ready: {}. Run `{}` for the guided install path.",
                status.issues.join(", "),
                status.setup_command
            )
        }
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

fn parakeet_warmup_selected(config: &Config) -> bool {
    config.transcription.engine == "parakeet"
        || config.effective_live_transcript_backend() == "parakeet"
}

pub fn warmup_active_backend(config: &Config) -> Result<BackendWarmupResult, TranscribeError> {
    if !parakeet_warmup_selected(config) {
        return Err(TranscribeError::EngineNotAvailable("parakeet".into()));
    }
    if !crate::pipeline::private_audio_processing_supported() {
        return Err(TranscribeError::EngineNotAvailable(
            "parakeet-private-audio".into(),
        ));
    }
    if !crate::pipeline::parakeet_private_audio_transport_supported() {
        return Err(TranscribeError::EngineNotAvailable(
            "parakeet-private-audio-transport".into(),
        ));
    }
    if !crate::pipeline::private_audio_processing_available() {
        return Err(TranscribeError::EngineNotAvailable(
            "parakeet-private-audio-runtime".into(),
        ));
    }

    #[cfg(feature = "parakeet")]
    {
        if crate::parakeet_sidecar::sidecar_enabled_effective(config) {
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
        Ok(BackendWarmupResult {
            backend_id: "parakeet".into(),
            model: stats.model,
            elapsed_ms: stats.elapsed_ms,
            used_gpu: stats.used_gpu,
        })
    }

    #[cfg(not(feature = "parakeet"))]
    {
        Err(TranscribeError::EngineNotAvailable("parakeet".into()))
    }
}

#[cfg(feature = "parakeet")]
#[allow(clippy::too_many_arguments)]
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
    let capability = crate::pipeline::parakeet_capability(true);
    if !capability.selectable {
        return Err(format!("Parakeet {}", capability.unavailable_reason()));
    }
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
        &transcribe::DecodeHints::default(),
    )
    .map_err(|error| error.to_string())?;
    let direct_elapsed_ms = started.elapsed().as_millis() as u64;

    let helper_started = std::time::Instant::now();
    let mut helper_command = crate::engine_process::command(helper_binary);
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

#[cfg(test)]
mod tests {
    use super::{parakeet_warmup_selected, sidecar_state_label};
    use crate::Config;

    #[test]
    fn parakeet_warmup_selection_includes_live_backend() {
        let mut config = Config::default();
        config.transcription.engine = "whisper".into();
        config.live_transcript.backend = "parakeet".into();

        assert!(parakeet_warmup_selected(&config));
    }

    #[test]
    fn parakeet_warmup_selection_skips_non_parakeet_backends() {
        let mut config = Config::default();
        config.transcription.engine = "whisper".into();
        config.live_transcript.backend = "apple-speech".into();

        assert!(!parakeet_warmup_selected(&config));
    }

    #[test]
    fn sidecar_status_is_honest_when_forced_on_without_exact_transport() {
        let mut config = Config::default();
        config.transcription.parakeet_sidecar_enabled = Some(true);

        let label = sidecar_state_label(&config, false, true);
        assert!(label.contains("unavailable (forced on)"));
        if crate::pipeline::parakeet_private_audio_transport_supported() {
            assert!(label.contains("secure direct subprocess"));
        } else {
            assert!(label.contains("transcription uses Whisper"));
        }
        assert!(!label.contains("warm server"));
    }

    #[test]
    #[cfg(all(target_os = "linux", feature = "parakeet"))]
    fn linux_pathname_transport_failure_keeps_whisper_active_without_setup_advice() {
        crate::pipeline::with_private_audio_processing_available_for_test(false, || {
            let mut config = Config::default();
            config.transcription.engine = "parakeet".into();

            let status = super::parakeet_backend_status(&config);
            assert!(!status.ready);
            assert!(status
                .issues
                .iter()
                .any(|issue| issue.contains("pathname-only CLI")));

            let health = super::parakeet_health_item(&config);
            assert_eq!(health.state, "attention");
            assert!(health.detail.contains("Whisper remains active"));
            assert!(!health.detail.contains("Run `minutes setup"));

            assert!(matches!(
                super::warmup_active_backend(&config),
                Err(crate::error::TranscribeError::EngineNotAvailable(reason))
                    if reason == "parakeet-private-audio-transport"
            ));
        });
    }

    #[test]
    fn capability_layers_keep_runtime_unavailability_distinct_from_transport() {
        let capability = crate::pipeline::ParakeetCapability::from_layers(true, true, true, false);
        assert!(!capability.selectable);
        assert!(capability.unavailable_reason().contains("at runtime"));
    }

    #[cfg(feature = "parakeet")]
    #[test]
    fn benchmark_rejects_unavailable_transport_before_launching_helpers() {
        let error = super::benchmark_parakeet(
            std::path::Path::new("missing-helper"),
            "missing-parakeet",
            std::path::Path::new("missing-model"),
            std::path::Path::new("missing-audio"),
            std::path::Path::new("missing-vocab"),
            "tdt-ctc-110m",
            false,
            None,
            0.5,
            &Config::default(),
        )
        .expect_err("benchmark must fail at the shared capability gate");

        assert!(error.contains("cannot receive secure private audio"));
    }

    #[test]
    #[cfg(windows)]
    fn windows_status_never_claims_parakeet_ready_without_secure_transport() {
        let mut config = Config::default();
        config.transcription.parakeet_sidecar_enabled = Some(true);
        let status = super::parakeet_backend_status(&config);
        assert!(!status.ready);
        assert!(!status.sidecar.contains("secure direct subprocess"));
        assert!(status
            .issues
            .iter()
            .any(|issue| issue.contains("pathname-only CLI")
                && issue.contains("transcription uses Whisper")));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_status_discloses_proof_bound_parakeet_fallback() {
        let mut config = Config::default();
        config.transcription.parakeet_sidecar_enabled = Some(true);
        let status = super::parakeet_backend_status(&config);
        assert!(!status.ready);
        assert!(status.sidecar.contains("transcription uses Whisper"));
        assert!(!status.sidecar.contains("secure direct subprocess"));
        assert!(status.issues.iter().any(|issue| {
            issue.contains("pathname-only CLI") && issue.contains("transcription uses Whisper")
        }));
        assert!(status.issues.iter().any(|issue| {
            issue.contains("warm sidecar requested") && issue.contains("using Whisper")
        }));

        let health = super::parakeet_health_item(&config);
        assert_eq!(health.state, "attention");
        assert!(health.detail.contains("Whisper remains active"));
        assert!(!health.detail.contains("Run `minutes setup"));

        config.transcription.engine = "parakeet".into();
        assert!(matches!(
            super::warmup_active_backend(&config),
            Err(crate::error::TranscribeError::EngineNotAvailable(reason))
                if reason == "parakeet-private-audio-transport"
        ));
    }
}
