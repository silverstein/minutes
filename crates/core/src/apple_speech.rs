use crate::config::Config;
use crate::error::{MinutesError, Result, TranscribeError};
use crate::pipeline::{clean_transcript_line, normalize_space};
use crate::{markdown::ContentType, transcribe};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::time::Duration;
use std::time::Instant;

#[cfg(target_os = "macos")]
use crate::calendar::output_with_timeout;
#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(target_os = "macos")]
const HELPER_SOURCE: &str = include_str!("../resources/apple-speech-helper.swift");
#[cfg(target_os = "macos")]
const HELPER_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(target_os = "macos")]
const HELPER_TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(900);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechModuleCapability {
    pub module_id: String,
    pub is_available: Option<bool>,
    pub asset_status: String,
    pub supported_locales: Vec<String>,
    pub installed_locales: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechCapabilityReport {
    pub kind: String,
    pub schema_version: u32,
    pub os_version: String,
    pub runtime_supported: bool,
    pub read_only: bool,
    pub speech_transcriber: AppleSpeechModuleCapability,
    pub dictation_transcriber: AppleSpeechModuleCapability,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechTranscriptSegment {
    pub start_ms: u64,
    pub duration_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechTranscriptionResult {
    pub kind: String,
    pub schema_version: u32,
    pub module_id: String,
    pub locale: String,
    pub ensure_assets: bool,
    pub os_version: String,
    pub runtime_supported: bool,
    pub asset_status_before: String,
    pub asset_status_after: String,
    pub total_elapsed_ms: u64,
    pub first_result_elapsed_ms: Option<u64>,
    pub transcript: String,
    pub word_count: usize,
    pub segments: Vec<AppleSpeechTranscriptSegment>,
    pub notes: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppleSpeechMode {
    Speech,
    Dictation,
}

impl AppleSpeechMode {
    #[cfg(target_os = "macos")]
    fn as_helper_arg(self) -> &'static str {
        match self {
            Self::Speech => "speech",
            Self::Dictation => "dictation",
        }
    }

    fn backend_id(self) -> &'static str {
        match self {
            Self::Speech => "apple-speech-transcriber",
            Self::Dictation => "apple-dictation-transcriber",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechBenchmarkCase {
    pub id: String,
    pub audio_path: PathBuf,
    #[serde(default = "default_eval_content_type")]
    pub content_type: ContentType,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub reference_text: String,
    #[serde(default)]
    pub reference_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechBackendBenchmark {
    pub backend_id: String,
    pub status: String,
    pub cold_elapsed_ms: Option<u64>,
    pub warm_elapsed_ms: Option<u64>,
    pub total_elapsed_ms: Option<u64>,
    pub first_result_elapsed_ms: Option<u64>,
    pub word_count: usize,
    pub transcript: String,
    pub segment_count: usize,
    pub has_timestamps: bool,
    pub wer: Option<f64>,
    pub wer_punct_insensitive: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechBenchmarkCaseResult {
    pub id: String,
    pub audio_path: PathBuf,
    pub content_type: ContentType,
    pub locale: String,
    pub reference_available: bool,
    pub speech_transcriber: AppleSpeechBackendBenchmark,
    pub dictation_transcriber: AppleSpeechBackendBenchmark,
    pub whisper: AppleSpeechBackendBenchmark,
    pub parakeet: AppleSpeechBackendBenchmark,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechAggregateMetrics {
    pub cases_total: usize,
    pub cases_succeeded: usize,
    pub cases_with_reference: usize,
    pub average_elapsed_ms: Option<f64>,
    pub average_first_result_elapsed_ms: Option<f64>,
    pub average_wer: Option<f64>,
    pub average_wer_punct_insensitive: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechBenchmarkTotals {
    pub speech_transcriber: AppleSpeechAggregateMetrics,
    pub dictation_transcriber: AppleSpeechAggregateMetrics,
    pub whisper: AppleSpeechAggregateMetrics,
    pub parakeet: AppleSpeechAggregateMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechBenchmarkSlices {
    pub overall: AppleSpeechBenchmarkTotals,
    pub meeting: AppleSpeechBenchmarkTotals,
    pub dictation: AppleSpeechBenchmarkTotals,
    pub memo: AppleSpeechBenchmarkTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechBenchmarkReport {
    pub generated_at: String,
    pub corpus_path: PathBuf,
    pub configured_engine: String,
    pub capabilities: AppleSpeechCapabilityReport,
    pub cases: Vec<AppleSpeechBenchmarkCaseResult>,
    pub totals: AppleSpeechBenchmarkTotals,
    pub slices: AppleSpeechBenchmarkSlices,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechBenchmarkRequest {
    pub command: String,
    pub generated_at: String,
    pub corpus_path: PathBuf,
    pub output_root: PathBuf,
    pub configured_engine: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppleSpeechBenchmarkArtifactPaths {
    pub run_dir: PathBuf,
    pub request_json: PathBuf,
    pub results_json: PathBuf,
    pub summary_md: PathBuf,
}

pub fn default_research_root() -> PathBuf {
    Config::minutes_dir().join("research").join("apple-speech")
}

pub fn live_locale_hint(language: Option<&str>) -> Option<String> {
    let language = language?;
    let trimmed = language.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.replace('-', "_"))
}

pub fn probe_capabilities() -> Result<AppleSpeechCapabilityReport> {
    #[cfg(target_os = "macos")]
    {
        let helper = ensure_helper_installed()?;
        run_helper_capabilities(&helper)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(AppleSpeechCapabilityReport {
            kind: "capabilities".into(),
            schema_version: 1,
            os_version: std::env::consts::OS.into(),
            runtime_supported: false,
            read_only: true,
            speech_transcriber: AppleSpeechModuleCapability {
                module_id: "speech-transcriber".into(),
                is_available: None,
                asset_status: "unsupported".into(),
                supported_locales: Vec::new(),
                installed_locales: Vec::new(),
            },
            dictation_transcriber: AppleSpeechModuleCapability {
                module_id: "dictation-transcriber".into(),
                is_available: None,
                asset_status: "unsupported".into(),
                supported_locales: Vec::new(),
                installed_locales: Vec::new(),
            },
            notes: vec!["Apple Speech evaluation is only available on macOS.".into()],
        })
    }
}

#[cfg(target_os = "macos")]
pub fn transcribe_with_apple_speech(
    audio_path: &Path,
    locale: Option<&str>,
    mode: AppleSpeechMode,
    ensure_assets: bool,
) -> Result<AppleSpeechTranscriptionResult> {
    let helper = ensure_helper_installed()?;
    run_helper_transcription(&helper, audio_path, locale, mode, ensure_assets)
}

#[cfg(not(target_os = "macos"))]
pub fn transcribe_with_apple_speech(
    _audio_path: &Path,
    locale: Option<&str>,
    mode: AppleSpeechMode,
    ensure_assets: bool,
) -> Result<AppleSpeechTranscriptionResult> {
    Ok(AppleSpeechTranscriptionResult {
        kind: "transcription".into(),
        schema_version: 1,
        module_id: mode.backend_id().into(),
        locale: locale.unwrap_or("en-US").into(),
        ensure_assets,
        os_version: std::env::consts::OS.into(),
        runtime_supported: false,
        asset_status_before: "unsupported".into(),
        asset_status_after: "unsupported".into(),
        total_elapsed_ms: 0,
        first_result_elapsed_ms: None,
        transcript: String::new(),
        word_count: 0,
        segments: Vec::new(),
        notes: vec!["Apple Speech evaluation is only available on macOS.".into()],
        error: Some("unsupported platform".into()),
    })
}

pub fn run_benchmark_corpus(
    corpus_path: &Path,
    config: &Config,
) -> Result<AppleSpeechBenchmarkReport> {
    let cases = load_benchmark_cases(corpus_path)?;
    if cases.is_empty() {
        return Err(MinutesError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "apple-speech benchmark corpus is empty".to_string(),
        )));
    }

    let capabilities = probe_capabilities()?;
    let mut results = Vec::new();
    for case in &cases {
        results.push(run_benchmark_case(case, config)?);
    }
    let notes = vec![
        "Whisper is always benchmarked explicitly as the cross-platform baseline.".into(),
        "Parakeet is benchmarked explicitly when compiled/configured; failures are recorded instead of skipped.".into(),
        "Apple timings are measured by invoking the helper twice per mode and recording the second run as warm.".into(),
        "Current Minutes dictation is a streaming UI path; this benchmark uses file-based backends as a comparable proxy, not a live hotkey benchmark.".into(),
    ];

    Ok(AppleSpeechBenchmarkReport {
        generated_at: Utc::now().to_rfc3339(),
        corpus_path: corpus_path.to_path_buf(),
        configured_engine: config.transcription.engine.clone(),
        capabilities,
        totals: totals_for_cases(&results),
        slices: AppleSpeechBenchmarkSlices {
            overall: totals_for_cases(&results),
            meeting: totals_for_cases(
                &results
                    .iter()
                    .filter(|case| case.content_type == ContentType::Meeting)
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            dictation: totals_for_cases(
                &results
                    .iter()
                    .filter(|case| case.content_type == ContentType::Dictation)
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            memo: totals_for_cases(
                &results
                    .iter()
                    .filter(|case| case.content_type == ContentType::Memo)
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
        },
        cases: results,
        notes,
    })
}

fn load_benchmark_cases(corpus_path: &Path) -> Result<Vec<AppleSpeechBenchmarkCase>> {
    let raw = fs::read_to_string(corpus_path)?;
    let mut cases: Vec<AppleSpeechBenchmarkCase> = serde_json::from_str(&raw).map_err(|error| {
        MinutesError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        ))
    })?;
    normalize_benchmark_case_paths(&mut cases, corpus_path);
    Ok(cases)
}

fn normalize_benchmark_case_paths(cases: &mut [AppleSpeechBenchmarkCase], corpus_path: &Path) {
    let Some(corpus_dir) = corpus_path.parent() else {
        return;
    };
    for case in cases {
        case.audio_path = resolve_corpus_relative_path(corpus_dir, &case.audio_path);
        if let Some(reference_path) = case.reference_path.as_mut() {
            *reference_path = resolve_corpus_relative_path(corpus_dir, reference_path);
        }
    }
}

fn resolve_corpus_relative_path(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

pub fn write_benchmark_artifacts(
    request: &AppleSpeechBenchmarkRequest,
    report: &AppleSpeechBenchmarkReport,
) -> Result<AppleSpeechBenchmarkArtifactPaths> {
    let run_dir = request
        .output_root
        .join(Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string());
    fs::create_dir_all(&run_dir)?;

    let request_json = run_dir.join("request.json");
    let results_json = run_dir.join("results.json");
    let summary_md = run_dir.join("summary.md");

    fs::write(
        &request_json,
        serde_json::to_string_pretty(request).map_err(|error| {
            MinutesError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            ))
        })?,
    )?;
    fs::write(
        &results_json,
        serde_json::to_string_pretty(report).map_err(|error| {
            MinutesError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            ))
        })?,
    )?;
    fs::write(&summary_md, render_benchmark_summary(report))?;

    Ok(AppleSpeechBenchmarkArtifactPaths {
        run_dir,
        request_json,
        results_json,
        summary_md,
    })
}

pub fn render_benchmark_summary(report: &AppleSpeechBenchmarkReport) -> String {
    let mut lines = Vec::new();
    lines.push("# Apple Speech Benchmark Summary".to_string());
    lines.push(String::new());
    lines.push(format!("- Generated at: `{}`", report.generated_at));
    lines.push(format!("- Corpus: `{}`", report.corpus_path.display()));
    lines.push(format!(
        "- Configured Minutes engine during run: `{}`",
        report.configured_engine
    ));
    lines.push(String::new());
    lines.push("## Capability snapshot".to_string());
    lines.push(String::new());
    lines.push(format!(
        "- SpeechTranscriber available: `{}`",
        report
            .capabilities
            .speech_transcriber
            .is_available
            .map(|value| value.to_string())
            .unwrap_or_else(|| "n/a".into())
    ));
    lines.push(format!(
        "- SpeechTranscriber asset status: `{}`",
        report.capabilities.speech_transcriber.asset_status
    ));
    lines.push(format!(
        "- DictationTranscriber asset status: `{}`",
        report.capabilities.dictation_transcriber.asset_status
    ));
    if !report.capabilities.notes.is_empty() {
        lines.push(format!(
            "- Capability notes: {}",
            report.capabilities.notes.join(" | ")
        ));
    }
    lines.push(String::new());
    lines.push("## Overall metrics".to_string());
    lines.push(String::new());
    for (label, metrics) in [
        ("SpeechTranscriber", &report.totals.speech_transcriber),
        ("DictationTranscriber", &report.totals.dictation_transcriber),
        ("Whisper", &report.totals.whisper),
        ("Parakeet", &report.totals.parakeet),
    ] {
        lines.push(format!(
            "- {}: succeeded `{}/{}`; avg elapsed `{}` ms; avg first-result `{}` ms; avg WER `{}`; avg WER (punct-insensitive) `{}`",
            label,
            metrics.cases_succeeded,
            metrics.cases_total,
            format_optional_f64(metrics.average_elapsed_ms),
            format_optional_f64(metrics.average_first_result_elapsed_ms),
            format_optional_f64(metrics.average_wer.map(|value| value * 100.0)),
            format_optional_f64(
                metrics
                    .average_wer_punct_insensitive
                    .map(|value| value * 100.0)
            ),
        ));
    }
    for (slice_label, totals) in [
        ("meeting", &report.slices.meeting),
        ("dictation", &report.slices.dictation),
        ("memo", &report.slices.memo),
    ] {
        lines.push(String::new());
        lines.push(format!("## {} metrics", slice_label));
        lines.push(String::new());
        for (label, metrics) in [
            ("SpeechTranscriber", &totals.speech_transcriber),
            ("DictationTranscriber", &totals.dictation_transcriber),
            ("Whisper", &totals.whisper),
            ("Parakeet", &totals.parakeet),
        ] {
            lines.push(format!(
                "- {}: succeeded `{}/{}`; avg elapsed `{}` ms; avg first-result `{}` ms; avg WER `{}`; avg WER (punct-insensitive) `{}`",
                label,
                metrics.cases_succeeded,
                metrics.cases_total,
                format_optional_f64(metrics.average_elapsed_ms),
                format_optional_f64(metrics.average_first_result_elapsed_ms),
                format_optional_f64(metrics.average_wer.map(|value| value * 100.0)),
                format_optional_f64(
                    metrics
                        .average_wer_punct_insensitive
                        .map(|value| value * 100.0)
                ),
            ));
        }
    }
    lines.push(String::new());
    lines.push("## Cases".to_string());
    lines.push(String::new());
    for case in &report.cases {
        lines.push(format!(
            "- `{}` [{} {}]",
            case.id,
            content_type_label(case.content_type),
            case.locale
        ));
        lines.push(format!(
            "  speech: {} / {} ms / WER {} / WER no-punct {}",
            case.speech_transcriber.status,
            case.speech_transcriber
                .total_elapsed_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".into()),
            format_optional_f64(case.speech_transcriber.wer.map(|value| value * 100.0)),
            format_optional_f64(
                case.speech_transcriber
                    .wer_punct_insensitive
                    .map(|value| value * 100.0)
            ),
        ));
        lines.push(format!(
            "  dictation: {} / {} ms / WER {} / WER no-punct {}",
            case.dictation_transcriber.status,
            case.dictation_transcriber
                .total_elapsed_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".into()),
            format_optional_f64(case.dictation_transcriber.wer.map(|value| value * 100.0)),
            format_optional_f64(
                case.dictation_transcriber
                    .wer_punct_insensitive
                    .map(|value| value * 100.0)
            ),
        ));
        lines.push(format!(
            "  whisper: {} / {} ms / WER {} / WER no-punct {}",
            case.whisper.status,
            case.whisper
                .total_elapsed_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".into()),
            format_optional_f64(case.whisper.wer.map(|value| value * 100.0)),
            format_optional_f64(
                case.whisper
                    .wer_punct_insensitive
                    .map(|value| value * 100.0)
            ),
        ));
        lines.push(format!(
            "  parakeet: {} / {} ms / WER {} / WER no-punct {}",
            case.parakeet.status,
            case.parakeet
                .total_elapsed_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".into()),
            format_optional_f64(case.parakeet.wer.map(|value| value * 100.0)),
            format_optional_f64(
                case.parakeet
                    .wer_punct_insensitive
                    .map(|value| value * 100.0)
            ),
        ));
    }
    lines.push(String::new());
    lines.push("## Notes".to_string());
    lines.push(String::new());
    for note in &report.notes {
        lines.push(format!("- {}", note));
    }
    lines.join("\n")
}

fn default_eval_content_type() -> ContentType {
    ContentType::Meeting
}

fn aggregate_metrics(
    cases: &[AppleSpeechBenchmarkCaseResult],
    select: impl Fn(&AppleSpeechBenchmarkCaseResult) -> &AppleSpeechBackendBenchmark,
) -> AppleSpeechAggregateMetrics {
    let mut metrics = AppleSpeechAggregateMetrics {
        cases_total: cases.len(),
        ..Default::default()
    };

    let mut elapsed_sum = 0f64;
    let mut elapsed_count = 0usize;
    let mut first_sum = 0f64;
    let mut first_count = 0usize;
    let mut wer_sum = 0f64;
    let mut wer_count = 0usize;
    let mut wer_punct_insensitive_sum = 0f64;
    let mut wer_punct_insensitive_count = 0usize;

    for case in cases {
        let result = select(case);
        if result.status == "ok" {
            metrics.cases_succeeded += 1;
            if case.reference_available {
                metrics.cases_with_reference += 1;
            }
            if let Some(elapsed) = result.total_elapsed_ms {
                elapsed_sum += elapsed as f64;
                elapsed_count += 1;
            }
            if let Some(first) = result.first_result_elapsed_ms {
                first_sum += first as f64;
                first_count += 1;
            }
            if let Some(wer) = result.wer {
                wer_sum += wer;
                wer_count += 1;
            }
            if let Some(wer) = result.wer_punct_insensitive {
                wer_punct_insensitive_sum += wer;
                wer_punct_insensitive_count += 1;
            }
        }
    }

    metrics.average_elapsed_ms = average(elapsed_sum, elapsed_count);
    metrics.average_first_result_elapsed_ms = average(first_sum, first_count);
    metrics.average_wer = average(wer_sum, wer_count);
    metrics.average_wer_punct_insensitive =
        average(wer_punct_insensitive_sum, wer_punct_insensitive_count);
    metrics
}

fn totals_for_cases(cases: &[AppleSpeechBenchmarkCaseResult]) -> AppleSpeechBenchmarkTotals {
    AppleSpeechBenchmarkTotals {
        speech_transcriber: aggregate_metrics(cases, |case| &case.speech_transcriber),
        dictation_transcriber: aggregate_metrics(cases, |case| &case.dictation_transcriber),
        whisper: aggregate_metrics(cases, |case| &case.whisper),
        parakeet: aggregate_metrics(cases, |case| &case.parakeet),
    }
}

fn average(sum: f64, count: usize) -> Option<f64> {
    if count == 0 {
        None
    } else {
        Some(sum / count as f64)
    }
}

fn format_optional_f64(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "n/a".into())
}

fn run_benchmark_case(
    case: &AppleSpeechBenchmarkCase,
    config: &Config,
) -> Result<AppleSpeechBenchmarkCaseResult> {
    let locale = case.locale.clone().unwrap_or_else(|| "en-US".into());
    let reference = load_reference_text(case)?;
    let reference_available = reference.is_some();

    let speech_transcriber =
        benchmark_apple_mode(case, &locale, AppleSpeechMode::Speech, reference.as_deref())?;
    let dictation_transcriber = benchmark_apple_mode(
        case,
        &locale,
        AppleSpeechMode::Dictation,
        reference.as_deref(),
    )?;
    let whisper = benchmark_minutes_backend(case, &locale, "whisper", config, reference.as_deref());
    let parakeet =
        benchmark_minutes_backend(case, &locale, "parakeet", config, reference.as_deref());

    Ok(AppleSpeechBenchmarkCaseResult {
        id: case.id.clone(),
        audio_path: case.audio_path.clone(),
        content_type: case.content_type,
        locale,
        reference_available,
        speech_transcriber,
        dictation_transcriber,
        whisper,
        parakeet,
    })
}

fn benchmark_apple_mode(
    case: &AppleSpeechBenchmarkCase,
    locale: &str,
    mode: AppleSpeechMode,
    reference_text: Option<&str>,
) -> Result<AppleSpeechBackendBenchmark> {
    let cold = transcribe_with_apple_speech(&case.audio_path, Some(locale), mode, true)?;
    let warm = transcribe_with_apple_speech(&case.audio_path, Some(locale), mode, true)?;
    let selected = if warm.error.is_none() { &warm } else { &cold };
    Ok(AppleSpeechBackendBenchmark {
        backend_id: mode.backend_id().into(),
        status: if selected.error.is_none() {
            "ok".into()
        } else if selected.runtime_supported {
            "error".into()
        } else {
            "unsupported".into()
        },
        cold_elapsed_ms: Some(cold.total_elapsed_ms),
        warm_elapsed_ms: Some(warm.total_elapsed_ms),
        total_elapsed_ms: Some(selected.total_elapsed_ms),
        first_result_elapsed_ms: selected.first_result_elapsed_ms,
        word_count: selected.word_count,
        transcript: selected.transcript.clone(),
        segment_count: selected.segments.len(),
        has_timestamps: selected
            .segments
            .iter()
            .any(|segment| segment.start_ms > 0 || segment.duration_ms > 0),
        wer: reference_text.map(|reference| word_error_rate(reference, &selected.transcript)),
        wer_punct_insensitive: reference_text
            .map(|reference| word_error_rate_punct_insensitive(reference, &selected.transcript)),
        error: selected.error.clone(),
    })
}

fn benchmark_minutes_backend(
    case: &AppleSpeechBenchmarkCase,
    locale: &str,
    engine: &str,
    config: &Config,
    reference_text: Option<&str>,
) -> AppleSpeechBackendBenchmark {
    let mut config = config.clone();
    config.transcription.engine = engine.into();
    config.transcription.language = locale_language_hint(locale);

    let started = Instant::now();
    let result = match case.content_type {
        ContentType::Meeting => transcribe::transcribe_meeting(&case.audio_path, &config),
        _ => transcribe::transcribe(&case.audio_path, &config),
    };

    match result {
        Ok(result) => AppleSpeechBackendBenchmark {
            backend_id: engine.into(),
            status: "ok".into(),
            cold_elapsed_ms: None,
            warm_elapsed_ms: None,
            total_elapsed_ms: Some(started.elapsed().as_millis() as u64),
            first_result_elapsed_ms: None,
            word_count: result.stats.final_words,
            transcript: result.text.clone(),
            segment_count: result.text.lines().count(),
            has_timestamps: result.text.lines().any(|line| line.starts_with('[')),
            wer: reference_text.map(|reference| word_error_rate(reference, &result.text)),
            wer_punct_insensitive: reference_text
                .map(|reference| word_error_rate_punct_insensitive(reference, &result.text)),
            error: None,
        },
        Err(error) => {
            let is_parakeet_unavailable = engine == "parakeet"
                && matches!(
                    &error,
                    TranscribeError::EngineNotAvailable(_) | TranscribeError::ParakeetFailed(_)
                );
            AppleSpeechBackendBenchmark {
                backend_id: engine.into(),
                status: if is_parakeet_unavailable {
                    "unsupported".into()
                } else {
                    "error".into()
                },
                cold_elapsed_ms: None,
                warm_elapsed_ms: None,
                total_elapsed_ms: None,
                first_result_elapsed_ms: None,
                word_count: 0,
                transcript: String::new(),
                segment_count: 0,
                has_timestamps: false,
                wer: None,
                wer_punct_insensitive: None,
                error: Some(error.to_string()),
            }
        }
    }
}

fn load_reference_text(case: &AppleSpeechBenchmarkCase) -> Result<Option<String>> {
    if !case.reference_text.trim().is_empty() {
        return Ok(Some(case.reference_text.clone()));
    }
    let Some(path) = &case.reference_path else {
        return Ok(None);
    };
    Ok(Some(fs::read_to_string(path)?))
}

fn eval_text_for_compare(text: &str) -> String {
    text.lines()
        .filter_map(clean_transcript_line)
        .map(|line| normalize_space(&line).to_lowercase())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn eval_text_for_compare_punct_insensitive(text: &str) -> String {
    eval_text_for_compare(text)
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch.is_whitespace() {
                ch
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn word_error_rate(reference: &str, hypothesis: &str) -> f64 {
    let reference = eval_text_for_compare(reference);
    let hypothesis = eval_text_for_compare(hypothesis);
    let reference_words: Vec<&str> = reference.split_whitespace().collect();
    let hypothesis_words: Vec<&str> = hypothesis.split_whitespace().collect();
    if reference_words.is_empty() {
        return if hypothesis_words.is_empty() {
            0.0
        } else {
            1.0
        };
    }

    let mut dp = vec![vec![0usize; hypothesis_words.len() + 1]; reference_words.len() + 1];
    for (i, row) in dp.iter_mut().enumerate().take(reference_words.len() + 1) {
        row[0] = i;
    }
    for (j, cell) in dp[0]
        .iter_mut()
        .enumerate()
        .take(hypothesis_words.len() + 1)
    {
        *cell = j;
    }
    for i in 1..=reference_words.len() {
        for j in 1..=hypothesis_words.len() {
            let cost = usize::from(reference_words[i - 1] != hypothesis_words[j - 1]);
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }

    dp[reference_words.len()][hypothesis_words.len()] as f64 / reference_words.len() as f64
}

fn word_error_rate_punct_insensitive(reference: &str, hypothesis: &str) -> f64 {
    let reference = eval_text_for_compare_punct_insensitive(reference);
    let hypothesis = eval_text_for_compare_punct_insensitive(hypothesis);
    let reference_words: Vec<&str> = reference.split_whitespace().collect();
    let hypothesis_words: Vec<&str> = hypothesis.split_whitespace().collect();
    if reference_words.is_empty() {
        return if hypothesis_words.is_empty() {
            0.0
        } else {
            1.0
        };
    }

    let mut dp = vec![vec![0usize; hypothesis_words.len() + 1]; reference_words.len() + 1];
    for (i, row) in dp.iter_mut().enumerate().take(reference_words.len() + 1) {
        row[0] = i;
    }
    for (j, cell) in dp[0]
        .iter_mut()
        .enumerate()
        .take(hypothesis_words.len() + 1)
    {
        *cell = j;
    }
    for i in 1..=reference_words.len() {
        for j in 1..=hypothesis_words.len() {
            let cost = usize::from(reference_words[i - 1] != hypothesis_words[j - 1]);
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }

    dp[reference_words.len()][hypothesis_words.len()] as f64 / reference_words.len() as f64
}

#[cfg(target_os = "macos")]
fn run_helper_capabilities(helper: &Path) -> Result<AppleSpeechCapabilityReport> {
    let mut command = Command::new(helper);
    command.arg("capabilities");
    let output = output_with_timeout(command, HELPER_TIMEOUT).ok_or_else(|| {
        MinutesError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "apple speech helper capabilities timed out",
        ))
    })?;
    if !output.status.success() {
        return Err(MinutesError::Io(std::io::Error::other(format!(
            "apple speech helper capabilities failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        MinutesError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        ))
    })
}

#[cfg(target_os = "macos")]
fn run_helper_transcription(
    helper: &Path,
    audio_path: &Path,
    locale: Option<&str>,
    mode: AppleSpeechMode,
    ensure_assets: bool,
) -> Result<AppleSpeechTranscriptionResult> {
    let mut command = Command::new(helper);
    command
        .arg("transcribe")
        .args(["--mode", mode.as_helper_arg()])
        .args([
            "--audio-path",
            audio_path.to_str().ok_or_else(|| {
                MinutesError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "audio path is not valid UTF-8",
                ))
            })?,
        ]);
    if let Some(locale) = locale {
        command.args(["--locale", locale]);
    }
    if ensure_assets {
        command.arg("--ensure-assets");
    }

    let output = output_with_timeout(command, HELPER_TRANSCRIBE_TIMEOUT).ok_or_else(|| {
        MinutesError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "apple speech helper transcription timed out",
        ))
    })?;
    let parsed: AppleSpeechTranscriptionResult =
        serde_json::from_slice(&output.stdout).map_err(|error| {
            MinutesError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            ))
        })?;
    if !output.status.success() && parsed.error.is_none() {
        return Err(MinutesError::Io(std::io::Error::other(format!(
            "apple speech helper failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))));
    }
    Ok(parsed)
}

#[cfg(target_os = "macos")]
fn ensure_helper_installed() -> Result<PathBuf> {
    let bin_path = Config::minutes_dir()
        .join("bin")
        .join("apple-speech-helper");
    let source_path = Config::minutes_dir()
        .join("lib")
        .join("apple-speech-helper.swift");

    if let Some(parent) = source_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = bin_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let needs_source_write = match fs::read_to_string(&source_path) {
        Ok(existing) => existing != HELPER_SOURCE,
        Err(_) => true,
    };
    if needs_source_write {
        fs::write(&source_path, HELPER_SOURCE)?;
    }

    let needs_compile = match (fs::metadata(&source_path), fs::metadata(&bin_path)) {
        (_, Err(_)) => true,
        (Ok(source_meta), Ok(bin_meta)) => source_meta.modified().ok() > bin_meta.modified().ok(),
        _ => true,
    };
    if needs_compile {
        compile_helper(&source_path, &bin_path)?;
    }

    Ok(bin_path)
}

#[cfg(target_os = "macos")]
fn compile_helper(source_path: &Path, bin_path: &Path) -> Result<()> {
    let output = Command::new("xcrun")
        .arg("swiftc")
        .arg("-parse-as-library")
        .arg("-O")
        .arg(source_path)
        .arg("-o")
        .arg(bin_path)
        .output()
        .or_else(|_| {
            Command::new("swiftc")
                .arg("-parse-as-library")
                .arg("-O")
                .arg(source_path)
                .arg("-o")
                .arg(bin_path)
                .output()
        })?;

    if !output.status.success() {
        return Err(MinutesError::Io(std::io::Error::other(format!(
            "failed to compile apple speech helper: {}",
            String::from_utf8_lossy(&output.stderr)
        ))));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(bin_path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn content_type_label(content_type: ContentType) -> &'static str {
    match content_type {
        ContentType::Meeting => "meeting",
        ContentType::Memo => "memo",
        ContentType::Dictation => "dictation",
    }
}

fn locale_language_hint(locale: &str) -> Option<String> {
    let trimmed = locale.trim();
    if trimmed.is_empty() {
        return None;
    }
    let primary = trimmed.split(['_', '-']).next().unwrap_or(trimmed).trim();
    if primary.is_empty() {
        None
    } else {
        Some(primary.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn locale_language_hint_uses_primary_subtag() {
        assert_eq!(locale_language_hint("en_US"), Some("en".into()));
        assert_eq!(locale_language_hint("pt-BR"), Some("pt".into()));
        assert_eq!(locale_language_hint(""), None);
    }

    #[test]
    fn live_locale_hint_preserves_plain_language_codes() {
        assert_eq!(live_locale_hint(Some("en")), Some("en".into()));
        assert_eq!(live_locale_hint(Some(" fr ")), Some("fr".into()));
        assert_eq!(live_locale_hint(Some("pt-BR")), Some("pt_BR".into()));
        assert_eq!(live_locale_hint(Some("")), None);
        assert_eq!(live_locale_hint(None), None);
    }

    #[test]
    fn word_error_rate_normalizes_timestamped_minutes_output() {
        let reference = "Matt and Wesley are reviewing the Minutes Apple speech benchmark.";
        let hypothesis =
            "[0:00] Matt and Wesley are reviewing the Minute's Apple Speech Benchmark.\n";
        let wer = word_error_rate(reference, hypothesis);
        assert!(wer >= 0.0);
        assert!(wer < 0.34);
    }

    #[test]
    fn benchmark_case_paths_resolve_relative_to_corpus_file() {
        let dir = tempdir().unwrap();
        let corpus_dir = dir.path().join("fixtures");
        std::fs::create_dir_all(corpus_dir.join("audio")).unwrap();
        std::fs::create_dir_all(corpus_dir.join("refs")).unwrap();
        let absolute_audio = dir.path().join("absolute.wav");

        let corpus_path = corpus_dir.join("apple-speech-corpus.json");
        std::fs::write(
            &corpus_path,
            serde_json::json!([
                {
                    "id": "case-1",
                    "audioPath": "audio/sample.wav",
                    "contentType": "meeting",
                    "referencePath": "refs/sample.txt"
                },
                {
                    "id": "case-2",
                    "audioPath": absolute_audio,
                    "contentType": "dictation"
                }
            ])
            .to_string(),
        )
        .unwrap();

        let cases = load_benchmark_cases(&corpus_path).unwrap();

        assert_eq!(
            cases[0].audio_path,
            corpus_dir.join(Path::new("audio").join("sample.wav"))
        );
        assert_eq!(
            cases[0].reference_path,
            Some(corpus_dir.join(Path::new("refs").join("sample.txt")))
        );
        assert_eq!(cases[1].audio_path, absolute_audio);
    }

    #[test]
    fn punct_insensitive_wer_ignores_terminal_punctuation() {
        let reference = "Minutes benchmark dictation check. Apple speech should handle short form voice notes locally.";
        let hypothesis =
            "Minutes benchmark dictation check Apple speech should handle short form voice notes locally";
        let punct_sensitive = word_error_rate(reference, hypothesis);
        let punct_insensitive = word_error_rate_punct_insensitive(reference, hypothesis);
        assert!(punct_sensitive > punct_insensitive);
        assert_eq!(punct_insensitive, 0.0);
    }
}
