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
    pub max_wer_regression: Option<f64>,
    #[serde(default)]
    pub require_hinted_terms: Vec<String>,
    #[serde(default)]
    pub forbid_hinted_terms: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalOptions {
    #[serde(default)]
    pub engine_override: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalTranscriptMetrics {
    pub wer: f64,
    pub focus_hits: Vec<String>,
    pub forbidden_hits: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalCaseResult {
    pub id: String,
    pub engine: String,
    pub baseline: DecodeHintEvalTranscriptMetrics,
    pub candidate: DecodeHintEvalTranscriptMetrics,
    pub delta_wer: f64,
    pub max_wer_regression: Option<f64>,
    pub required_terms: Vec<String>,
    pub forbidden_terms: Vec<String>,
    pub passed: bool,
    pub failure_reasons: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalTotals {
    pub cases_total: usize,
    pub cases_passed: usize,
    pub cases_failed: usize,
    pub improved_cases: usize,
    pub regressed_cases: usize,
    pub average_delta_wer: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodeHintEvalReport {
    pub generated_at: String,
    pub corpus_path: PathBuf,
    pub options: DecodeHintEvalOptions,
    pub totals: DecodeHintEvalTotals,
    pub cases: Vec<DecodeHintEvalCaseResult>,
    pub failure_messages: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
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

pub fn default_research_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".minutes")
        .join("research")
        .join("decode-hints")
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
        config.identity.name = case.identity_name.clone();
        config.identity.aliases = case.identity_aliases.clone();

        let reference = eval_text_for_compare(&load_reference_text(&case)?);
        let hints = build_decode_hints(
            case.title.as_deref(),
            case.calendar_event_title.as_deref(),
            case.pre_context.as_deref(),
            &case.attendees,
            Some(&config.identity),
        );

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

        for reason in &case_failures {
            failure_messages.push(format!("{} {reason}", case.id));
        }

        results.push(DecodeHintEvalCaseResult {
            id: case.id,
            engine: config.transcription.engine.clone(),
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
            passed: case_failures.is_empty(),
            failure_reasons: case_failures,
        });
    }

    let totals = DecodeHintEvalTotals {
        cases_total: results.len(),
        cases_passed: results.iter().filter(|case| case.passed).count(),
        cases_failed: results.iter().filter(|case| !case.passed).count(),
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

pub fn render_decode_hint_eval_summary(report: &DecodeHintEvalReport) -> String {
    let verdict = if report.failure_messages.is_empty() {
        "PASS"
    } else {
        "FAIL"
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
            "- `{}`: {} (`{:.4}` -> `{:.4}`, delta `{:.4}`)",
            case.id,
            if case.passed { "pass" } else { "fail" },
            case.baseline.wer,
            case.candidate.wer,
            case.delta_wer
        ));
        if !case.failure_reasons.is_empty() {
            lines.push(format!("  reasons: {}", case.failure_reasons.join("; ")));
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

fn default_eval_content_type() -> ContentType {
    ContentType::Meeting
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
                improved_cases: 0,
                regressed_cases: 1,
                average_delta_wer: 0.031,
            },
            cases: vec![DecodeHintEvalCaseResult {
                id: "case-1".into(),
                engine: "parakeet".into(),
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
                failure_reasons: vec![
                    "hinted WER regressed by 0.0310 (> 0.0200)".into(),
                    "contains forbidden hinted term 'matt mullenweg'".into(),
                ],
            }],
            failure_messages: vec![
                "case-1 hinted WER regressed by 0.0310 (> 0.0200)".into(),
                "case-1 contains forbidden hinted term 'matt mullenweg'".into(),
            ],
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
}
