use crate::config::Config;
use crate::error::MinutesError;
use crate::pipeline::{
    build_decode_hints, clean_transcript_line, normalize_space,
    normalize_transcript_for_self_name_participant,
};
use crate::transcribe::{self, DecodeHints};
use crate::{ContentType, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DecodeHintEvalCase {
    pub id: String,
    pub audio_path: PathBuf,
    #[serde(default = "default_eval_content_type")]
    pub content_type: ContentType,
    #[serde(default)]
    pub reference_text: String,
    #[serde(default)]
    pub reference_path: Option<PathBuf>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub calendar_event_title: Option<String>,
    #[serde(default)]
    pub pre_context: Option<String>,
    #[serde(default)]
    pub extra_priority_hints: Vec<String>,
    #[serde(default)]
    pub extra_context_hints: Vec<String>,
    #[serde(default)]
    pub attendees: Vec<String>,
    #[serde(default)]
    pub identity_name: Option<String>,
    #[serde(default)]
    pub identity_aliases: Vec<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub parakeet_boost_score_override: Option<f32>,
    #[serde(default)]
    pub max_wer_regression: Option<f64>,
    #[serde(default)]
    pub require_hinted_terms: Vec<String>,
    #[serde(default)]
    pub forbid_hinted_terms: Vec<String>,
    #[serde(default)]
    pub allowed_failure_substrings: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalOptions {
    #[serde(default)]
    pub engine_override: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalTranscriptMetrics {
    pub wer: f64,
    pub focus_hits: Vec<String>,
    pub forbidden_hits: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalCaseResult {
    pub id: String,
    pub engine: String,
    #[serde(default)]
    pub hint_debug: DecodeHintEvalHintDebug,
    pub baseline: DecodeHintEvalTranscriptMetrics,
    pub candidate: DecodeHintEvalTranscriptMetrics,
    pub delta_wer: f64,
    pub max_wer_regression: Option<f64>,
    pub required_terms: Vec<String>,
    pub forbidden_terms: Vec<String>,
    pub passed: bool,
    #[serde(default = "default_case_status")]
    pub status: String,
    #[serde(default)]
    pub failure_reasons: Vec<String>,
    #[serde(default)]
    pub allowed_failure_reasons: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalHintDebug {
    #[serde(default)]
    pub priority_phrases: Vec<String>,
    #[serde(default)]
    pub contextual_phrases: Vec<String>,
    #[serde(default)]
    pub whisper_prompt_phrases: Vec<String>,
    #[serde(default)]
    pub parakeet_boost_phrases: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalTotals {
    pub cases_total: usize,
    pub cases_passed: usize,
    pub cases_failed: usize,
    #[serde(default)]
    pub cases_allowed_failures: usize,
    pub improved_cases: usize,
    pub regressed_cases: usize,
    pub average_delta_wer: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalReport {
    pub generated_at: String,
    pub corpus_path: PathBuf,
    pub options: DecodeHintEvalOptions,
    pub totals: DecodeHintEvalTotals,
    pub cases: Vec<DecodeHintEvalCaseResult>,
    pub failure_messages: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalRequest {
    pub command: String,
    pub generated_at: String,
    pub corpus_path: PathBuf,
    pub output_root: PathBuf,
    pub git_commit: Option<String>,
    pub options: DecodeHintEvalOptions,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalArtifactPaths {
    pub run_dir: PathBuf,
    pub request_json: PathBuf,
    pub results_json: PathBuf,
    pub baseline_json: PathBuf,
    pub candidate_json: PathBuf,
    pub summary_md: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DecodeHintEvalSidecarReport<'a> {
    generated_at: &'a str,
    corpus_path: &'a Path,
    cases: Vec<DecodeHintEvalSidecarCase>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DecodeHintEvalSidecarCase {
    id: String,
    engine: String,
    wer: f64,
    focus_hits: Vec<String>,
    forbidden_hits: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalComparisonTotals {
    pub shared_cases: usize,
    pub added_cases: usize,
    pub removed_cases: usize,
    pub improved_cases: usize,
    pub regressed_cases: usize,
    pub newly_passing_cases: usize,
    pub newly_failing_cases: usize,
    pub unchanged_cases: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalComparisonCase {
    pub id: String,
    pub status: String,
    pub left_candidate_wer: Option<f64>,
    pub right_candidate_wer: Option<f64>,
    pub candidate_wer_delta: Option<f64>,
    pub left_passed: Option<bool>,
    pub right_passed: Option<bool>,
    pub gained_focus_hits: Vec<String>,
    pub lost_focus_hits: Vec<String>,
    pub newly_missing_terms: Vec<String>,
    pub resolved_failures: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalComparisonReport {
    pub generated_at: String,
    pub left_path: PathBuf,
    pub right_path: PathBuf,
    pub totals: DecodeHintEvalComparisonTotals,
    pub cases: Vec<DecodeHintEvalComparisonCase>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalComparisonRequest {
    pub command: String,
    pub generated_at: String,
    pub left_path: PathBuf,
    pub right_path: PathBuf,
    pub output_root: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalComparisonArtifactPaths {
    pub run_dir: PathBuf,
    pub request_json: PathBuf,
    pub comparison_json: PathBuf,
    pub summary_md: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintRunIndexEntry {
    pub kind: String,
    pub run_dir: PathBuf,
    pub generated_at: String,
    pub status: String,
    pub source_path: PathBuf,
    pub cases_total: usize,
    pub cases_failed: usize,
    pub improved_cases: usize,
    pub regressed_cases: usize,
    pub newly_passing_cases: usize,
    pub newly_failing_cases: usize,
    pub summary_path: PathBuf,
}

pub fn default_research_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".minutes")
        .join("research")
        .join("decode-hints")
}

pub fn default_comparison_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".minutes")
        .join("research")
        .join("decode-hints-comparisons")
}

pub fn list_decode_hint_runs(limit: usize) -> Result<Vec<DecodeHintRunIndexEntry>> {
    let entries = collect_decode_hint_runs(&default_research_root(), &default_comparison_root())?;
    Ok(entries.into_iter().take(limit).collect())
}

pub fn run_decode_hint_eval_corpus(
    corpus_path: &Path,
    options: &DecodeHintEvalOptions,
) -> Result<DecodeHintEvalReport> {
    let raw = fs::read_to_string(corpus_path)?;
    let cases: Vec<DecodeHintEvalCase> = serde_json::from_str(&raw).map_err(invalid_data_error)?;
    if cases.is_empty() {
        return Err(invalid_input("decode-hint eval corpus is empty"));
    }

    let mut results = Vec::new();
    let mut failure_messages = Vec::new();
    let mut delta_sum = 0.0f64;
    let mut improved_cases = 0usize;
    let mut regressed_cases = 0usize;
    let mut allowed_failure_cases = 0usize;

    for case in cases {
        let mut config = Config::default();
        if let Some(language) = &case.language {
            config.transcription.language = Some(language.clone());
        }
        if let Some(engine) = options
            .engine_override
            .as_ref()
            .or(case.engine.as_ref())
            .cloned()
        {
            config.transcription.engine = engine;
        }
        if let Some(boost_score) = case.parakeet_boost_score_override {
            config.transcription.parakeet_boost_score = boost_score;
        }
        config.identity.name = case.identity_name.clone();
        config.identity.aliases = case.identity_aliases.clone();

        let reference = eval_text_for_compare(&load_reference_text(&case)?);
        let hints = build_decode_hints(
            case.title.as_deref(),
            case.calendar_event_title.as_deref(),
            case.pre_context.as_deref(),
            &case.attendees,
            Some(&config.identity),
        )
        .with_additional_candidates(&case.extra_priority_hints, &case.extra_context_hints);
        let hint_debug = DecodeHintEvalHintDebug {
            priority_phrases: hints.debug_priority_phrases(),
            contextual_phrases: hints.debug_contextual_phrases(),
            whisper_prompt_phrases: hints
                .debug_priority_phrases()
                .into_iter()
                .chain(hints.debug_contextual_phrases())
                .take(12)
                .collect(),
            parakeet_boost_phrases: effective_parakeet_boost_phrases(&config, &hints),
        };

        let baseline = transcribe_case(&case, &config, &DecodeHints::default())?;
        let candidate = transcribe_case(&case, &config, &hints)?;

        let baseline_text = eval_text_for_compare(&baseline.text);
        let candidate_text = eval_text_for_compare(&candidate.text);
        let baseline_wer = word_error_rate(&reference, &baseline_text);
        let candidate_wer = word_error_rate(&reference, &candidate_text);
        let delta_wer = candidate_wer - baseline_wer;
        let baseline_focus_hits = present_terms(&baseline_text, &case.require_hinted_terms);
        let candidate_focus_hits = present_terms(&candidate_text, &case.require_hinted_terms);
        let baseline_forbidden_hits = present_terms(&baseline_text, &case.forbid_hinted_terms);
        let candidate_forbidden_hits = present_terms(&candidate_text, &case.forbid_hinted_terms);

        delta_sum += delta_wer;
        if delta_wer < 0.0 {
            improved_cases += 1;
        } else if delta_wer > 0.0 {
            regressed_cases += 1;
        }

        let mut case_failures = Vec::new();
        if let Some(max_regression) = case.max_wer_regression {
            if delta_wer > max_regression {
                case_failures.push(format!(
                    "hinted WER regressed by {:.4} (> {:.4})",
                    delta_wer, max_regression
                ));
            }
        }
        for term in &case.require_hinted_terms {
            if !candidate_focus_hits
                .iter()
                .any(|hit| hit.eq_ignore_ascii_case(term))
            {
                case_failures.push(format!("missing required hinted term '{term}'"));
            }
        }
        for term in &case.forbid_hinted_terms {
            if candidate_forbidden_hits
                .iter()
                .any(|hit| hit.eq_ignore_ascii_case(term))
            {
                case_failures.push(format!("contains forbidden hinted term '{term}'"));
            }
        }

        let (allowed_failure_reasons, blocking_failure_reasons): (Vec<_>, Vec<_>) =
            case_failures.into_iter().partition(|reason| {
                case.allowed_failure_substrings
                    .iter()
                    .any(|allowed| reason.contains(allowed))
            });

        for reason in &blocking_failure_reasons {
            failure_messages.push(format!("{} {reason}", case.id));
        }

        let passed = blocking_failure_reasons.is_empty();
        let status = if passed {
            if allowed_failure_reasons.is_empty() {
                "pass"
            } else {
                allowed_failure_cases += 1;
                "allowed-failure"
            }
        } else {
            "fail"
        };

        results.push(DecodeHintEvalCaseResult {
            id: case.id,
            engine: config.transcription.engine.clone(),
            hint_debug,
            baseline: DecodeHintEvalTranscriptMetrics {
                wer: baseline_wer,
                focus_hits: baseline_focus_hits,
                forbidden_hits: baseline_forbidden_hits,
            },
            candidate: DecodeHintEvalTranscriptMetrics {
                wer: candidate_wer,
                focus_hits: candidate_focus_hits,
                forbidden_hits: candidate_forbidden_hits,
            },
            delta_wer,
            max_wer_regression: case.max_wer_regression,
            required_terms: case.require_hinted_terms,
            forbidden_terms: case.forbid_hinted_terms,
            passed,
            status: status.into(),
            failure_reasons: blocking_failure_reasons,
            allowed_failure_reasons,
        });
    }

    let totals = DecodeHintEvalTotals {
        cases_total: results.len(),
        cases_passed: results.iter().filter(|case| case.passed).count(),
        cases_failed: results.iter().filter(|case| !case.passed).count(),
        cases_allowed_failures: allowed_failure_cases,
        improved_cases,
        regressed_cases,
        average_delta_wer: if results.is_empty() {
            0.0
        } else {
            delta_sum / results.len() as f64
        },
    };

    Ok(DecodeHintEvalReport {
        generated_at: Utc::now().to_rfc3339(),
        corpus_path: corpus_path.to_path_buf(),
        options: options.clone(),
        totals,
        cases: results,
        failure_messages,
    })
}

pub fn write_decode_hint_eval_artifacts(
    request: &DecodeHintEvalRequest,
    report: &DecodeHintEvalReport,
) -> Result<DecodeHintEvalArtifactPaths> {
    let run_dir = request
        .output_root
        .join(Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string());
    fs::create_dir_all(&run_dir)?;

    let request_json = run_dir.join("request.json");
    let results_json = run_dir.join("results.json");
    let baseline_json = run_dir.join("baseline.json");
    let candidate_json = run_dir.join("candidate.json");
    let summary_md = run_dir.join("summary.md");

    fs::write(
        &request_json,
        serde_json::to_string_pretty(request).map_err(invalid_data_error)?,
    )?;
    fs::write(
        &results_json,
        serde_json::to_string_pretty(report).map_err(invalid_data_error)?,
    )?;

    let baseline = DecodeHintEvalSidecarReport {
        generated_at: &report.generated_at,
        corpus_path: &report.corpus_path,
        cases: report
            .cases
            .iter()
            .map(|case| DecodeHintEvalSidecarCase {
                id: case.id.clone(),
                engine: case.engine.clone(),
                wer: case.baseline.wer,
                focus_hits: case.baseline.focus_hits.clone(),
                forbidden_hits: case.baseline.forbidden_hits.clone(),
            })
            .collect(),
    };
    fs::write(
        &baseline_json,
        serde_json::to_string_pretty(&baseline).map_err(invalid_data_error)?,
    )?;

    let candidate = DecodeHintEvalSidecarReport {
        generated_at: &report.generated_at,
        corpus_path: &report.corpus_path,
        cases: report
            .cases
            .iter()
            .map(|case| DecodeHintEvalSidecarCase {
                id: case.id.clone(),
                engine: case.engine.clone(),
                wer: case.candidate.wer,
                focus_hits: case.candidate.focus_hits.clone(),
                forbidden_hits: case.candidate.forbidden_hits.clone(),
            })
            .collect(),
    };
    fs::write(
        &candidate_json,
        serde_json::to_string_pretty(&candidate).map_err(invalid_data_error)?,
    )?;

    fs::write(&summary_md, render_decode_hint_eval_summary(report))?;

    Ok(DecodeHintEvalArtifactPaths {
        run_dir,
        request_json,
        results_json,
        baseline_json,
        candidate_json,
        summary_md,
    })
}

pub fn load_decode_hint_eval_report(path: &Path) -> Result<DecodeHintEvalReport> {
    let resolved = resolve_report_path(path);
    let raw = fs::read_to_string(&resolved)?;
    serde_json::from_str(&raw).map_err(invalid_data_error)
}

pub fn compare_decode_hint_eval_reports(
    left_path: &Path,
    right_path: &Path,
) -> Result<DecodeHintEvalComparisonReport> {
    let left_resolved = resolve_report_path(left_path);
    let right_resolved = resolve_report_path(right_path);
    let left = load_decode_hint_eval_report(&left_resolved)?;
    let right = load_decode_hint_eval_report(&right_resolved)?;

    let left_cases: std::collections::BTreeMap<_, _> = left
        .cases
        .iter()
        .map(|case| (case.id.clone(), case))
        .collect();
    let right_cases: std::collections::BTreeMap<_, _> = right
        .cases
        .iter()
        .map(|case| (case.id.clone(), case))
        .collect();

    let mut ids = std::collections::BTreeSet::new();
    ids.extend(left_cases.keys().cloned());
    ids.extend(right_cases.keys().cloned());

    let mut totals = DecodeHintEvalComparisonTotals::default();
    let mut cases = Vec::new();

    for id in ids {
        let left_case = left_cases.get(&id).copied();
        let right_case = right_cases.get(&id).copied();
        let comparison = match (left_case, right_case) {
            (Some(left_case), Some(right_case)) => {
                totals.shared_cases += 1;
                let delta = right_case.candidate.wer - left_case.candidate.wer;
                if delta < 0.0 {
                    totals.improved_cases += 1;
                } else if delta > 0.0 {
                    totals.regressed_cases += 1;
                } else {
                    totals.unchanged_cases += 1;
                }
                if !left_case.passed && right_case.passed {
                    totals.newly_passing_cases += 1;
                }
                if left_case.passed && !right_case.passed {
                    totals.newly_failing_cases += 1;
                }

                let left_hits: std::collections::BTreeSet<_> =
                    left_case.candidate.focus_hits.iter().cloned().collect();
                let right_hits: std::collections::BTreeSet<_> =
                    right_case.candidate.focus_hits.iter().cloned().collect();
                let left_failures: std::collections::BTreeSet<_> =
                    left_case.failure_reasons.iter().cloned().collect();
                let right_failures: std::collections::BTreeSet<_> =
                    right_case.failure_reasons.iter().cloned().collect();

                DecodeHintEvalComparisonCase {
                    id,
                    status: "shared".into(),
                    left_candidate_wer: Some(left_case.candidate.wer),
                    right_candidate_wer: Some(right_case.candidate.wer),
                    candidate_wer_delta: Some(delta),
                    left_passed: Some(left_case.passed),
                    right_passed: Some(right_case.passed),
                    gained_focus_hits: right_hits.difference(&left_hits).cloned().collect(),
                    lost_focus_hits: left_hits.difference(&right_hits).cloned().collect(),
                    newly_missing_terms: right_failures
                        .difference(&left_failures)
                        .cloned()
                        .collect(),
                    resolved_failures: left_failures.difference(&right_failures).cloned().collect(),
                }
            }
            (None, Some(right_case)) => {
                totals.added_cases += 1;
                DecodeHintEvalComparisonCase {
                    id,
                    status: "added".into(),
                    left_candidate_wer: None,
                    right_candidate_wer: Some(right_case.candidate.wer),
                    candidate_wer_delta: None,
                    left_passed: None,
                    right_passed: Some(right_case.passed),
                    gained_focus_hits: right_case.candidate.focus_hits.clone(),
                    lost_focus_hits: Vec::new(),
                    newly_missing_terms: right_case.failure_reasons.clone(),
                    resolved_failures: Vec::new(),
                }
            }
            (Some(left_case), None) => {
                totals.removed_cases += 1;
                DecodeHintEvalComparisonCase {
                    id,
                    status: "removed".into(),
                    left_candidate_wer: Some(left_case.candidate.wer),
                    right_candidate_wer: None,
                    candidate_wer_delta: None,
                    left_passed: Some(left_case.passed),
                    right_passed: None,
                    gained_focus_hits: Vec::new(),
                    lost_focus_hits: left_case.candidate.focus_hits.clone(),
                    newly_missing_terms: Vec::new(),
                    resolved_failures: left_case.failure_reasons.clone(),
                }
            }
            (None, None) => continue,
        };
        cases.push(comparison);
    }

    Ok(DecodeHintEvalComparisonReport {
        generated_at: Utc::now().to_rfc3339(),
        left_path: left_resolved,
        right_path: right_resolved,
        totals,
        cases,
    })
}

pub fn write_decode_hint_eval_comparison_artifacts(
    request: &DecodeHintEvalComparisonRequest,
    report: &DecodeHintEvalComparisonReport,
) -> Result<DecodeHintEvalComparisonArtifactPaths> {
    let run_dir = request
        .output_root
        .join(Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string());
    fs::create_dir_all(&run_dir)?;

    let request_json = run_dir.join("request.json");
    let comparison_json = run_dir.join("comparison.json");
    let summary_md = run_dir.join("summary.md");

    fs::write(
        &request_json,
        serde_json::to_string_pretty(request).map_err(invalid_data_error)?,
    )?;
    fs::write(
        &comparison_json,
        serde_json::to_string_pretty(report).map_err(invalid_data_error)?,
    )?;
    fs::write(
        &summary_md,
        render_decode_hint_eval_comparison_summary(report),
    )?;

    Ok(DecodeHintEvalComparisonArtifactPaths {
        run_dir,
        request_json,
        comparison_json,
        summary_md,
    })
}

pub fn render_decode_hint_eval_summary(report: &DecodeHintEvalReport) -> String {
    let verdict = if !report.failure_messages.is_empty() {
        "FAIL"
    } else if report.totals.cases_allowed_failures > 0 {
        "PASS WITH ALLOWED FAILURES"
    } else {
        "PASS"
    };
    let mut lines = vec![
        "# Decode Hint Eval Summary".to_string(),
        String::new(),
        format!("- Verdict: **{verdict}**"),
        format!("- Corpus: `{}`", report.corpus_path.display()),
        format!("- Generated at: `{}`", report.generated_at),
        format!("- Cases: {}", report.totals.cases_total),
        format!("- Passed: {}", report.totals.cases_passed),
        format!("- Failed: {}", report.totals.cases_failed),
        format!(
            "- Allowed failures: {}",
            report.totals.cases_allowed_failures
        ),
        format!("- Improved cases: {}", report.totals.improved_cases),
        format!("- Regressed cases: {}", report.totals.regressed_cases),
        format!(
            "- Average candidate-minus-baseline WER delta: `{:.4}`",
            report.totals.average_delta_wer
        ),
        String::new(),
        "## Case results".to_string(),
        String::new(),
    ];

    for case in &report.cases {
        lines.push(format!(
            "- `{}`: {} [{}] (`{:.4}` -> `{:.4}`, delta `{:.4}`)",
            case.id,
            if case.passed { "pass" } else { "fail" },
            case.status,
            case.baseline.wer,
            case.candidate.wer,
            case.delta_wer
        ));
        if !case.failure_reasons.is_empty() {
            lines.push(format!("  reasons: {}", case.failure_reasons.join("; ")));
        }
        if !case.allowed_failure_reasons.is_empty() {
            lines.push(format!(
                "  allowed failures: {}",
                case.allowed_failure_reasons.join("; ")
            ));
        }
    }

    if !report.failure_messages.is_empty() {
        lines.push(String::new());
        lines.push("## Failure messages".to_string());
        lines.push(String::new());
        for failure in &report.failure_messages {
            lines.push(format!("- {failure}"));
        }
    }

    lines.join("\n")
}

pub fn render_decode_hint_eval_comparison_summary(
    report: &DecodeHintEvalComparisonReport,
) -> String {
    let mut lines = vec![
        "# Decode Hint Eval Comparison".to_string(),
        String::new(),
        format!("- Left: `{}`", report.left_path.display()),
        format!("- Right: `{}`", report.right_path.display()),
        format!("- Generated at: `{}`", report.generated_at),
        format!("- Shared cases: {}", report.totals.shared_cases),
        format!("- Added cases: {}", report.totals.added_cases),
        format!("- Removed cases: {}", report.totals.removed_cases),
        format!("- Improved cases: {}", report.totals.improved_cases),
        format!("- Regressed cases: {}", report.totals.regressed_cases),
        format!("- Newly passing: {}", report.totals.newly_passing_cases),
        format!("- Newly failing: {}", report.totals.newly_failing_cases),
        String::new(),
        "## Case deltas".to_string(),
        String::new(),
    ];

    for case in &report.cases {
        let headline = match (
            case.left_candidate_wer,
            case.right_candidate_wer,
            case.candidate_wer_delta,
        ) {
            (Some(left), Some(right), Some(delta)) => format!(
                "- `{}` [{}]: candidate WER `{:.4}` -> `{:.4}` (delta `{:.4}`)",
                case.id, case.status, left, right, delta
            ),
            (None, Some(right), _) => format!(
                "- `{}` [{}]: new case on right with candidate WER `{:.4}`",
                case.id, case.status, right
            ),
            (Some(left), None, _) => format!(
                "- `{}` [{}]: removed case (left candidate WER `{:.4}`)",
                case.id, case.status, left
            ),
            _ => format!("- `{}` [{}]", case.id, case.status),
        };
        lines.push(headline);
        if !case.gained_focus_hits.is_empty() {
            lines.push(format!(
                "  gained hits: {}",
                case.gained_focus_hits.join(", ")
            ));
        }
        if !case.lost_focus_hits.is_empty() {
            lines.push(format!("  lost hits: {}", case.lost_focus_hits.join(", ")));
        }
        if !case.resolved_failures.is_empty() {
            lines.push(format!(
                "  resolved failures: {}",
                case.resolved_failures.join("; ")
            ));
        }
        if !case.newly_missing_terms.is_empty() {
            lines.push(format!(
                "  new failures: {}",
                case.newly_missing_terms.join("; ")
            ));
        }
    }

    lines.join("\n")
}

fn default_eval_content_type() -> ContentType {
    ContentType::Meeting
}

fn default_case_status() -> String {
    "pass".into()
}

fn resolve_report_path(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.join("results.json")
    } else {
        path.to_path_buf()
    }
}

fn collect_decode_hint_runs(
    eval_root: &Path,
    comparison_root: &Path,
) -> Result<Vec<DecodeHintRunIndexEntry>> {
    let mut runs = Vec::new();
    runs.extend(collect_eval_runs(eval_root)?);
    runs.extend(collect_comparison_runs(comparison_root)?);
    runs.sort_by(|left, right| right.generated_at.cmp(&left.generated_at));
    Ok(runs)
}

fn collect_eval_runs(root: &Path) -> Result<Vec<DecodeHintRunIndexEntry>> {
    let mut runs = Vec::new();
    if !root.exists() {
        return Ok(runs);
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if entry.file_name() == "clips" {
            continue;
        }
        let results_path = path.join("results.json");
        let summary_path = path.join("summary.md");
        if !results_path.exists() {
            continue;
        }
        let report = load_decode_hint_eval_report(&results_path)?;
        runs.push(DecodeHintRunIndexEntry {
            kind: "decode-hints".into(),
            run_dir: path,
            generated_at: report.generated_at,
            status: if report.failure_messages.is_empty() {
                "pass".into()
            } else {
                "fail".into()
            },
            source_path: report.corpus_path,
            cases_total: report.totals.cases_total,
            cases_failed: report.totals.cases_failed,
            improved_cases: report.totals.improved_cases,
            regressed_cases: report.totals.regressed_cases,
            newly_passing_cases: 0,
            newly_failing_cases: 0,
            summary_path,
        });
    }

    Ok(runs)
}

fn collect_comparison_runs(root: &Path) -> Result<Vec<DecodeHintRunIndexEntry>> {
    let mut runs = Vec::new();
    if !root.exists() {
        return Ok(runs);
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let comparison_path = path.join("comparison.json");
        let summary_path = path.join("summary.md");
        if !comparison_path.exists() {
            continue;
        }
        let raw = fs::read_to_string(&comparison_path)?;
        let report: DecodeHintEvalComparisonReport =
            serde_json::from_str(&raw).map_err(invalid_data_error)?;
        runs.push(DecodeHintRunIndexEntry {
            kind: "decode-hints-comparison".into(),
            run_dir: path,
            generated_at: report.generated_at,
            status: if report.totals.newly_failing_cases > 0 || report.totals.regressed_cases > 0 {
                "mixed".into()
            } else {
                "improved-or-stable".into()
            },
            source_path: report.right_path,
            cases_total: report.totals.shared_cases + report.totals.added_cases,
            cases_failed: report.totals.newly_failing_cases + report.totals.regressed_cases,
            improved_cases: report.totals.improved_cases,
            regressed_cases: report.totals.regressed_cases,
            newly_passing_cases: report.totals.newly_passing_cases,
            newly_failing_cases: report.totals.newly_failing_cases,
            summary_path,
        });
    }

    Ok(runs)
}

fn invalid_input(message: &str) -> MinutesError {
    MinutesError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message.to_string(),
    ))
}

fn invalid_data_error(error: impl std::fmt::Display) -> MinutesError {
    MinutesError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        error.to_string(),
    ))
}

fn transcribe_case(
    case: &DecodeHintEvalCase,
    config: &Config,
    hints: &DecodeHints,
) -> Result<transcribe::TranscribeResult> {
    let mut result = match case.content_type {
        ContentType::Meeting => {
            transcribe::transcribe_meeting_with_hints(&case.audio_path, config, hints)
                .map_err(MinutesError::from)
        }
        _ => transcribe::transcribe_with_hints(&case.audio_path, config, hints)
            .map_err(MinutesError::from),
    }?;

    if case.content_type == ContentType::Meeting && !hints.is_empty() {
        result.text = normalize_transcript_for_self_name_participant(
            &result.text,
            &case.attendees,
            &config.identity,
        );
    }

    Ok(result)
}

fn eval_text_for_compare(text: &str) -> String {
    text.lines()
        .filter_map(clean_transcript_line)
        .map(|line| normalize_space(&line).to_lowercase())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn word_error_rate(reference: &str, hypothesis: &str) -> f64 {
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

fn present_terms(text: &str, terms: &[String]) -> Vec<String> {
    let lower = text.to_lowercase();
    terms
        .iter()
        .filter(|term| lower.contains(&term.to_lowercase()))
        .cloned()
        .collect()
}

fn load_reference_text(case: &DecodeHintEvalCase) -> Result<String> {
    if !case.reference_text.trim().is_empty() {
        return Ok(case.reference_text.clone());
    }
    let Some(path) = &case.reference_path else {
        return Err(invalid_input(&format!(
            "{} missing reference_text/reference_path",
            case.id
        )));
    };
    Ok(fs::read_to_string(path)?)
}

#[cfg(feature = "parakeet")]
fn effective_parakeet_boost_phrases(config: &Config, hints: &DecodeHints) -> Vec<String> {
    transcribe::combined_parakeet_boost_phrases(config, hints)
}

#[cfg(not(feature = "parakeet"))]
fn effective_parakeet_boost_phrases(_config: &Config, _hints: &DecodeHints) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_report() -> DecodeHintEvalReport {
        DecodeHintEvalReport {
            generated_at: "2026-04-15T12:00:00Z".into(),
            corpus_path: PathBuf::from("/tmp/corpus.json"),
            options: DecodeHintEvalOptions::default(),
            totals: DecodeHintEvalTotals {
                cases_total: 1,
                cases_passed: 0,
                cases_failed: 1,
                cases_allowed_failures: 0,
                improved_cases: 0,
                regressed_cases: 1,
                average_delta_wer: 0.031,
            },
            cases: vec![DecodeHintEvalCaseResult {
                id: "case-1".into(),
                engine: "parakeet".into(),
                hint_debug: DecodeHintEvalHintDebug {
                    priority_phrases: vec!["Alex Chen".into()],
                    contextual_phrases: vec!["X1 Planning".into()],
                    whisper_prompt_phrases: vec!["Alex Chen".into(), "X1 Planning".into()],
                    parakeet_boost_phrases: vec!["Alex Chen".into(), "X1 Planning".into()],
                },
                baseline: DecodeHintEvalTranscriptMetrics {
                    wer: 0.12,
                    focus_hits: vec!["alex chen".into()],
                    forbidden_hits: vec![],
                },
                candidate: DecodeHintEvalTranscriptMetrics {
                    wer: 0.151,
                    focus_hits: vec!["alex chen".into()],
                    forbidden_hits: vec!["matt mullenweg".into()],
                },
                delta_wer: 0.031,
                max_wer_regression: Some(0.02),
                required_terms: vec!["alex chen".into()],
                forbidden_terms: vec!["matt mullenweg".into()],
                passed: false,
                status: "fail".into(),
                failure_reasons: vec![
                    "hinted WER regressed by 0.0310 (> 0.0200)".into(),
                    "contains forbidden hinted term 'matt mullenweg'".into(),
                ],
                allowed_failure_reasons: vec![],
            }],
            failure_messages: vec![
                "case-1 hinted WER regressed by 0.0310 (> 0.0200)".into(),
                "case-1 contains forbidden hinted term 'matt mullenweg'".into(),
            ],
        }
    }

    fn sample_allowed_failure_report() -> DecodeHintEvalReport {
        DecodeHintEvalReport {
            generated_at: "2026-04-15T12:00:00Z".into(),
            corpus_path: PathBuf::from("/tmp/corpus.json"),
            options: DecodeHintEvalOptions::default(),
            totals: DecodeHintEvalTotals {
                cases_total: 1,
                cases_passed: 1,
                cases_failed: 0,
                cases_allowed_failures: 1,
                improved_cases: 0,
                regressed_cases: 0,
                average_delta_wer: 0.0,
            },
            cases: vec![DecodeHintEvalCaseResult {
                id: "research-case".into(),
                engine: "parakeet".into(),
                hint_debug: DecodeHintEvalHintDebug::default(),
                baseline: DecodeHintEvalTranscriptMetrics {
                    wer: 0.12,
                    focus_hits: vec![],
                    forbidden_hits: vec![],
                },
                candidate: DecodeHintEvalTranscriptMetrics {
                    wer: 0.12,
                    focus_hits: vec!["pdf toolkit".into()],
                    forbidden_hits: vec![],
                },
                delta_wer: 0.0,
                max_wer_regression: Some(0.02),
                required_terms: vec!["garrett gunderson".into()],
                forbidden_terms: vec![],
                passed: true,
                status: "allowed-failure".into(),
                failure_reasons: vec![],
                allowed_failure_reasons: vec![
                    "missing required hinted term 'garrett gunderson'".into()
                ],
            }],
            failure_messages: vec![],
        }
    }

    #[test]
    fn render_summary_surfaces_failures() {
        let summary = render_decode_hint_eval_summary(&sample_report());
        assert!(summary.contains("Verdict: **FAIL**"));
        assert!(summary.contains("case-1"));
        assert!(summary.contains("matt mullenweg"));
    }

    #[test]
    fn render_summary_surfaces_allowed_failure_verdict() {
        let summary = render_decode_hint_eval_summary(&sample_allowed_failure_report());
        assert!(summary.contains("Verdict: **PASS WITH ALLOWED FAILURES**"));
        assert!(summary.contains("Allowed failures: 1"));
        assert!(
            summary.contains("allowed failures: missing required hinted term 'garrett gunderson'")
        );
    }

    #[test]
    fn write_artifacts_creates_expected_files() {
        let tmp = TempDir::new().unwrap();
        let request = DecodeHintEvalRequest {
            command: "minutes autoresearch decode-hints".into(),
            generated_at: "2026-04-15T12:00:00Z".into(),
            corpus_path: PathBuf::from("/tmp/corpus.json"),
            output_root: tmp.path().to_path_buf(),
            git_commit: Some("abc123".into()),
            options: DecodeHintEvalOptions::default(),
        };

        let paths = write_decode_hint_eval_artifacts(&request, &sample_report()).unwrap();
        assert!(paths.run_dir.exists());
        assert!(paths.request_json.exists());
        assert!(paths.results_json.exists());
        assert!(paths.baseline_json.exists());
        assert!(paths.candidate_json.exists());
        assert!(paths.summary_md.exists());
    }

    #[test]
    fn compare_reports_surfaces_improvements_and_new_failures() {
        let left = sample_report();
        let mut right = sample_report();
        right.generated_at = "2026-04-16T12:00:00Z".into();
        right.cases[0].candidate.wer = 0.10;
        right.cases[0].candidate.focus_hits.push("mat".into());
        right.cases[0].failure_reasons =
            vec!["missing required hinted term 'pdf extension'".into()];
        right.failure_messages = right.cases[0]
            .failure_reasons
            .iter()
            .map(|reason| format!("case-1 {reason}"))
            .collect();

        let tmp = TempDir::new().unwrap();
        let left_path = tmp.path().join("left.json");
        let right_path = tmp.path().join("right.json");
        fs::write(&left_path, serde_json::to_string_pretty(&left).unwrap()).unwrap();
        fs::write(&right_path, serde_json::to_string_pretty(&right).unwrap()).unwrap();

        let comparison = compare_decode_hint_eval_reports(&left_path, &right_path).unwrap();
        assert_eq!(comparison.totals.shared_cases, 1);
        assert_eq!(comparison.totals.improved_cases, 1);
        assert_eq!(
            comparison.cases[0].gained_focus_hits,
            vec!["mat".to_string()]
        );
        assert!(comparison.cases[0]
            .newly_missing_terms
            .iter()
            .any(|reason| reason.contains("pdf extension")));
        assert!(comparison.cases[0]
            .resolved_failures
            .iter()
            .any(|reason| reason.contains("matt mullenweg")));
    }

    #[test]
    fn collect_decode_hint_runs_lists_eval_and_comparison_runs() {
        let tmp = TempDir::new().unwrap();
        let eval_root = tmp.path().join("decode-hints");
        let comparison_root = tmp.path().join("decode-hints-comparisons");
        fs::create_dir_all(&eval_root).unwrap();
        fs::create_dir_all(&comparison_root).unwrap();

        let eval_dir = eval_root.join("2026-04-16T00-00-00Z");
        fs::create_dir_all(&eval_dir).unwrap();
        fs::write(
            eval_dir.join("results.json"),
            serde_json::to_string_pretty(&sample_report()).unwrap(),
        )
        .unwrap();
        fs::write(eval_dir.join("summary.md"), "# eval").unwrap();

        let comparison = DecodeHintEvalComparisonReport {
            generated_at: "2026-04-16T13:00:00Z".into(),
            left_path: PathBuf::from("/tmp/left.json"),
            right_path: PathBuf::from("/tmp/right.json"),
            totals: DecodeHintEvalComparisonTotals {
                shared_cases: 1,
                added_cases: 0,
                removed_cases: 0,
                improved_cases: 1,
                regressed_cases: 0,
                newly_passing_cases: 1,
                newly_failing_cases: 0,
                unchanged_cases: 0,
            },
            cases: vec![],
        };
        let comparison_dir = comparison_root.join("2026-04-16T13-00-00Z");
        fs::create_dir_all(&comparison_dir).unwrap();
        fs::write(
            comparison_dir.join("comparison.json"),
            serde_json::to_string_pretty(&comparison).unwrap(),
        )
        .unwrap();
        fs::write(comparison_dir.join("summary.md"), "# comparison").unwrap();

        let runs = collect_decode_hint_runs(&eval_root, &comparison_root).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].kind, "decode-hints-comparison");
        assert_eq!(runs[0].source_path, PathBuf::from("/tmp/right.json"));
        assert_eq!(runs[1].kind, "decode-hints");
        assert_eq!(runs[1].source_path, PathBuf::from("/tmp/corpus.json"));
    }

    #[test]
    fn checked_in_example_corpus_matches_supported_gate_shape() {
        let fixture = include_str!("../../../tests/fixtures/proper-name-eval.example.json");
        let cases: Vec<DecodeHintEvalCase> =
            serde_json::from_str(fixture).expect("example corpus should parse");

        assert_eq!(cases.len(), 3, "keep the public starter corpus intentional");
        assert!(
            cases.iter().any(|case| case.id == "self-intro-parakeet"),
            "starter corpus should include a parakeet self-name case"
        );
        assert!(
            cases.iter().any(|case| case.id == "self-intro-whisper"),
            "starter corpus should include a whisper self-name case"
        );
        let research_case = cases
            .iter()
            .find(|case| case.id == "external-proper-noun-research")
            .expect("starter corpus should include the external proper-noun research case");
        assert!(
            !research_case.allowed_failure_substrings.is_empty(),
            "external proper-noun example should stay explicitly scoped as research"
        );
    }
}
