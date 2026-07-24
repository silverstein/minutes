//! Deterministic, provider-neutral replay and fault injection for Sidekick.
//!
//! This module deliberately drives the public [`LiveSidekickEngine`] API. It
//! does not duplicate the reducer or publication policy. A scripted backend is
//! used only at the provider seam so failures and completion order can be
//! controlled without a network, microphone, screen, or human operator.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use minutes_core::live_sidekick::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

pub const LIVE_SIDEKICK_ENGINE_EVAL_VERSION: &str = "1";
pub const LIVE_SIDEKICK_ENGINE_EVAL_SEED: u64 = 0x4d49_4e55_5445_5301;

const MAX_WINDOW_CHARS: usize = 4_096;
const FIXED_FIRST_TOKEN_MS: u64 = 120;
const FIXED_GENERATION_MS: u64 = 600;
const FIXED_VERIFICATION_MS: u64 = 120;
const MAX_DIAGNOSTIC_CHARS: usize = 240;
const STRATEGIST_WARMUP_SEQUENCE: u64 = u64::MAX;
const VERIFIER_WARMUP_SEQUENCE: u64 = u64::MAX - 1;

/// Bounded proof artifact emitted by the replay harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiveSidekickEngineEvalReport {
    pub schema_version: &'static str,
    pub fixed_seed: u64,
    pub passed: bool,
    pub reproducible: bool,
    pub deterministic_digest: String,
    pub summary: EvalSummary,
    pub coverage: EvalCoverage,
    pub scenarios: Vec<EvalScenario>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvalSummary {
    pub scenarios_passed: usize,
    pub scenarios_total: usize,
    pub assertions_passed: usize,
    pub assertions_total: usize,
}

/// Honest boundary between production paths exercised here and deferred native
/// adapter acceptance. This travels with every report so a green replay cannot
/// be mistaken for end-to-end microphone or diarization proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvalCoverage {
    pub production_engine: bool,
    pub production_reducer: bool,
    pub production_evidence_window: bool,
    pub production_publication_gate: bool,
    pub production_verification_gate: bool,
    pub provider_contract: &'static str,
    pub deterministic_provider_faults: bool,
    pub native_audio_capture: bool,
    pub native_asr: bool,
    pub native_diarization: bool,
    pub native_screen_permission_adapter: bool,
    pub real_cloud_provider: bool,
    pub release_ready_from_this_report_alone: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvalScenario {
    pub id: &'static str,
    pub passed: bool,
    pub simulated_total_ms: u64,
    pub assertions: Vec<EvalAssertion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvalAssertion {
    pub name: &'static str,
    pub passed: bool,
    /// Bounded, content-free diagnostic suitable for CI artifacts.
    pub observed: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendLane {
    Strategist,
    Verifier,
}

impl BackendLane {
    fn label(self) -> &'static str {
        match self {
            Self::Strategist => "strategist",
            Self::Verifier => "verifier",
        }
    }
}

struct PendingTurn {
    id: ReasoningTurnId,
    request: ReasoningTurnRequest,
    sink: Arc<dyn ReasoningEventSink>,
    completed: bool,
    interrupted: bool,
}

#[derive(Default)]
struct ScriptState {
    sessions_started: usize,
    sessions_closed: usize,
    turns: Vec<PendingTurn>,
    turns_steered: usize,
    turns_interrupted: usize,
}

#[derive(Clone)]
struct ScriptedBackend {
    state: Arc<Mutex<ScriptState>>,
    lane: BackendLane,
    steerable: bool,
}

impl ScriptedBackend {
    fn new(lane: BackendLane, steerable: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(ScriptState::default())),
            lane,
            steerable,
        }
    }

    fn state(&self) -> MutexGuard<'_, ScriptState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn requests(&self) -> Vec<ReasoningTurnRequest> {
        self.state()
            .turns
            .iter()
            .map(|turn| turn.request.clone())
            .collect()
    }

    fn sessions_started(&self) -> usize {
        self.state().sessions_started
    }

    fn sessions_closed(&self) -> usize {
        self.state().sessions_closed
    }

    fn turns_interrupted(&self) -> usize {
        self.state().turns_interrupted
    }

    fn turns_steered(&self) -> usize {
        self.state().turns_steered
    }

    fn complete_candidate(
        &self,
        index: usize,
        candidate: InterventionCandidate,
    ) -> Result<(), String> {
        let (id, invocation, sink) = self.take_completion(index)?;
        sink.on_event(ReasoningStreamEvent::Completed {
            turn_id: id,
            invocation,
            result: ReasoningTurnResult {
                text: serde_json::to_string(&candidate).map_err(|error| error.to_string())?,
                first_token_ms: Some(FIXED_FIRST_TOKEN_MS),
                total_ms: FIXED_GENERATION_MS,
            },
        });
        Ok(())
    }

    fn complete_verdict(
        &self,
        index: usize,
        verdict: EvidenceVerificationVerdict,
    ) -> Result<(), String> {
        let (id, invocation, sink) = self.take_completion(index)?;
        sink.on_event(ReasoningStreamEvent::Completed {
            turn_id: id,
            invocation,
            result: ReasoningTurnResult {
                text: serde_json::to_string(&verdict).map_err(|error| error.to_string())?,
                first_token_ms: Some(40),
                total_ms: FIXED_VERIFICATION_MS,
            },
        });
        Ok(())
    }

    fn fail(&self, index: usize, error: ReasoningError) -> Result<(), String> {
        let (id, invocation, sink) = self.take_completion(index)?;
        sink.on_event(ReasoningStreamEvent::Failed {
            turn_id: id,
            invocation,
            error,
        });
        Ok(())
    }

    fn take_completion(
        &self,
        index: usize,
    ) -> Result<
        (
            ReasoningTurnId,
            InvocationIdentity,
            Arc<dyn ReasoningEventSink>,
        ),
        String,
    > {
        let mut state = self.state();
        let turn = state
            .turns
            .get_mut(index)
            .ok_or_else(|| format!("{} turn {index} does not exist", self.lane.label()))?;
        if turn.completed {
            return Err(format!(
                "{} turn {index} already completed",
                self.lane.label()
            ));
        }
        turn.completed = true;
        Ok((
            turn.id.clone(),
            turn.request.invocation,
            Arc::clone(&turn.sink),
        ))
    }
}

impl PersistentReasoningBackend for ScriptedBackend {
    fn descriptor(&self) -> ReasoningBackendDescriptor {
        ReasoningBackendDescriptor {
            provider: "minutes-eval-scripted".into(),
            model: self.lane.label().into(),
            privacy: ReasoningPrivacyClass::OnDevice,
            persistent: true,
            steerable: self.steerable,
            streaming: true,
            image_input: true,
        }
    }

    fn start_session(
        &self,
        config: ReasoningSessionConfig,
    ) -> Result<Box<dyn PersistentReasoningSession>, ReasoningError> {
        config.validate()?;
        let mut state = self.state();
        state.sessions_started = state.sessions_started.saturating_add(1);
        let sequence = state.sessions_started;
        drop(state);
        Ok(Box::new(ScriptedSession {
            id: ReasoningSessionId::new(format!("eval-{}-session-{sequence}", self.lane.label())),
            backend: self.clone(),
            closed: false,
        }))
    }
}

struct ScriptedSession {
    id: ReasoningSessionId,
    backend: ScriptedBackend,
    closed: bool,
}

impl PersistentReasoningSession for ScriptedSession {
    fn id(&self) -> &ReasoningSessionId {
        &self.id
    }

    fn start_turn(
        &mut self,
        request: ReasoningTurnRequest,
        sink: Arc<dyn ReasoningEventSink>,
    ) -> Result<ReasoningTurnId, ReasoningError> {
        let is_strategist_warmup = request.invocation.sequence == STRATEGIST_WARMUP_SEQUENCE;
        let is_verifier_warmup = request.invocation.sequence == VERIFIER_WARMUP_SEQUENCE;
        if is_strategist_warmup || is_verifier_warmup {
            let id = ReasoningTurnId::new(format!("eval-{}-warmup", self.backend.lane.label()));
            let text = if is_strategist_warmup {
                serde_json::to_string(&silent_candidate())
            } else {
                serde_json::to_string(&allow_verdict())
            }
            .map_err(|error| {
                ReasoningError::new(ReasoningErrorKind::Protocol, error.to_string(), false)
            })?;
            sink.on_event(ReasoningStreamEvent::Completed {
                turn_id: id.clone(),
                invocation: request.invocation,
                result: ReasoningTurnResult {
                    text,
                    first_token_ms: Some(1),
                    total_ms: 1,
                },
            });
            return Ok(id);
        }

        let mut state = self.backend.state();
        let id = ReasoningTurnId::new(format!(
            "eval-{}-turn-{}",
            self.backend.lane.label(),
            state.turns.len() + 1
        ));
        state.turns.push(PendingTurn {
            id: id.clone(),
            request,
            sink,
            completed: false,
            interrupted: false,
        });
        Ok(id)
    }

    fn steer_turn(
        &mut self,
        turn_id: &ReasoningTurnId,
        request: ReasoningTurnRequest,
    ) -> Result<(), ReasoningError> {
        if !self.backend.steerable {
            return Err(ReasoningError::new(
                ReasoningErrorKind::Unavailable,
                "scripted backend is non-steerable",
                true,
            ));
        }
        let mut state = self.backend.state();
        let turn = state
            .turns
            .iter_mut()
            .find(|turn| &turn.id == turn_id && !turn.completed)
            .ok_or_else(|| ReasoningError::invalid_request("unknown scripted turn"))?;
        turn.request = request;
        state.turns_steered = state.turns_steered.saturating_add(1);
        Ok(())
    }

    fn interrupt_turn(&mut self, turn_id: &ReasoningTurnId) -> Result<(), ReasoningError> {
        let mut state = self.backend.state();
        if let Some(turn) = state
            .turns
            .iter_mut()
            .find(|turn| &turn.id == turn_id && !turn.completed)
        {
            turn.interrupted = true;
            state.turns_interrupted = state.turns_interrupted.saturating_add(1);
        }
        Ok(())
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        let mut state = self.backend.state();
        state.sessions_closed = state.sessions_closed.saturating_add(1);
    }
}

struct Fixture {
    engine: LiveSidekickEngine,
    strategist: ScriptedBackend,
    verifier: ScriptedBackend,
}

fn fixture(steerable: bool, max_transcript_items: usize) -> Result<Fixture, String> {
    let strategist = ScriptedBackend::new(BackendLane::Strategist, steerable);
    let verifier = ScriptedBackend::new(BackendLane::Verifier, false);
    let mut engine = LiveSidekickEngine::new_with_verifier_backend(
        "eval-assistance".into(),
        AssistanceSurface::NativeRecall,
        UserRole::DecisionMaker,
        AssistancePosture::Strategist,
        Arc::new(strategist.clone()),
        Arc::new(verifier.clone()),
        LiveSidekickEngineConfig {
            base_instructions: "Provide bounded meeting strategy.".into(),
            developer_instructions: "Return the provider-neutral contract.".into(),
            prepared_context: "Synthetic evaluation context only.".into(),
            max_window_chars: MAX_WINDOW_CHARS,
            max_transcript_items,
        },
    )
    .map_err(|error| error.to_string())?;
    engine
        .start_capture("eval-capture".into(), CaptureMode::Recording)
        .map_err(|error| error.to_string())?;
    Ok(Fixture {
        engine,
        strategist,
        verifier,
    })
}

fn observe(
    engine: &mut LiveSidekickEngine,
    id: &str,
    text: &str,
    speaker: Option<&str>,
) -> Result<(), String> {
    let reduction = engine
        .observe_transcript(ReasoningTranscriptEvidence {
            evidence_id: id.into(),
            text: text.into(),
            speaker_label: speaker.map(str::to_owned),
            speaker_verified: false,
            offset_ms: 0,
            duration_ms: 100,
        })
        .map_err(|error| error.to_string())?;
    if !reduction.accepted {
        return Err(format!("transcript {id} was rejected"));
    }
    Ok(())
}

fn candidate(
    text: &str,
    evidence_ids: &[&str],
    visual_evidence_ids: &[&str],
) -> InterventionCandidate {
    InterventionCandidate {
        decision: InterventionDecision::Speak,
        kind: Some("strategy".into()),
        text: Some(text.into()),
        evidence_ids: evidence_ids.iter().copied().map(EvidenceId::new).collect(),
        visual_evidence_ids: visual_evidence_ids
            .iter()
            .copied()
            .map(EvidenceId::new)
            .collect(),
        claims_visual_observation: !visual_evidence_ids.is_empty(),
        confidence: 96,
    }
}

fn silent_candidate() -> InterventionCandidate {
    InterventionCandidate {
        decision: InterventionDecision::Silent,
        kind: None,
        text: None,
        evidence_ids: Vec::new(),
        visual_evidence_ids: Vec::new(),
        claims_visual_observation: false,
        confidence: 100,
    }
}

fn allow_verdict() -> EvidenceVerificationVerdict {
    EvidenceVerificationVerdict {
        decision: EvidenceVerificationDecision::Allow,
        reason_code: EvidenceVerificationReason::Supported,
    }
}

fn reject_verdict(reason_code: EvidenceVerificationReason) -> EvidenceVerificationVerdict {
    EvidenceVerificationVerdict {
        decision: EvidenceVerificationDecision::Reject,
        reason_code,
    }
}

fn assertion(name: &'static str, passed: bool, observed: impl Into<String>) -> EvalAssertion {
    let observed = observed.into();
    let observed = if observed.chars().count() > MAX_DIAGNOSTIC_CHARS {
        observed
            .chars()
            .take(MAX_DIAGNOSTIC_CHARS)
            .collect::<String>()
    } else {
        observed
    };
    EvalAssertion {
        name,
        passed,
        observed,
    }
}

fn scenario(
    id: &'static str,
    simulated_total_ms: u64,
    run: impl FnOnce() -> Result<Vec<EvalAssertion>, String>,
) -> EvalScenario {
    match run() {
        Ok(assertions) => EvalScenario {
            id,
            passed: assertions.iter().all(|item| item.passed),
            simulated_total_ms,
            assertions,
        },
        Err(error) => EvalScenario {
            id,
            passed: false,
            simulated_total_ms,
            assertions: vec![assertion("scenario_completed", false, error)],
        },
    }
}

fn exact_screen_publication() -> EvalScenario {
    scenario("exact_screen_publication", 720, || {
        let mut fixture = fixture(true, 4)?;
        observe(
            &mut fixture.engine,
            "transcript-a",
            "The synthetic review is on the final decision.",
            Some("PARTICIPANT_A"),
        )?;
        let png = BASE64
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
            .map_err(|error| error.to_string())?;
        let screen_reduction = fixture
            .engine
            .observe_screen_bytes(
                "screen-a".into(),
                PathBuf::from("/synthetic/minutes/screen-a.png"),
                png.clone(),
            )
            .map_err(|error| error.to_string())?;
        let turn = fixture
            .engine
            .send_user("What should I focus on?")
            .map_err(|error| error.to_string())?;
        let receipt = fixture.engine.foreground_evidence_receipt(&turn);
        fixture.strategist.complete_candidate(
            0,
            candidate(
                "The exact-session image and transcript support focusing on the final decision.",
                &["transcript-a"],
                &["screen-a"],
            ),
        )?;
        fixture.engine.pump();
        fixture.verifier.complete_verdict(0, allow_verdict())?;
        let publications = fixture.engine.take_publications();
        let requests = fixture.strategist.requests();
        let exact_bytes = requests
            .first()
            .and_then(|request| request.window.latest_image.as_ref())
            .is_some_and(|image| {
                image.png_bytes == png && image.evidence_id.as_str() == "screen-a"
            });
        let receipt_exact = receipt.is_some_and(|receipt| {
            receipt
                .visual_evidence_ids
                .iter()
                .any(|id| id.as_str() == "screen-a")
                && receipt
                    .transcript_evidence_ids
                    .iter()
                    .any(|id| id.as_str() == "transcript-a")
        });
        let verified = publications.first().is_some_and(|publication| {
            publication
                .evidence_verification
                .verdict
                .allows_publication()
                && publication
                    .candidate
                    .visual_evidence_ids
                    .iter()
                    .any(|id| id.as_str() == "screen-a")
        });
        fixture
            .engine
            .stop_capture()
            .map_err(|error| error.to_string())?;
        Ok(vec![
            assertion(
                "screen_evidence_accepted",
                screen_reduction.accepted,
                format!("accepted={}", screen_reduction.accepted),
            ),
            assertion(
                "exact_selected_bytes_reached_provider",
                exact_bytes,
                "sha-bound PNG",
            ),
            assertion(
                "foreground_receipt_is_exact_session",
                receipt_exact,
                "2 evidence lanes",
            ),
            assertion(
                "independent_verifier_gated_publication",
                publications.len() == 1 && verified,
                format!("publications={}", publications.len()),
            ),
        ])
    })
}

fn correction_during_verification() -> EvalScenario {
    scenario("correction_during_verification", 2_160, || {
        let mut fixture = fixture(true, 6)?;
        observe(
            &mut fixture.engine,
            "transcript-old",
            "The initial synthetic position is proceed.",
            Some("PARTICIPANT_A"),
        )?;
        fixture
            .engine
            .send_user("What is the current decision?")
            .map_err(|error| error.to_string())?;
        fixture.strategist.complete_candidate(
            0,
            candidate(
                "The initial evidence supports proceeding.",
                &["transcript-old"],
                &[],
            ),
        )?;
        fixture.engine.pump();

        observe(
            &mut fixture.engine,
            "transcript-correction",
            "Correction: the current synthetic position is stop.",
            Some("PARTICIPANT_A"),
        )?;
        fixture.verifier.complete_verdict(0, allow_verdict())?;
        fixture.engine.pump();
        fixture
            .verifier
            .complete_verdict(1, reject_verdict(EvidenceVerificationReason::Contradiction))?;
        fixture.engine.pump();
        let stale_publications = fixture.engine.take_publications();

        fixture.strategist.complete_candidate(
            1,
            candidate(
                "The correction changes the current decision to stop.",
                &["transcript-correction"],
                &[],
            ),
        )?;
        fixture.engine.pump();
        fixture.verifier.complete_verdict(2, allow_verdict())?;
        let publications = fixture.engine.take_publications();
        let correction_won = publications.first().is_some_and(|publication| {
            publication
                .candidate
                .evidence_ids
                .iter()
                .any(|id| id.as_str() == "transcript-correction")
                && !publication
                    .candidate
                    .evidence_ids
                    .iter()
                    .any(|id| id.as_str() == "transcript-old")
        });
        let fresh_request = fixture.strategist.requests().get(1).is_some_and(|request| {
            request
                .window
                .transcript
                .iter()
                .any(|item| item.evidence_id.as_str() == "transcript-correction")
        });
        fixture
            .engine
            .stop_capture()
            .map_err(|error| error.to_string())?;
        Ok(vec![
            assertion(
                "stale_candidate_never_published",
                stale_publications.is_empty(),
                format!("stale_publications={}", stale_publications.len()),
            ),
            assertion(
                "verification_refreshed_then_rejected",
                fixture.verifier.requests().len() == 3,
                format!("verifier_turns={}", fixture.verifier.requests().len()),
            ),
            assertion(
                "generation_restarted_on_fresh_window",
                fresh_request,
                "revision advanced",
            ),
            assertion(
                "correction_won_publication",
                publications.len() == 1 && correction_won,
                format!("publications={}", publications.len()),
            ),
        ])
    })
}

fn provider_failure_and_recovery() -> EvalScenario {
    scenario("provider_failure_and_recovery", 720, || {
        let mut fixture = fixture(true, 4)?;
        observe(
            &mut fixture.engine,
            "transcript-a",
            "The synthetic decision remains open.",
            None,
        )?;
        fixture
            .engine
            .send_user("Give me the current strategy.")
            .map_err(|error| error.to_string())?;
        fixture.strategist.fail(
            0,
            ReasoningError::new(
                ReasoningErrorKind::Unavailable,
                "synthetic network unavailable",
                true,
            ),
        )?;
        let failures = fixture.engine.take_failures();
        let capture_survived = fixture.engine.session.phase == AssistancePhase::Ready;
        fixture
            .engine
            .recover_backend()
            .map_err(|error| error.to_string())?;
        fixture
            .engine
            .send_user("Retry with the current evidence.")
            .map_err(|error| error.to_string())?;
        fixture.strategist.complete_candidate(
            1,
            candidate(
                "The current evidence supports keeping the decision open.",
                &["transcript-a"],
                &[],
            ),
        )?;
        fixture.engine.pump();
        fixture.verifier.complete_verdict(0, allow_verdict())?;
        let publications = fixture.engine.take_publications();
        let sessions_replaced =
            fixture.strategist.sessions_started() == 2 && fixture.verifier.sessions_started() == 2;
        fixture
            .engine
            .stop_capture()
            .map_err(|error| error.to_string())?;
        Ok(vec![
            assertion(
                "provider_failure_is_classified",
                failures.len() == 1
                    && failures[0].error.kind == ReasoningErrorKind::Unavailable
                    && failures[0].error.retryable,
                format!("failures={}", failures.len()),
            ),
            assertion(
                "capture_survives_provider_failure",
                capture_survived,
                "phase=ready",
            ),
            assertion(
                "recovery_replaces_provider_epoch",
                sessions_replaced,
                "sessions=2+2",
            ),
            assertion(
                "recovered_turn_publishes",
                publications.len() == 1,
                format!("publications={}", publications.len()),
            ),
        ])
    })
}

fn unavailable_screen_fails_closed() -> EvalScenario {
    scenario("unavailable_screen_fails_closed", 720, || {
        let mut fixture = fixture(true, 4)?;
        observe(
            &mut fixture.engine,
            "transcript-a",
            "The synthetic transcript is still available.",
            None,
        )?;
        let permission_adapter_result = fixture.engine.observe_screen_bytes(
            "screen-denied".into(),
            PathBuf::from("/synthetic/minutes/screen-denied.png"),
            Vec::new(),
        );
        fixture
            .engine
            .send_user("What does the evidence say?")
            .map_err(|error| error.to_string())?;
        fixture.strategist.complete_candidate(
            0,
            candidate(
                "The screen shows an unsupported synthetic detail.",
                &["transcript-a"],
                &["screen-denied"],
            ),
        )?;
        let visual_failures = fixture.engine.take_failures();
        let no_visual_publication = fixture.engine.take_publications().is_empty();

        fixture
            .engine
            .send_user("Answer from the transcript only.")
            .map_err(|error| error.to_string())?;
        fixture.strategist.complete_candidate(
            1,
            candidate(
                "The transcript says the synthetic transcript remains available.",
                &["transcript-a"],
                &[],
            ),
        )?;
        fixture.engine.pump();
        fixture.verifier.complete_verdict(0, allow_verdict())?;
        let publications = fixture.engine.take_publications();
        let transcript_only = publications.first().is_some_and(|publication| {
            !publication.candidate.claims_visual_observation
                && publication.candidate.visual_evidence_ids.is_empty()
        });
        let permission_observed = match permission_adapter_result.as_ref() {
            Ok(reduction) => format!("accepted={}", reduction.accepted),
            Err(error) => format!("kind={:?}", error.kind),
        };
        fixture
            .engine
            .stop_capture()
            .map_err(|error| error.to_string())?;
        Ok(vec![
            assertion(
                "missing_screen_bytes_rejected",
                permission_adapter_result
                    .as_ref()
                    .is_err_and(|error| error.kind == ReasoningErrorKind::InvalidRequest),
                permission_observed,
            ),
            assertion(
                "fabricated_visual_provenance_failed",
                visual_failures.len() == 1,
                format!("failures={}", visual_failures.len()),
            ),
            assertion(
                "fabricated_visual_claim_never_published",
                no_visual_publication,
                "publications=0",
            ),
            assertion(
                "transcript_only_recovery_publishes",
                publications.len() == 1 && transcript_only,
                format!("publications={}", publications.len()),
            ),
        ])
    })
}

fn foreground_preempts_stale_background() -> EvalScenario {
    scenario("foreground_preempts_stale_background", 720, || {
        let mut fixture = fixture(false, 4)?;
        observe(
            &mut fixture.engine,
            "transcript-a",
            "A synthetic material decision is pending.",
            None,
        )?;
        fixture
            .engine
            .evaluate_background()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "background turn did not start".to_string())?;
        fixture
            .engine
            .send_user("Prioritize my question now.")
            .map_err(|error| error.to_string())?;
        fixture.strategist.complete_candidate(
            0,
            candidate(
                "This stale background response must never publish.",
                &["transcript-a"],
                &[],
            ),
        )?;
        fixture.engine.pump();
        let stale_publications = fixture.engine.take_publications();
        fixture.strategist.complete_candidate(
            1,
            candidate(
                "Your typed question takes priority over background analysis.",
                &["transcript-a"],
                &[],
            ),
        )?;
        fixture.engine.pump();
        fixture.verifier.complete_verdict(0, allow_verdict())?;
        let publications = fixture.engine.take_publications();
        let foreground = publications
            .first()
            .is_some_and(|publication| matches!(publication.work, SidekickWork::Foreground { .. }));
        fixture
            .engine
            .stop_capture()
            .map_err(|error| error.to_string())?;
        Ok(vec![
            assertion(
                "background_turn_interrupted",
                fixture.strategist.turns_interrupted() == 1,
                format!("interrupts={}", fixture.strategist.turns_interrupted()),
            ),
            assertion(
                "late_background_completion_ignored",
                stale_publications.is_empty(),
                format!("stale_publications={}", stale_publications.len()),
            ),
            assertion(
                "foreground_published_first",
                publications.len() == 1 && foreground,
                format!("publications={}", publications.len()),
            ),
        ])
    })
}

fn steerable_foreground_reuses_active_turn() -> EvalScenario {
    scenario("steerable_foreground_reuses_active_turn", 720, || {
        let mut fixture = fixture(true, 4)?;
        observe(
            &mut fixture.engine,
            "transcript-a",
            "A synthetic material decision is pending.",
            None,
        )?;
        fixture
            .engine
            .evaluate_background()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "background turn did not start".to_string())?;
        fixture
            .engine
            .send_user("Refocus the active reasoning on my question.")
            .map_err(|error| error.to_string())?;
        fixture.strategist.complete_candidate(
            0,
            candidate(
                "The active turn was steered to the typed foreground question.",
                &["transcript-a"],
                &[],
            ),
        )?;
        fixture.engine.pump();
        fixture.verifier.complete_verdict(0, allow_verdict())?;
        let publications = fixture.engine.take_publications();
        let foreground = publications
            .first()
            .is_some_and(|publication| matches!(publication.work, SidekickWork::Foreground { .. }));
        fixture
            .engine
            .stop_capture()
            .map_err(|error| error.to_string())?;
        Ok(vec![
            assertion(
                "active_provider_turn_steered",
                fixture.strategist.turns_steered() == 1,
                format!("steers={}", fixture.strategist.turns_steered()),
            ),
            assertion(
                "steer_avoids_extra_generation",
                fixture.strategist.requests().len() == 1,
                format!("generation_turns={}", fixture.strategist.requests().len()),
            ),
            assertion(
                "steered_work_published_as_foreground",
                publications.len() == 1 && foreground,
                format!("publications={}", publications.len()),
            ),
        ])
    })
}

fn evidence_bounds_are_enforced() -> EvalScenario {
    scenario("evidence_bounds_are_enforced", 720, || {
        let mut fixture = fixture(true, 3)?;
        for index in 0..6 {
            observe(
                &mut fixture.engine,
                &format!("transcript-{index}"),
                &format!("Synthetic bounded transcript item {index}."),
                None,
            )?;
        }
        let turn = fixture
            .engine
            .send_user("Use only the current bounded window.")
            .map_err(|error| error.to_string())?;
        let receipt = fixture
            .engine
            .foreground_evidence_receipt(&turn)
            .ok_or_else(|| "foreground receipt missing".to_string())?;
        let request = fixture
            .strategist
            .requests()
            .first()
            .cloned()
            .ok_or_else(|| "generation request missing".to_string())?;
        fixture.strategist.complete_candidate(
            0,
            candidate(
                "The newest bounded item is the current synthetic evidence.",
                &["transcript-5"],
                &[],
            ),
        )?;
        fixture.engine.pump();
        fixture.verifier.complete_verdict(0, allow_verdict())?;
        let publications = fixture.engine.take_publications();
        let selected_ids = receipt
            .transcript_evidence_ids
            .iter()
            .map(EvidenceId::as_str)
            .collect::<Vec<_>>();
        let newest_three = selected_ids == vec!["transcript-3", "transcript-4", "transcript-5"];
        let bounded = request.serialized_evidence_chars() <= MAX_WINDOW_CHARS;
        fixture
            .engine
            .stop_capture()
            .map_err(|error| error.to_string())?;
        Ok(vec![
            assertion(
                "transcript_item_bound_keeps_newest",
                newest_three,
                format!("selected_items={}", selected_ids.len()),
            ),
            assertion(
                "serialized_window_within_budget",
                bounded,
                format!(
                    "chars={}/{}",
                    request.serialized_evidence_chars(),
                    MAX_WINDOW_CHARS
                ),
            ),
            assertion(
                "bounded_turn_publishes",
                publications.len() == 1,
                format!("publications={}", publications.len()),
            ),
        ])
    })
}

fn teardown_rejects_late_completion() -> EvalScenario {
    scenario("teardown_rejects_late_completion", 0, || {
        let mut fixture = fixture(true, 4)?;
        observe(
            &mut fixture.engine,
            "transcript-a",
            "The synthetic meeting is ending.",
            None,
        )?;
        fixture
            .engine
            .send_user("One final question.")
            .map_err(|error| error.to_string())?;
        fixture
            .engine
            .stop_capture()
            .map_err(|error| error.to_string())?;
        let inactive = !fixture.engine.has_active_turn();
        fixture.strategist.complete_candidate(
            0,
            candidate(
                "This completion arrived after teardown.",
                &["transcript-a"],
                &[],
            ),
        )?;
        fixture.engine.pump();
        let publications = fixture.engine.take_publications();
        let failures = fixture.engine.take_failures();
        let phase_ended = fixture.engine.session.phase == AssistancePhase::MeetingEnded;
        Ok(vec![
            assertion("active_turn_cleared_on_stop", inactive, "active=false"),
            assertion(
                "provider_sessions_closed",
                fixture.strategist.sessions_closed() == 1
                    && fixture.verifier.sessions_closed() == 1,
                format!(
                    "closed={}+{}",
                    fixture.strategist.sessions_closed(),
                    fixture.verifier.sessions_closed()
                ),
            ),
            assertion(
                "late_completion_has_no_visible_effect",
                publications.is_empty() && failures.is_empty(),
                format!(
                    "publications={} failures={}",
                    publications.len(),
                    failures.len()
                ),
            ),
            assertion("capture_phase_ended", phase_ended, "phase=meeting_ended"),
        ])
    })
}

fn run_once() -> Vec<EvalScenario> {
    vec![
        exact_screen_publication(),
        correction_during_verification(),
        provider_failure_and_recovery(),
        unavailable_screen_fails_closed(),
        foreground_preempts_stale_background(),
        steerable_foreground_reuses_active_turn(),
        evidence_bounds_are_enforced(),
        teardown_rejects_late_completion(),
    ]
}

/// Run every deterministic engine scenario twice and bind the bounded result
/// to a SHA-256 digest. No wall-clock fields or meeting content enter the
/// artifact, so byte-equivalent results are expected on every platform.
pub fn run_live_sidekick_engine_eval() -> LiveSidekickEngineEvalReport {
    let scenarios = run_once();
    let second_run = run_once();
    let reproducible = scenarios == second_run;
    let digest = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&scenarios).expect("eval scenarios are JSON serializable")
        )
    );
    let assertions_total = scenarios
        .iter()
        .map(|scenario| scenario.assertions.len())
        .sum();
    let assertions_passed = scenarios
        .iter()
        .flat_map(|scenario| scenario.assertions.iter())
        .filter(|assertion| assertion.passed)
        .count();
    let scenarios_passed = scenarios.iter().filter(|scenario| scenario.passed).count();
    let summary = EvalSummary {
        scenarios_passed,
        scenarios_total: scenarios.len(),
        assertions_passed,
        assertions_total,
    };
    LiveSidekickEngineEvalReport {
        schema_version: LIVE_SIDEKICK_ENGINE_EVAL_VERSION,
        fixed_seed: LIVE_SIDEKICK_ENGINE_EVAL_SEED,
        passed: reproducible
            && summary.scenarios_passed == summary.scenarios_total
            && summary.assertions_passed == summary.assertions_total,
        reproducible,
        deterministic_digest: digest,
        summary,
        coverage: EvalCoverage {
            production_engine: true,
            production_reducer: true,
            production_evidence_window: true,
            production_publication_gate: true,
            production_verification_gate: true,
            provider_contract: "persistent_steerable_streaming_reasoning_v1",
            deterministic_provider_faults: true,
            native_audio_capture: false,
            native_asr: false,
            native_diarization: false,
            native_screen_permission_adapter: false,
            real_cloud_provider: false,
            release_ready_from_this_report_alone: false,
        },
        scenarios,
    }
}
