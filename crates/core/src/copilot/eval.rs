//! Versioned deterministic replay and scoring for the real-time copilot.
//!
//! The scored suite uses only synthetic fixtures and a no-network scripted
//! model. Both accelerated and real-time playback share the same logical clock;
//! real-time playback merely sleeps before advancing it.

use super::{
    BattleCard, CancelToken, CopilotClock, CopilotModel, CopilotRequest, CopilotRunner,
    CopilotState, CopilotUtterance, LatencyRecord, ModelError, ModelEventSink, ModelHealth,
    ModelHealthStatus, ModelStreamEvent, NudgeDraft, NudgeKind, NudgePolicy, PartialLatencySeed,
    RunnerEvent, SubmitOutcome, TranscriptUpdateKind,
};
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

pub const EVAL_SUITE_VERSION: u32 = 1;
pub const EVAL_FIXED_SEED: u64 = 0x4d49_4e55_5445_5301;
const TRIGGER_DELAY_MS: u64 = 8;
const CONTEXT_DELAY_MS: u64 = 7;
const DEFAULT_MODEL_COMPLETION_MS: u64 = 40;
const DUPLICATE_WINDOW_MS: u64 = 30_000;
const WORKER_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("failed to read copilot eval fixture {path}: {source}")]
    ReadFixture {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid copilot eval fixture {path}: {source}")]
    ParseFixture {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("invalid copilot eval fixture '{fixture}': {message}")]
    InvalidFixture { fixture: String, message: String },
    #[error("copilot eval runner stalled while waiting for {0}")]
    RunnerStalled(String),
    #[error("copilot eval runner rejected fixture '{fixture}' revision {revision}: {outcome:?}")]
    SubmitRejected {
        fixture: String,
        revision: u64,
        outcome: SubmitOutcome,
    },
    #[error("copilot eval runner degraded: {0}")]
    RunnerDegraded(String),
    #[error("copilot eval fixture directory contains no JSON fixtures: {0}")]
    EmptyFixtureDirectory(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureContentOrigin {
    Synthetic,
    Public,
    Redacted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalFixture {
    pub schema_version: u32,
    pub id: String,
    pub description: String,
    pub content_origin: FixtureContentOrigin,
    pub goal: String,
    #[serde(default)]
    pub synthesize_partials: bool,
    pub transcript: Vec<FixtureUtterance>,
    #[serde(default)]
    pub labels: FixtureLabels,
    #[serde(default)]
    pub mock_script: Vec<MockRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureUtterance {
    pub utterance_sequence: u64,
    #[serde(default = "default_source")]
    pub source: String,
    pub offset_ms: u64,
    pub duration_ms: u64,
    pub final_text: String,
    #[serde(default)]
    pub partials: Vec<FixturePartial>,
}

fn default_source() -> String {
    "synthetic-fixture".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixturePartial {
    /// Meeting-relative publication time.
    pub at_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureLabels {
    #[serde(default)]
    pub opportunities: Vec<OpportunityLabel>,
    #[serde(default)]
    pub no_opportunity_ranges: Vec<NoOpportunityRange>,
    #[serde(default)]
    pub revision_reversals: Vec<RevisionReversal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpportunityLabel {
    pub id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<NudgeKind>,
    #[serde(default)]
    pub match_any: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoOpportunityRange {
    pub id: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionReversal {
    pub utterance_sequence: u64,
    pub from_contains: String,
    pub to_contains: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockCue {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_kind: Option<TranscriptUpdateKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utterance_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_contains: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockRule {
    pub cue: MockCue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    pub completion_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<NudgeDraft>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayMode {
    RealTime,
    Accelerated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalOptions {
    pub mode: ReplayMode,
}

impl Default for EvalOptions {
    fn default() -> Self {
        Self {
            mode: ReplayMode::RealTime,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateMetric {
    pub numerator: usize,
    pub denominator: usize,
    pub rate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatencyPercentiles {
    pub samples: usize,
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityMetrics {
    pub useful_nudge_precision: RateMetric,
    pub opportunity_recall: RateMetric,
    pub stale_nudge_rate: RateMetric,
    pub contradiction_after_revision_rate: RateMetric,
    pub duplicate_nagging_rate: RateMetric,
    pub no_nudge_quality: RateMetric,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NudgeObservation {
    pub kind: NudgeKind,
    pub text: String,
    pub source_chip: String,
    pub evidence_revision: u64,
    pub evidence_time_ms: u64,
    pub delivered_at_ms: u64,
    pub grounded_in_partial: bool,
    pub stale_at_delivery: bool,
    pub contradiction_after_revision: bool,
    pub duplicate_or_nagging: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_opportunity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixtureReport {
    pub fixture_id: String,
    pub description: String,
    pub content_origin: FixtureContentOrigin,
    pub transcript_updates: usize,
    pub nudges: Vec<NudgeObservation>,
    pub quality: QualityMetrics,
    pub latency: BTreeMap<String, LatencyPercentiles>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineThresholds {
    pub min_useful_nudge_precision: f64,
    pub min_opportunity_recall: f64,
    pub max_stale_nudge_rate: f64,
    pub max_contradiction_after_revision_rate: f64,
    pub max_duplicate_nagging_rate: f64,
    pub min_no_nudge_quality: f64,
    pub max_model_to_first_token_p95_ms: f64,
    pub max_audio_to_nudge_p95_ms: f64,
}

impl Default for BaselineThresholds {
    fn default() -> Self {
        Self {
            min_useful_nudge_precision: 0.90,
            min_opportunity_recall: 0.90,
            max_stale_nudge_rate: 0.0,
            max_contradiction_after_revision_rate: 0.0,
            max_duplicate_nagging_rate: 0.0,
            min_no_nudge_quality: 1.0,
            max_model_to_first_token_p95_ms: 100.0,
            max_audio_to_nudge_p95_ms: 5_000.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuiteSummary {
    pub fixtures: usize,
    pub transcript_updates: usize,
    pub nudges: usize,
    pub quality: QualityMetrics,
    pub latency: BTreeMap<String, LatencyPercentiles>,
    pub baseline_passed: bool,
    pub baseline_failures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalReport {
    pub suite_version: u32,
    pub fixed_seed: u64,
    pub mode: ReplayMode,
    pub provider: String,
    pub fixtures: Vec<FixtureReport>,
    pub summary: SuiteSummary,
    pub thresholds: BaselineThresholds,
}

#[derive(Debug)]
struct LogicalClock {
    base: Instant,
    base_utc: DateTime<Utc>,
    logical_us: Mutex<u64>,
    advanced: Condvar,
    mode: ReplayMode,
    real_started: Instant,
}

impl LogicalClock {
    fn new(mode: ReplayMode) -> Self {
        Self {
            base: Instant::now(),
            base_utc: Utc
                .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("fixed eval epoch is valid"),
            logical_us: Mutex::new(0),
            advanced: Condvar::new(),
            mode,
            real_started: Instant::now(),
        }
    }

    fn elapsed_us(&self) -> u64 {
        *self.logical_us.lock().unwrap()
    }

    fn instant_at(&self, micros: u64) -> Instant {
        self.base + Duration::from_micros(micros)
    }

    fn advance_to(&self, target_us: u64) {
        if self.mode == ReplayMode::RealTime {
            let target = self.real_started + Duration::from_micros(target_us);
            while Instant::now() < target {
                std::thread::sleep(
                    target
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(20)),
                );
            }
        }
        let mut now = self.logical_us.lock().unwrap();
        *now = (*now).max(target_us);
        self.advanced.notify_all();
    }

    fn wait_until(&self, target_us: u64, cancel: &CancelToken) -> bool {
        let mut now = self.logical_us.lock().unwrap();
        while *now < target_us {
            if cancel.is_cancelled() {
                return false;
            }
            let (next, _) = self
                .advanced
                .wait_timeout(now, Duration::from_millis(1))
                .unwrap();
            now = next;
        }
        !cancel.is_cancelled()
    }
}

impl CopilotClock for LogicalClock {
    fn monotonic_now(&self) -> Instant {
        self.instant_at(self.elapsed_us())
    }

    fn utc_now(&self) -> DateTime<Utc> {
        let micros = self.elapsed_us().min(i64::MAX as u64) as i64;
        self.base_utc + chrono::Duration::microseconds(micros)
    }
}

#[derive(Debug, Clone, Copy)]
enum MockSignal {
    Started {
        evidence_revision: u64,
        first_token_us: Option<u64>,
        completion_us: u64,
    },
    FirstToken(u64),
    Completed(u64),
    Cancelled(u64),
}

/// Deterministic, no-network copilot provider driven by fixture cues.
pub struct ScriptedCopilotModel {
    rules: Vec<MockRule>,
    uses: Mutex<Vec<bool>>,
    clock: Arc<LogicalClock>,
    signal_tx: Sender<MockSignal>,
}

impl ScriptedCopilotModel {
    fn new(rules: Vec<MockRule>, clock: Arc<LogicalClock>, signal_tx: Sender<MockSignal>) -> Self {
        let uses = vec![false; rules.len()];
        Self {
            rules,
            uses: Mutex::new(uses),
            clock,
            signal_tx,
        }
    }

    fn response_for(&self, request: &CopilotRequest) -> MockRule {
        let trigger = request.utterances.iter().find(|utterance| {
            utterance.utterance_sequence == request.evidence_utterance_sequence
                && utterance.revision == request.evidence_utterance_revision
        });
        let mut uses = self.uses.lock().unwrap();
        if let Some((index, rule)) = self.rules.iter().enumerate().find(|(index, rule)| {
            !uses[*index]
                && rule.cue.matches(request, trigger)
                && rule.completion_ms >= rule.first_token_ms.unwrap_or_default()
        }) {
            uses[index] = true;
            return rule.clone();
        }
        MockRule {
            cue: MockCue::default(),
            first_token_ms: None,
            completion_ms: DEFAULT_MODEL_COMPLETION_MS,
            draft: None,
        }
    }
}

impl MockCue {
    fn matches(&self, request: &CopilotRequest, trigger: Option<&CopilotUtterance>) -> bool {
        self.update_kind
            .is_none_or(|kind| kind == request.update_kind)
            && self
                .utterance_sequence
                .is_none_or(|sequence| sequence == request.evidence_utterance_sequence)
            && self.text_contains.as_ref().is_none_or(|needle| {
                trigger.is_some_and(|utterance| contains(&utterance.text, needle))
            })
    }
}

impl CopilotModel for ScriptedCopilotModel {
    fn provider_name(&self) -> &str {
        "mock"
    }

    fn model_name(&self) -> &str {
        "scripted-v1"
    }

    fn prewarm(&self) -> Result<(), ModelError> {
        Ok(())
    }

    fn stream_structured(
        &self,
        request: &CopilotRequest,
        cancel: &CancelToken,
        sink: &dyn ModelEventSink,
    ) -> Result<NudgeDraft, ModelError> {
        let rule = self.response_for(request);
        let started_us = self.clock.elapsed_us();
        let first_token_us = rule
            .first_token_ms
            .map(|delay| started_us.saturating_add(delay.saturating_mul(1_000)));
        let completion_us = started_us.saturating_add(rule.completion_ms.saturating_mul(1_000));
        let revision = request.evidence_revision;
        let _ = self.signal_tx.send(MockSignal::Started {
            evidence_revision: revision,
            first_token_us,
            completion_us,
        });

        if let Some(first_token_us) = first_token_us {
            if !self.clock.wait_until(first_token_us, cancel) {
                let _ = self.signal_tx.send(MockSignal::Cancelled(revision));
                return Err(ModelError::cancelled());
            }
            sink.on_event(ModelStreamEvent::TextDelta("{".into()));
            let _ = self.signal_tx.send(MockSignal::FirstToken(revision));
        }
        if !self.clock.wait_until(completion_us, cancel) {
            let _ = self.signal_tx.send(MockSignal::Cancelled(revision));
            return Err(ModelError::cancelled());
        }
        let _ = self.signal_tx.send(MockSignal::Completed(revision));
        Ok(rule.draft.unwrap_or_else(|| NudgeDraft {
            kind: NudgeKind::Hold,
            text: String::new(),
            source_chip: String::new(),
        }))
    }

    fn health(&self) -> ModelHealth {
        ModelHealth {
            provider: self.provider_name().into(),
            model: self.model_name().into(),
            status: ModelHealthStatus::Available,
            detail: "deterministic no-network fixture provider".into(),
            checked_ts: self.clock.utc_now(),
        }
    }
}

#[derive(Debug, Clone)]
struct ExpandedEvent {
    evidence_revision: u64,
    utterance_sequence: u64,
    utterance_revision: u64,
    update_kind: TranscriptUpdateKind,
    source: String,
    text: String,
    offset_ms: u64,
    duration_ms: u64,
    audio_origin_ms: u64,
    published_ms: u64,
    trigger_ms: u64,
    context_ready_ms: u64,
}

#[derive(Debug, Clone)]
struct RequestSnapshot {
    request: CopilotRequest,
    evidence_time_ms: u64,
}

#[derive(Debug, Clone, Copy)]
struct ActiveCall {
    evidence_revision: u64,
    first_token_us: Option<u64>,
    completion_us: u64,
    first_token_observed: bool,
}

impl ActiveCall {
    fn next_milestone_us(&self) -> u64 {
        if !self.first_token_observed {
            self.first_token_us.unwrap_or(self.completion_us)
        } else {
            self.completion_us
        }
    }
}

struct ReplayResult {
    report: FixtureReport,
    latency_records: Vec<LatencyRecord>,
}

pub fn builtin_fixtures() -> Result<Vec<EvalFixture>, EvalError> {
    const BUILTINS: [(&str, &str); 4] = [
        (
            "long_monologue.json",
            include_str!("../../tests/fixtures/copilot_eval/v1/long_monologue.json"),
        ),
        (
            "revision_reversal.json",
            include_str!("../../tests/fixtures/copilot_eval/v1/revision_reversal.json"),
        ),
        (
            "overlapping_opportunities.json",
            include_str!("../../tests/fixtures/copilot_eval/v1/overlapping_opportunities.json"),
        ),
        (
            "no_opportunity.json",
            include_str!("../../tests/fixtures/copilot_eval/v1/no_opportunity.json"),
        ),
    ];
    BUILTINS
        .into_iter()
        .map(|(name, contents)| parse_fixture(Path::new(name), contents))
        .collect()
}

pub fn load_fixtures_dir(path: &Path) -> Result<Vec<EvalFixture>, EvalError> {
    let mut paths = std::fs::read_dir(path)
        .map_err(|source| EvalError::ReadFixture {
            path: path.to_path_buf(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|entry| entry.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        return Err(EvalError::EmptyFixtureDirectory(path.to_path_buf()));
    }
    paths
        .into_iter()
        .map(|fixture_path| {
            let contents = std::fs::read_to_string(&fixture_path).map_err(|source| {
                EvalError::ReadFixture {
                    path: fixture_path.clone(),
                    source,
                }
            })?;
            parse_fixture(&fixture_path, &contents)
        })
        .collect()
}

fn parse_fixture(path: &Path, contents: &str) -> Result<EvalFixture, EvalError> {
    let fixture: EvalFixture =
        serde_json::from_str(contents).map_err(|source| EvalError::ParseFixture {
            path: path.to_path_buf(),
            source,
        })?;
    validate_fixture(&fixture)?;
    Ok(fixture)
}

fn validate_fixture(fixture: &EvalFixture) -> Result<(), EvalError> {
    let invalid = |message: String| EvalError::InvalidFixture {
        fixture: fixture.id.clone(),
        message,
    };
    if fixture.schema_version != EVAL_SUITE_VERSION {
        return Err(invalid(format!(
            "schema_version {} does not match suite version {EVAL_SUITE_VERSION}",
            fixture.schema_version
        )));
    }
    if fixture.id.trim().is_empty() || fixture.goal.trim().is_empty() {
        return Err(invalid("id and goal must be non-empty".into()));
    }
    if fixture.transcript.is_empty() {
        return Err(invalid("transcript must contain at least one final".into()));
    }
    let mut sequences = BTreeSet::new();
    for utterance in &fixture.transcript {
        if !sequences.insert(utterance.utterance_sequence) {
            return Err(invalid(format!(
                "duplicate utterance_sequence {}",
                utterance.utterance_sequence
            )));
        }
        if utterance.duration_ms == 0 || utterance.final_text.trim().is_empty() {
            return Err(invalid(format!(
                "utterance {} needs non-empty text and duration",
                utterance.utterance_sequence
            )));
        }
        let final_at = utterance.offset_ms.saturating_add(utterance.duration_ms);
        let mut prior = utterance.offset_ms;
        for partial in &utterance.partials {
            if partial.at_ms <= prior || partial.at_ms >= final_at || partial.text.trim().is_empty()
            {
                return Err(invalid(format!(
                    "utterance {} partial times must increase inside the utterance",
                    utterance.utterance_sequence
                )));
            }
            prior = partial.at_ms;
        }
    }
    for opportunity in &fixture.labels.opportunities {
        if opportunity.id.trim().is_empty()
            || opportunity.start_ms > opportunity.end_ms
            || opportunity.match_any.is_empty()
        {
            return Err(invalid(format!(
                "opportunity '{}' needs an ordered range and match_any terms",
                opportunity.id
            )));
        }
    }
    for range in &fixture.labels.no_opportunity_ranges {
        if range.id.trim().is_empty() || range.start_ms > range.end_ms {
            return Err(invalid(format!(
                "no-opportunity range '{}' is invalid",
                range.id
            )));
        }
    }
    for rule in &fixture.mock_script {
        if rule.completion_ms < rule.first_token_ms.unwrap_or_default() {
            return Err(invalid(
                "mock completion_ms must be at or after first_token_ms".into(),
            ));
        }
    }
    Ok(())
}

pub fn run_builtin_suite(options: EvalOptions) -> Result<EvalReport, EvalError> {
    run_suite(&builtin_fixtures()?, options)
}

pub fn run_suite(fixtures: &[EvalFixture], options: EvalOptions) -> Result<EvalReport, EvalError> {
    if fixtures.is_empty() {
        return Err(EvalError::InvalidFixture {
            fixture: "suite".into(),
            message: "at least one fixture is required".into(),
        });
    }
    let mut reports = Vec::with_capacity(fixtures.len());
    let mut all_records = Vec::new();
    for fixture in fixtures {
        validate_fixture(fixture)?;
        let replay = run_fixture(fixture, options.mode)?;
        reports.push(replay.report);
        all_records.extend(replay.latency_records);
    }

    let quality = aggregate_quality(reports.iter().map(|report| &report.quality));
    let latency = aggregate_latency(&all_records);
    let thresholds = BaselineThresholds::default();
    let baseline_failures = threshold_failures(&quality, &latency, &thresholds);
    let summary = SuiteSummary {
        fixtures: reports.len(),
        transcript_updates: reports.iter().map(|report| report.transcript_updates).sum(),
        nudges: reports.iter().map(|report| report.nudges.len()).sum(),
        quality,
        latency,
        baseline_passed: baseline_failures.is_empty(),
        baseline_failures,
    };
    Ok(EvalReport {
        suite_version: EVAL_SUITE_VERSION,
        fixed_seed: EVAL_FIXED_SEED,
        mode: options.mode,
        provider: "mock/scripted-v1 (no network)".into(),
        fixtures: reports,
        summary,
        thresholds,
    })
}

fn run_fixture(fixture: &EvalFixture, mode: ReplayMode) -> Result<ReplayResult, EvalError> {
    let events = expand_fixture(fixture);
    let clock = Arc::new(LogicalClock::new(mode));
    let (signal_tx, signal_rx) = mpsc::channel();
    let model = Arc::new(ScriptedCopilotModel::new(
        fixture.mock_script.clone(),
        Arc::clone(&clock),
        signal_tx,
    ));
    let runner = CopilotRunner::start_with_clock(
        model,
        NudgePolicy::new(12_000),
        Duration::ZERO,
        clock.clone(),
    );
    wait_for_runner_ready(&runner)?;

    let epoch = runner.session_epoch();
    let mut event_index = 0;
    let mut active_call: Option<ActiveCall> = None;
    let mut transcript = BTreeMap::<u64, CopilotUtterance>::new();
    let mut snapshots = BTreeMap::<u64, RequestSnapshot>::new();
    let mut observations = Vec::new();
    let mut latest_revision = 0;

    while event_index < events.len() || active_call.is_some() {
        let next_event_us = events
            .get(event_index)
            .map(|event| event.context_ready_ms.saturating_mul(1_000));
        let next_model_us = active_call.map(|call| call.next_milestone_us());
        let process_event = match (next_event_us, next_model_us) {
            (Some(event_us), Some(model_us)) => event_us <= model_us,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };

        if process_event {
            let event = &events[event_index];
            clock.advance_to(event.context_ready_ms.saturating_mul(1_000));
            runner.tick(clock.utc_now());
            apply_event_freshness(&runner, epoch, event);
            transcript.insert(
                event.utterance_sequence,
                CopilotUtterance {
                    utterance_sequence: event.utterance_sequence,
                    revision: event.utterance_revision,
                    update_kind: event.update_kind,
                    source: event.source.clone(),
                    text: event.text.clone(),
                    speaker: None,
                    speaker_verified: false,
                    offset_ms: event.offset_ms,
                    duration_ms: event.duration_ms,
                },
            );
            let request = CopilotRequest {
                goal: fixture.goal.clone(),
                session_epoch: epoch,
                evidence_revision: event.evidence_revision,
                evidence_utterance_sequence: event.utterance_sequence,
                evidence_utterance_revision: event.utterance_revision,
                update_kind: event.update_kind,
                utterances: transcript.values().cloned().collect(),
                battle_card: BattleCard::empty(),
            };
            snapshots.insert(
                event.evidence_revision,
                RequestSnapshot {
                    request: request.clone(),
                    evidence_time_ms: event.published_ms,
                },
            );
            let seed = PartialLatencySeed {
                session_epoch: epoch,
                utterance_sequence: event.utterance_sequence,
                utterance_revision: event.utterance_revision,
                audio_received_at: clock.instant_at(event.audio_origin_ms.saturating_mul(1_000)),
                partial_published_at: clock.instant_at(event.published_ms.saturating_mul(1_000)),
                trigger_at: clock.instant_at(event.trigger_ms.saturating_mul(1_000)),
                context_ready_at: clock.instant_at(event.context_ready_ms.saturating_mul(1_000)),
            };
            let outcome = runner.submit_with_latency(request, seed);
            if !matches!(
                outcome,
                SubmitOutcome::Queued | SubmitOutcome::CancelledOlderRequest
            ) {
                return Err(EvalError::SubmitRejected {
                    fixture: fixture.id.clone(),
                    revision: event.evidence_revision,
                    outcome,
                });
            }
            latest_revision = event.evidence_revision;
            active_call = Some(wait_for_started(
                &signal_rx,
                event.evidence_revision,
                &runner,
            )?);
            event_index += 1;
        } else if let Some(mut call) = active_call {
            let target_us = call.next_milestone_us();
            clock.advance_to(target_us);
            runner.tick(clock.utc_now());
            if !call.first_token_observed && call.first_token_us.is_some() {
                wait_for_signal(
                    &signal_rx,
                    |signal| matches!(signal, MockSignal::FirstToken(revision) if revision == call.evidence_revision),
                    format!("first token for revision {}", call.evidence_revision),
                )?;
                call.first_token_observed = true;
                active_call = Some(call);
            } else {
                wait_for_signal(
                    &signal_rx,
                    |signal| matches!(signal, MockSignal::Completed(revision) if revision == call.evidence_revision),
                    format!("model completion for revision {}", call.evidence_revision),
                )?;
                wait_for_request_settled(&runner, call.evidence_revision)?;
                drain_runner_events(
                    &runner,
                    &clock,
                    &snapshots,
                    &transcript,
                    latest_revision,
                    &mut observations,
                )?;
                active_call = None;
            }
        }
    }

    drain_runner_events(
        &runner,
        &clock,
        &snapshots,
        &transcript,
        latest_revision,
        &mut observations,
    )?;
    let latency_records = runner.latency_records();
    runner.stop();
    let quality = score_observations(fixture, &events, &snapshots, &mut observations);
    let report = FixtureReport {
        fixture_id: fixture.id.clone(),
        description: fixture.description.clone(),
        content_origin: fixture.content_origin,
        transcript_updates: events.len(),
        nudges: observations,
        quality,
        latency: aggregate_latency(&latency_records),
    };
    Ok(ReplayResult {
        report,
        latency_records,
    })
}

fn wait_for_runner_ready(runner: &CopilotRunner) -> Result<(), EvalError> {
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while Instant::now() < deadline {
        let health = runner.health();
        match health.state {
            CopilotState::Listening => return Ok(()),
            CopilotState::Degraded => {
                return Err(EvalError::RunnerDegraded(
                    health
                        .last_error
                        .unwrap_or_else(|| "mock prewarm failed".into()),
                ));
            }
            _ => std::thread::yield_now(),
        }
    }
    Err(EvalError::RunnerStalled("mock provider prewarm".into()))
}

fn wait_for_started(
    signal_rx: &Receiver<MockSignal>,
    evidence_revision: u64,
    runner: &CopilotRunner,
) -> Result<ActiveCall, EvalError> {
    let deadline = Instant::now() + WORKER_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(EvalError::RunnerStalled(format!(
                "model start for revision {evidence_revision}"
            )));
        }
        match signal_rx.recv_timeout(remaining) {
            Ok(MockSignal::Started {
                evidence_revision: started_revision,
                first_token_us,
                completion_us,
            }) if started_revision == evidence_revision => {
                return Ok(ActiveCall {
                    evidence_revision,
                    first_token_us,
                    completion_us,
                    first_token_observed: false,
                });
            }
            Ok(MockSignal::Cancelled(cancelled_revision)) => {
                let _ = cancelled_revision;
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {
                return Err(EvalError::RunnerStalled(format!(
                    "model start for revision {evidence_revision}"
                )));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(EvalError::RunnerStalled(
                    "mock provider signal channel disconnected".into(),
                ));
            }
        }
        if runner.health().state == CopilotState::Degraded {
            return Err(EvalError::RunnerDegraded(
                runner
                    .health()
                    .last_error
                    .unwrap_or_else(|| "model request failed".into()),
            ));
        }
    }
}

fn wait_for_signal(
    signal_rx: &Receiver<MockSignal>,
    matches_signal: impl Fn(MockSignal) -> bool,
    description: String,
) -> Result<(), EvalError> {
    let deadline = Instant::now() + WORKER_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(EvalError::RunnerStalled(description));
        }
        match signal_rx.recv_timeout(remaining) {
            Ok(signal) if matches_signal(signal) => return Ok(()),
            Ok(MockSignal::Cancelled(revision)) => {
                return Err(EvalError::RunnerStalled(format!(
                    "revision {revision} was unexpectedly cancelled while waiting for {description}"
                )));
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {
                return Err(EvalError::RunnerStalled(description));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(EvalError::RunnerStalled(
                    "mock provider signal channel disconnected".into(),
                ));
            }
        }
    }
}

fn wait_for_request_settled(
    runner: &CopilotRunner,
    evidence_revision: u64,
) -> Result<(), EvalError> {
    let deadline = Instant::now() + WORKER_TIMEOUT;
    while Instant::now() < deadline {
        let health = runner.health();
        if health.in_flight_revision != Some(evidence_revision)
            && health.state != CopilotState::Thinking
        {
            if health.state == CopilotState::Degraded {
                return Err(EvalError::RunnerDegraded(
                    health
                        .last_error
                        .unwrap_or_else(|| "model request failed".into()),
                ));
            }
            return Ok(());
        }
        std::thread::yield_now();
    }
    Err(EvalError::RunnerStalled(format!(
        "runner settlement for revision {evidence_revision}"
    )))
}

fn apply_event_freshness(runner: &CopilotRunner, session_epoch: u64, event: &ExpandedEvent) {
    match event.update_kind {
        TranscriptUpdateKind::Partial => runner.supersede_partial_revision(
            session_epoch,
            event.utterance_sequence,
            event.utterance_revision,
        ),
        TranscriptUpdateKind::Final => {
            runner.retract_partials(session_epoch, event.utterance_sequence)
        }
    }
}

fn drain_runner_events(
    runner: &CopilotRunner,
    clock: &LogicalClock,
    snapshots: &BTreeMap<u64, RequestSnapshot>,
    transcript: &BTreeMap<u64, CopilotUtterance>,
    latest_revision: u64,
    observations: &mut Vec<NudgeObservation>,
) -> Result<(), EvalError> {
    while let Some(event) = runner.try_recv() {
        match event {
            RunnerEvent::Nudge(nudge) => {
                let Some(snapshot) = snapshots.get(&nudge.evidence_revision) else {
                    continue;
                };
                let grounded = nudge.grounded_partial_identity();
                let stale_partial = grounded.is_some_and(|(sequence, revision)| {
                    transcript.get(&sequence).is_none_or(|utterance| {
                        utterance.update_kind != TranscriptUpdateKind::Partial
                            || utterance.revision != revision
                    })
                });
                observations.push(NudgeObservation {
                    kind: nudge.kind,
                    text: nudge.text,
                    source_chip: nudge.source_chip,
                    evidence_revision: nudge.evidence_revision,
                    evidence_time_ms: snapshot.evidence_time_ms,
                    delivered_at_ms: clock.elapsed_us() / 1_000,
                    grounded_in_partial: grounded.is_some(),
                    stale_at_delivery: latest_revision > nudge.evidence_revision || stale_partial,
                    contradiction_after_revision: false,
                    duplicate_or_nagging: false,
                    matched_opportunity: None,
                });
            }
            RunnerEvent::Degraded { error } => return Err(EvalError::RunnerDegraded(error)),
            RunnerEvent::StateChanged(_)
            | RunnerEvent::Model(_)
            | RunnerEvent::RequestCancelled { .. }
            | RunnerEvent::EvidenceRetracted { .. } => {}
        }
    }
    Ok(())
}

fn expand_fixture(fixture: &EvalFixture) -> Vec<ExpandedEvent> {
    let mut raw = Vec::new();
    for utterance in &fixture.transcript {
        let partials = if utterance.partials.is_empty() && fixture.synthesize_partials {
            synthesize_partials(fixture, utterance)
        } else {
            utterance.partials.clone()
        };
        for (index, partial) in partials.iter().enumerate() {
            raw.push(ExpandedEvent {
                evidence_revision: 0,
                utterance_sequence: utterance.utterance_sequence,
                utterance_revision: index as u64 + 1,
                update_kind: TranscriptUpdateKind::Partial,
                source: utterance.source.clone(),
                text: partial.text.clone(),
                offset_ms: utterance.offset_ms,
                duration_ms: partial.at_ms.saturating_sub(utterance.offset_ms),
                audio_origin_ms: utterance.offset_ms,
                published_ms: partial.at_ms,
                trigger_ms: partial.at_ms.saturating_add(TRIGGER_DELAY_MS),
                context_ready_ms: partial
                    .at_ms
                    .saturating_add(TRIGGER_DELAY_MS)
                    .saturating_add(CONTEXT_DELAY_MS),
            });
        }
        let published_ms = utterance.offset_ms.saturating_add(utterance.duration_ms);
        raw.push(ExpandedEvent {
            evidence_revision: 0,
            utterance_sequence: utterance.utterance_sequence,
            utterance_revision: partials.len() as u64 + 1,
            update_kind: TranscriptUpdateKind::Final,
            source: utterance.source.clone(),
            text: utterance.final_text.clone(),
            offset_ms: utterance.offset_ms,
            duration_ms: utterance.duration_ms,
            audio_origin_ms: utterance.offset_ms,
            published_ms,
            trigger_ms: published_ms.saturating_add(TRIGGER_DELAY_MS),
            context_ready_ms: published_ms
                .saturating_add(TRIGGER_DELAY_MS)
                .saturating_add(CONTEXT_DELAY_MS),
        });
    }
    raw.sort_by_key(|event| {
        (
            event.context_ready_ms,
            event.utterance_sequence,
            event.utterance_revision,
        )
    });
    for (index, event) in raw.iter_mut().enumerate() {
        event.evidence_revision = index as u64 + 1;
    }
    raw
}

fn synthesize_partials(fixture: &EvalFixture, utterance: &FixtureUtterance) -> Vec<FixturePartial> {
    let words = utterance.final_text.split_whitespace().collect::<Vec<_>>();
    if words.len() < 4 || utterance.duration_ms < 600 {
        return Vec::new();
    }
    let mut rng = StableRng::new(stable_seed(fixture, utterance.utterance_sequence));
    let schedule_percent = [30_u64, 60, 82];
    let word_percent = [45_usize, 72, 90];
    let mut partials = Vec::new();
    let mut prior_at = utterance.offset_ms;
    for (schedule, word_share) in schedule_percent.into_iter().zip(word_percent) {
        let jitter = rng.next_bounded(7) as i64 - 3;
        let adjusted = (schedule as i64 + jitter).clamp(20, 92) as u64;
        let relative_at = utterance.duration_ms.saturating_mul(adjusted) / 100;
        let at_ms = utterance
            .offset_ms
            .saturating_add(relative_at)
            .max(prior_at.saturating_add(1));
        let word_count =
            ((words.len() * word_share).div_ceil(100)).clamp(1, words.len().saturating_sub(1));
        let text = words[..word_count].join(" ");
        if partials
            .last()
            .is_none_or(|prior: &FixturePartial| prior.text != text)
        {
            partials.push(FixturePartial { at_ms, text });
            prior_at = at_ms;
        }
    }
    partials
}

fn stable_seed(fixture: &EvalFixture, utterance_sequence: u64) -> u64 {
    let mut hash = EVAL_FIXED_SEED ^ utterance_sequence;
    for byte in fixture.id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

struct StableRng(u64);

impl StableRng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_bounded(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % bound.max(1)
    }
}

fn score_observations(
    fixture: &EvalFixture,
    events: &[ExpandedEvent],
    snapshots: &BTreeMap<u64, RequestSnapshot>,
    observations: &mut [NudgeObservation],
) -> QualityMetrics {
    let mut matched = BTreeSet::new();
    let mut useful = 0;
    let mut duplicates = 0;
    let mut fingerprints = BTreeMap::<String, u64>::new();
    let mut partial_grounded = 0;
    let mut contradictions = 0;

    for observation in observations.iter_mut() {
        if let Some(opportunity) = fixture.labels.opportunities.iter().find(|opportunity| {
            !matched.contains(&opportunity.id) && opportunity_matches(opportunity, observation)
        }) {
            matched.insert(opportunity.id.clone());
            observation.matched_opportunity = Some(opportunity.id.clone());
            useful += 1;
        } else if fixture
            .labels
            .opportunities
            .iter()
            .any(|opportunity| opportunity_matches(opportunity, observation))
        {
            observation.duplicate_or_nagging = true;
        }

        let fingerprint = format!("{:?}:{}", observation.kind, normalize(&observation.text));
        if fingerprints.get(&fingerprint).is_some_and(|prior_ms| {
            observation.delivered_at_ms.saturating_sub(*prior_ms) <= DUPLICATE_WINDOW_MS
        }) {
            observation.duplicate_or_nagging = true;
        }
        fingerprints.insert(fingerprint, observation.delivered_at_ms);
        if observation.duplicate_or_nagging {
            duplicates += 1;
        }

        if observation.grounded_in_partial {
            partial_grounded += 1;
            observation.contradiction_after_revision = snapshots
                .get(&observation.evidence_revision)
                .is_some_and(|snapshot| {
                    fixture.labels.revision_reversals.iter().any(|reversal| {
                        snapshot.request.utterances.iter().any(|utterance| {
                            utterance.utterance_sequence == reversal.utterance_sequence
                                && utterance.update_kind == TranscriptUpdateKind::Partial
                                && contains(&utterance.text, &reversal.from_contains)
                        }) && events.iter().any(|event| {
                            event.utterance_sequence == reversal.utterance_sequence
                                && event.evidence_revision > observation.evidence_revision
                                && event.context_ready_ms > observation.delivered_at_ms
                                && contains(&event.text, &reversal.to_contains)
                        })
                    })
                });
            if observation.contradiction_after_revision {
                contradictions += 1;
            }
        }
    }

    let stale = observations
        .iter()
        .filter(|observation| observation.stale_at_delivery)
        .count();
    let clean_no_opportunity_ranges = fixture
        .labels
        .no_opportunity_ranges
        .iter()
        .filter(|range| {
            !observations.iter().any(|observation| {
                observation.evidence_time_ms >= range.start_ms
                    && observation.evidence_time_ms <= range.end_ms
            })
        })
        .count();

    QualityMetrics {
        useful_nudge_precision: rate(useful, observations.len(), 1.0),
        opportunity_recall: rate(matched.len(), fixture.labels.opportunities.len(), 1.0),
        stale_nudge_rate: rate(stale, observations.len(), 0.0),
        contradiction_after_revision_rate: rate(contradictions, partial_grounded, 0.0),
        duplicate_nagging_rate: rate(duplicates, observations.len(), 0.0),
        no_nudge_quality: rate(
            clean_no_opportunity_ranges,
            fixture.labels.no_opportunity_ranges.len(),
            1.0,
        ),
    }
}

fn opportunity_matches(opportunity: &OpportunityLabel, observation: &NudgeObservation) -> bool {
    observation.evidence_time_ms >= opportunity.start_ms
        && observation.evidence_time_ms <= opportunity.end_ms
        && opportunity.kind.is_none_or(|kind| kind == observation.kind)
        && opportunity.match_any.iter().any(|term| {
            contains(&observation.text, term) || contains(&observation.source_chip, term)
        })
}

fn contains(haystack: &str, needle: &str) -> bool {
    normalize(haystack).contains(&normalize(needle))
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn rate(numerator: usize, denominator: usize, empty_rate: f64) -> RateMetric {
    RateMetric {
        numerator,
        denominator,
        rate: if denominator == 0 {
            empty_rate
        } else {
            numerator as f64 / denominator as f64
        },
    }
}

fn aggregate_quality<'a>(metrics: impl Iterator<Item = &'a QualityMetrics>) -> QualityMetrics {
    let metrics = metrics.collect::<Vec<_>>();
    let sum = |select: fn(&QualityMetrics) -> &RateMetric, empty_rate| {
        let numerator = metrics.iter().map(|metric| select(metric).numerator).sum();
        let denominator = metrics
            .iter()
            .map(|metric| select(metric).denominator)
            .sum();
        rate(numerator, denominator, empty_rate)
    };
    QualityMetrics {
        useful_nudge_precision: sum(|metric| &metric.useful_nudge_precision, 1.0),
        opportunity_recall: sum(|metric| &metric.opportunity_recall, 1.0),
        stale_nudge_rate: sum(|metric| &metric.stale_nudge_rate, 0.0),
        contradiction_after_revision_rate: sum(
            |metric| &metric.contradiction_after_revision_rate,
            0.0,
        ),
        duplicate_nagging_rate: sum(|metric| &metric.duplicate_nagging_rate, 0.0),
        no_nudge_quality: sum(|metric| &metric.no_nudge_quality, 1.0),
    }
}

fn aggregate_latency(records: &[LatencyRecord]) -> BTreeMap<String, LatencyPercentiles> {
    let mut stages = BTreeMap::<String, Vec<u64>>::new();
    for record in records {
        push_stage(
            &mut stages,
            "audio_to_partial",
            Some(
                record
                    .partial_published_us
                    .saturating_sub(record.audio_received_us),
            ),
        );
        push_stage(
            &mut stages,
            "partial_to_trigger",
            Some(
                record
                    .trigger_us
                    .saturating_sub(record.partial_published_us),
            ),
        );
        push_stage(
            &mut stages,
            "trigger_to_context",
            Some(record.context_ready_us.saturating_sub(record.trigger_us)),
        );
        push_stage(
            &mut stages,
            "context_to_model",
            record
                .model_request_us
                .map(|model| model.saturating_sub(record.context_ready_us)),
        );
        push_stage(
            &mut stages,
            "model_to_first_token",
            record
                .model_request_us
                .zip(record.first_token_us)
                .map(|(model, first_token)| first_token.saturating_sub(model)),
        );
        push_stage(
            &mut stages,
            "first_token_to_nudge",
            record
                .first_token_us
                .zip(record.nudge_us)
                .map(|(first_token, nudge)| nudge.saturating_sub(first_token)),
        );
        push_stage(
            &mut stages,
            "model_to_nudge",
            record
                .model_request_us
                .zip(record.nudge_us)
                .map(|(model, nudge)| nudge.saturating_sub(model)),
        );
        push_stage(&mut stages, "audio_to_nudge", record.nudge_us);
    }
    for name in [
        "audio_to_partial",
        "partial_to_trigger",
        "trigger_to_context",
        "context_to_model",
        "model_to_first_token",
        "first_token_to_nudge",
        "model_to_nudge",
        "audio_to_nudge",
    ] {
        stages.entry(name.into()).or_default();
    }
    stages
        .into_iter()
        .map(|(name, mut samples)| {
            samples.sort_unstable();
            let percentiles = LatencyPercentiles {
                samples: samples.len(),
                p50_ms: percentile(&samples, 50).map(micros_to_ms),
                p95_ms: percentile(&samples, 95).map(micros_to_ms),
            };
            (name, percentiles)
        })
        .collect()
}

fn push_stage(stages: &mut BTreeMap<String, Vec<u64>>, name: &str, sample: Option<u64>) {
    if let Some(sample) = sample {
        stages.entry(name.into()).or_default().push(sample);
    }
}

fn percentile(sorted: &[u64], percentile: usize) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (percentile.saturating_mul(sorted.len())).div_ceil(100);
    sorted.get(rank.saturating_sub(1)).copied()
}

fn micros_to_ms(micros: u64) -> f64 {
    micros as f64 / 1_000.0
}

fn threshold_failures(
    quality: &QualityMetrics,
    latency: &BTreeMap<String, LatencyPercentiles>,
    thresholds: &BaselineThresholds,
) -> Vec<String> {
    let mut failures = Vec::new();
    check_min(
        &mut failures,
        "useful_nudge_precision",
        quality.useful_nudge_precision.rate,
        thresholds.min_useful_nudge_precision,
    );
    check_min(
        &mut failures,
        "opportunity_recall",
        quality.opportunity_recall.rate,
        thresholds.min_opportunity_recall,
    );
    check_max(
        &mut failures,
        "stale_nudge_rate",
        quality.stale_nudge_rate.rate,
        thresholds.max_stale_nudge_rate,
    );
    check_max(
        &mut failures,
        "contradiction_after_revision_rate",
        quality.contradiction_after_revision_rate.rate,
        thresholds.max_contradiction_after_revision_rate,
    );
    check_max(
        &mut failures,
        "duplicate_nagging_rate",
        quality.duplicate_nagging_rate.rate,
        thresholds.max_duplicate_nagging_rate,
    );
    check_min(
        &mut failures,
        "no_nudge_quality",
        quality.no_nudge_quality.rate,
        thresholds.min_no_nudge_quality,
    );
    check_optional_max(
        &mut failures,
        "model_to_first_token_p95_ms",
        latency
            .get("model_to_first_token")
            .and_then(|metric| metric.p95_ms),
        thresholds.max_model_to_first_token_p95_ms,
    );
    check_optional_max(
        &mut failures,
        "audio_to_nudge_p95_ms",
        latency
            .get("audio_to_nudge")
            .and_then(|metric| metric.p95_ms),
        thresholds.max_audio_to_nudge_p95_ms,
    );
    failures
}

fn check_min(failures: &mut Vec<String>, name: &str, actual: f64, minimum: f64) {
    if actual < minimum {
        failures.push(format!("{name} {actual:.4} is below minimum {minimum:.4}"));
    }
}

fn check_max(failures: &mut Vec<String>, name: &str, actual: f64, maximum: f64) {
    if actual > maximum {
        failures.push(format!("{name} {actual:.4} exceeds maximum {maximum:.4}"));
    }
}

fn check_optional_max(failures: &mut Vec<String>, name: &str, actual: Option<f64>, maximum: f64) {
    match actual {
        Some(actual) if actual > maximum => {
            failures.push(format!("{name} {actual:.3} exceeds maximum {maximum:.3}"));
        }
        None => failures.push(format!("{name} has no samples")),
        Some(_) => {}
    }
}

pub fn render_report_table(report: &EvalReport) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "Copilot eval v{} | {:?} | seed {} | {}\n",
        report.suite_version, report.mode, report.fixed_seed, report.provider
    ));
    output.push_str(
        "fixture                         nudges precision stale contradict duplicate no-nudge audio->nudge p95\n",
    );
    for fixture in &report.fixtures {
        output.push_str(&format!(
            "{:<31} {:>6} {:>8.1}% {:>5.1}% {:>9.1}% {:>8.1}% {:>8.1}% {:>16}\n",
            truncate(&fixture.fixture_id, 31),
            fixture.nudges.len(),
            fixture.quality.useful_nudge_precision.rate * 100.0,
            fixture.quality.stale_nudge_rate.rate * 100.0,
            fixture.quality.contradiction_after_revision_rate.rate * 100.0,
            fixture.quality.duplicate_nagging_rate.rate * 100.0,
            fixture.quality.no_nudge_quality.rate * 100.0,
            format_latency(
                fixture
                    .latency
                    .get("audio_to_nudge")
                    .and_then(|metric| metric.p95_ms)
            ),
        ));
    }
    output.push_str(&format!(
        "summary                         {:>6} {:>8.1}% {:>5.1}% {:>9.1}% {:>8.1}% {:>8.1}% {:>16}\n",
        report.summary.nudges,
        report.summary.quality.useful_nudge_precision.rate * 100.0,
        report.summary.quality.stale_nudge_rate.rate * 100.0,
        report.summary.quality.contradiction_after_revision_rate.rate * 100.0,
        report.summary.quality.duplicate_nagging_rate.rate * 100.0,
        report.summary.quality.no_nudge_quality.rate * 100.0,
        format_latency(
            report
                .summary
                .latency
                .get("audio_to_nudge")
                .and_then(|metric| metric.p95_ms)
        ),
    ));
    output.push_str(if report.summary.baseline_passed {
        "baseline: PASS\n"
    } else {
        "baseline: FAIL\n"
    });
    for failure in &report.summary.baseline_failures {
        output.push_str(&format!("  - {failure}\n"));
    }
    output
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn format_latency(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".into(), |value| format!("{value:.1} ms"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_suite_is_deterministic_and_clears_baseline() {
        let options = EvalOptions {
            mode: ReplayMode::Accelerated,
        };
        let first = run_builtin_suite(options).expect("first deterministic replay");
        let second = run_builtin_suite(options).expect("second deterministic replay");

        assert_eq!(first, second);
        assert!(
            first.summary.baseline_passed,
            "baseline failures: {:?}",
            first.summary.baseline_failures
        );
        assert_eq!(first.summary.fixtures, 4);
        assert!(first
            .fixtures
            .iter()
            .all(|fixture| fixture.content_origin == FixtureContentOrigin::Synthetic));
    }

    #[test]
    fn scoring_detects_duplicates_staleness_and_later_reversal() {
        let fixture = EvalFixture {
            schema_version: EVAL_SUITE_VERSION,
            id: "metric-positive-control".into(),
            description: "synthetic metric positive control".into(),
            content_origin: FixtureContentOrigin::Synthetic,
            goal: "test metrics".into(),
            synthesize_partials: false,
            transcript: vec![FixtureUtterance {
                utterance_sequence: 1,
                source: default_source(),
                offset_ms: 0,
                duration_ms: 500,
                final_text: "Reject".into(),
                partials: vec![FixturePartial {
                    at_ms: 100,
                    text: "Approve".into(),
                }],
            }],
            labels: FixtureLabels {
                opportunities: vec![OpportunityLabel {
                    id: "decision".into(),
                    start_ms: 0,
                    end_ms: 500,
                    kind: Some(NudgeKind::Say),
                    match_any: vec!["approve".into()],
                }],
                no_opportunity_ranges: vec![],
                revision_reversals: vec![RevisionReversal {
                    utterance_sequence: 1,
                    from_contains: "Approve".into(),
                    to_contains: "Reject".into(),
                }],
            },
            mock_script: vec![],
        };
        let events = expand_fixture(&fixture);
        let request = CopilotRequest {
            goal: fixture.goal.clone(),
            session_epoch: 1,
            evidence_revision: 1,
            evidence_utterance_sequence: 1,
            evidence_utterance_revision: 1,
            update_kind: TranscriptUpdateKind::Partial,
            utterances: vec![CopilotUtterance {
                utterance_sequence: 1,
                revision: 1,
                update_kind: TranscriptUpdateKind::Partial,
                source: default_source(),
                text: "Approve".into(),
                speaker: None,
                speaker_verified: false,
                offset_ms: 0,
                duration_ms: 100,
            }],
            battle_card: BattleCard::empty(),
        };
        let snapshots = BTreeMap::from([(
            1,
            RequestSnapshot {
                request,
                evidence_time_ms: 100,
            },
        )]);
        let observation = || NudgeObservation {
            kind: NudgeKind::Say,
            text: "Approve now".into(),
            source_chip: "approve".into(),
            evidence_revision: 1,
            evidence_time_ms: 100,
            delivered_at_ms: 150,
            grounded_in_partial: true,
            stale_at_delivery: true,
            contradiction_after_revision: false,
            duplicate_or_nagging: false,
            matched_opportunity: None,
        };
        let mut observations = vec![observation(), observation()];

        let quality = score_observations(&fixture, &events, &snapshots, &mut observations);

        assert_eq!(quality.stale_nudge_rate.rate, 1.0);
        assert_eq!(quality.contradiction_after_revision_rate.rate, 1.0);
        assert_eq!(quality.duplicate_nagging_rate.numerator, 1);
        assert_eq!(quality.useful_nudge_precision.numerator, 1);
    }
}
