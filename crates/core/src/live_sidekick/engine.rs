//! Provider-neutral Sidekick orchestration.
//!
//! This is the intelligence loop owned by Minutes: reducer identity, bounded
//! evidence, correction epochs, intervention policy, and publication. Provider
//! adapters can stream reasoning, but cannot decide what reaches the user.

use super::*;
use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

#[derive(Debug, Clone)]
pub struct LiveSidekickEngineConfig {
    pub base_instructions: String,
    pub developer_instructions: String,
    pub prepared_context: String,
    pub max_window_chars: usize,
    pub max_transcript_items: usize,
}

impl LiveSidekickEngineConfig {
    pub fn validate(&self) -> Result<(), ReasoningError> {
        if self.base_instructions.trim().is_empty()
            || self.developer_instructions.trim().is_empty()
            || self.max_window_chars == 0
            || self.max_transcript_items == 0
        {
            return Err(ReasoningError::invalid_request(
                "Sidekick instructions and positive evidence bounds are required",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidekickWork {
    Background {
        run_id: BackgroundRunId,
        invocation: InvocationIdentity,
    },
    Foreground {
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
    },
}

impl SidekickWork {
    fn invocation(&self) -> InvocationIdentity {
        match self {
            Self::Background { invocation, .. } | Self::Foreground { invocation, .. } => {
                *invocation
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidekickPublication {
    pub work: SidekickWork,
    pub candidate: InterventionCandidate,
    pub first_token_ms: Option<u64>,
    pub total_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidekickFailure {
    pub work: SidekickWork,
    pub error: ReasoningError,
}

struct ActiveReasoning {
    turn_id: ReasoningTurnId,
    work: SidekickWork,
    allowed_evidence_ids: BTreeSet<EvidenceId>,
    allowed_visual_ids: BTreeSet<EvidenceId>,
}

pub struct LiveSidekickEngine {
    pub session: LiveAssistanceSession,
    backend: Arc<dyn PersistentReasoningBackend>,
    backend_session: Option<Box<dyn PersistentReasoningSession>>,
    descriptor: ReasoningBackendDescriptor,
    config: LiveSidekickEngineConfig,
    transcript: VecDeque<ReasoningTranscriptEvidence>,
    authoritative_memory: VecDeque<String>,
    latest_image: Option<ReasoningImageEvidence>,
    event_sender: mpsc::Sender<ReasoningStreamEvent>,
    event_receiver: mpsc::Receiver<ReasoningStreamEvent>,
    active: Option<ActiveReasoning>,
    publications: VecDeque<SidekickPublication>,
    failures: VecDeque<SidekickFailure>,
    evidence_revision: u64,
    last_background_revision: Option<u64>,
    next_run: u64,
    next_turn: u64,
    next_user_event: u64,
}

impl LiveSidekickEngine {
    pub fn new(
        session_id: LiveAssistanceSessionId,
        surface: AssistanceSurface,
        role: UserRole,
        posture: AssistancePosture,
        backend: Arc<dyn PersistentReasoningBackend>,
        config: LiveSidekickEngineConfig,
    ) -> Result<Self, ReasoningError> {
        config.validate()?;
        let descriptor = backend.descriptor();
        if !descriptor.persistent || !descriptor.streaming {
            return Err(ReasoningError::invalid_request(
                "Sidekick requires a persistent streaming reasoning backend",
            ));
        }
        let (event_sender, event_receiver) = mpsc::channel();
        Ok(Self {
            session: LiveAssistanceSession::new(session_id, surface, role, posture),
            backend,
            backend_session: None,
            descriptor,
            config,
            transcript: VecDeque::new(),
            authoritative_memory: VecDeque::new(),
            latest_image: None,
            event_sender,
            event_receiver,
            active: None,
            publications: VecDeque::new(),
            failures: VecDeque::new(),
            evidence_revision: 0,
            last_background_revision: None,
            next_run: 1,
            next_turn: 1,
            next_user_event: 1,
        })
    }

    pub fn descriptor(&self) -> &ReasoningBackendDescriptor {
        &self.descriptor
    }

    pub fn start_capture(
        &mut self,
        capture_session_id: CaptureSessionId,
        mode: CaptureMode,
    ) -> Result<Reduction, ReasoningError> {
        let reduction = self.session.reduce(AssistanceEvent::CaptureStarted {
            session_id: self.session.id.clone(),
            capture_session_id,
            mode,
        });
        if !reduction.accepted {
            return Err(ReasoningError::invalid_request(format!(
                "capture start rejected: {:?}",
                reduction.rejection
            )));
        }
        self.restart_backend()?;
        Ok(reduction)
    }

    pub fn observe_transcript(
        &mut self,
        evidence: ReasoningTranscriptEvidence,
    ) -> Result<Reduction, ReasoningError> {
        let capture_session_id = self.capture_id()?;
        let reduction = self.session.reduce(AssistanceEvent::EvidenceObserved {
            session_id: self.session.id.clone(),
            evidence: UntrustedEvidence {
                id: evidence.evidence_id.clone(),
                source_kind: EvidenceSourceKind::TranscriptFinal,
                capture_session_id: Some(capture_session_id),
                finalized_meeting_ref: None,
            },
        });
        if reduction.accepted {
            self.transcript.push_back(evidence);
            self.trim_transcript();
            self.evidence_revision = self.evidence_revision.saturating_add(1);
        }
        Ok(reduction)
    }

    pub fn observe_screen(
        &mut self,
        evidence_id: EvidenceId,
        path: PathBuf,
    ) -> Result<Reduction, ReasoningError> {
        let capture_session_id = self.capture_id()?;
        let canonical = path.canonicalize().map_err(|error| {
            ReasoningError::invalid_request(format!("screen image is unreadable: {error}"))
        })?;
        let bytes = std::fs::read(&canonical).map_err(|error| {
            ReasoningError::invalid_request(format!("screen image is unreadable: {error}"))
        })?;
        if canonical.extension().and_then(|value| value.to_str()) != Some("png")
            || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        {
            return Err(ReasoningError::invalid_request(
                "screen evidence must be a readable PNG",
            ));
        }
        let reduction = self.session.reduce(AssistanceEvent::EvidenceObserved {
            session_id: self.session.id.clone(),
            evidence: UntrustedEvidence {
                id: evidence_id.clone(),
                source_kind: EvidenceSourceKind::ScreenImage,
                capture_session_id: Some(capture_session_id.clone()),
                finalized_meeting_ref: None,
            },
        });
        if reduction.accepted {
            self.latest_image = Some(ReasoningImageEvidence {
                evidence_id,
                capture_session_id,
                path: canonical,
            });
            self.evidence_revision = self.evidence_revision.saturating_add(1);
        }
        Ok(reduction)
    }

    pub fn evaluate_background(&mut self) -> Result<Option<BackgroundRunId>, ReasoningError> {
        self.pump();
        if self.active.is_some()
            || self.last_background_revision == Some(self.evidence_revision)
            || self.evidence_revision == 0
        {
            return Ok(None);
        }
        let run_id = BackgroundRunId::new(format!("background-{}", self.next_run));
        self.next_run = self.next_run.saturating_add(1);
        let reduction = self.session.reduce(AssistanceEvent::BackgroundStarted {
            session_id: self.session.id.clone(),
            run_id: run_id.clone(),
        });
        if !reduction.accepted {
            return Ok(None);
        }
        let invocation = self
            .session
            .background_run
            .as_ref()
            .expect("accepted background has identity")
            .invocation;
        let work = SidekickWork::Background {
            run_id: run_id.clone(),
            invocation,
        };
        if let Err(error) = self.start_turn(work.clone(), None) {
            self.reduce_failure(&work);
            self.failures.push_back(SidekickFailure {
                work,
                error: error.clone(),
            });
            return Err(error);
        }
        Ok(Some(run_id))
    }

    pub fn send_user(
        &mut self,
        text: impl Into<String>,
    ) -> Result<ForegroundTurnId, ReasoningError> {
        self.pump();
        let text = text.into();
        if text.trim().is_empty() {
            return Err(ReasoningError::invalid_request("typed message is empty"));
        }
        let turn_id = ForegroundTurnId::new(format!("foreground-{}", self.next_turn));
        self.next_turn = self.next_turn.saturating_add(1);
        let source_event_id = EvidenceId::new(format!("typed-user-{}", self.next_user_event));
        self.next_user_event = self.next_user_event.saturating_add(1);
        let reduction = self.session.reduce(AssistanceEvent::UserMessage {
            session_id: self.session.id.clone(),
            turn_id: turn_id.clone(),
            source_event_id,
            text: text.clone(),
        });
        if !reduction.accepted {
            return Err(ReasoningError::invalid_request(format!(
                "typed message rejected: {:?}",
                reduction.rejection
            )));
        }
        let invocation = self
            .session
            .foreground_turn
            .as_ref()
            .expect("accepted foreground has identity")
            .invocation;
        let work = SidekickWork::Foreground {
            turn_id: turn_id.clone(),
            invocation,
        };
        self.authoritative_memory.push_back(text.clone());
        while self.authoritative_memory.len() > 6
            || self
                .authoritative_memory
                .iter()
                .map(String::len)
                .sum::<usize>()
                > 2_000
        {
            self.authoritative_memory.pop_front();
        }
        let request = match self.request_for(invocation, ReasoningTurnKind::Foreground, Some(text))
        {
            Ok(request) => request,
            Err(error) => {
                self.reduce_failure(&work);
                return Err(error);
            }
        };

        if self.descriptor.steerable {
            if let (Some(active), Some(provider)) = (&self.active, self.backend_session.as_mut()) {
                if provider
                    .steer_turn(&active.turn_id, request.clone())
                    .is_ok()
                {
                    let (allowed_evidence_ids, allowed_visual_ids) =
                        Self::allowed_provenance(&request);
                    self.active = Some(ActiveReasoning {
                        turn_id: active.turn_id.clone(),
                        work,
                        allowed_evidence_ids,
                        allowed_visual_ids,
                    });
                    return Ok(turn_id);
                }
            }
        }
        if let (Some(active), Some(provider)) = (self.active.take(), self.backend_session.as_mut())
        {
            let _ = provider.interrupt_turn(&active.turn_id);
        }
        if let Err(error) = self.start_turn(work.clone(), Some(request)) {
            self.reduce_failure(&work);
            self.failures.push_back(SidekickFailure {
                work,
                error: error.clone(),
            });
            return Err(error);
        }
        Ok(turn_id)
    }

    pub fn pump(&mut self) {
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                ReasoningStreamEvent::TextDelta { .. } => {}
                ReasoningStreamEvent::Completed {
                    turn_id,
                    invocation,
                    result,
                } => self.complete(turn_id, invocation, result),
                ReasoningStreamEvent::Failed {
                    turn_id,
                    invocation,
                    error,
                } => self.failed(turn_id, invocation, error),
            }
        }
    }

    pub fn take_publications(&mut self) -> Vec<SidekickPublication> {
        self.pump();
        self.publications.drain(..).collect()
    }

    pub fn take_failures(&mut self) -> Vec<SidekickFailure> {
        self.pump();
        self.failures.drain(..).collect()
    }

    pub fn stop_capture(&mut self) -> Result<Reduction, ReasoningError> {
        let capture_session_id = self.capture_id()?;
        let reduction = self.session.reduce(AssistanceEvent::CaptureStopped {
            session_id: self.session.id.clone(),
            capture_session_id,
        });
        self.active = None;
        if let Some(mut provider) = self.backend_session.take() {
            provider.close();
        }
        Ok(reduction)
    }

    pub fn invalidate_source_policy(&mut self, new_generation: u64) -> Result<(), ReasoningError> {
        let reduction = self
            .session
            .reduce(AssistanceEvent::SourcePolicyInvalidated {
                session_id: self.session.id.clone(),
                new_generation,
            });
        if !reduction.accepted {
            return Err(ReasoningError::invalid_request(format!(
                "policy invalidation rejected: {:?}",
                reduction.rejection
            )));
        }
        self.transcript.clear();
        self.authoritative_memory.clear();
        self.latest_image = None;
        self.active = None;
        self.evidence_revision = 0;
        self.last_background_revision = None;
        self.restart_backend()
    }

    fn start_turn(
        &mut self,
        work: SidekickWork,
        prepared_request: Option<ReasoningTurnRequest>,
    ) -> Result<(), ReasoningError> {
        let request = match prepared_request {
            Some(request) => request,
            None => self.request_for(work.invocation(), ReasoningTurnKind::Background, None)?,
        };
        let sender = self.event_sender.clone();
        let (allowed_evidence_ids, allowed_visual_ids) = Self::allowed_provenance(&request);
        let turn_id = self
            .backend_session
            .as_mut()
            .ok_or_else(|| {
                ReasoningError::new(
                    ReasoningErrorKind::Unavailable,
                    "reasoning backend is not attached",
                    true,
                )
            })?
            .start_turn(
                request,
                Arc::new(move |event| {
                    let _ = sender.send(event);
                }),
            )?;
        self.active = Some(ActiveReasoning {
            turn_id,
            work,
            allowed_evidence_ids,
            allowed_visual_ids,
        });
        Ok(())
    }

    fn request_for(
        &self,
        invocation: InvocationIdentity,
        kind: ReasoningTurnKind,
        typed_user_message: Option<String>,
    ) -> Result<ReasoningTurnRequest, ReasoningError> {
        let request = ReasoningTurnRequest {
            kind,
            invocation,
            window: BoundedReasoningWindow {
                capture_session_id: self.capture_id()?,
                transcript: self.transcript.iter().cloned().collect(),
                latest_image: self
                    .descriptor
                    .image_input
                    .then(|| self.latest_image.clone())
                    .flatten(),
                prepared_context: self.prepared_context_snapshot(),
            },
            authoritative_memory: self.authoritative_memory.iter().cloned().collect(),
            typed_user_message,
            output_contract: ReasoningOutputContract::InterventionCandidateV1,
        };
        request.validate(self.config.max_window_chars)?;
        Ok(request)
    }

    fn complete(
        &mut self,
        turn_id: ReasoningTurnId,
        invocation: InvocationIdentity,
        result: ReasoningTurnResult,
    ) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if active.turn_id != turn_id || active.work.invocation() != invocation {
            return;
        }
        let allowed_evidence_ids = active.allowed_evidence_ids.clone();
        let allowed_visual_ids = active.allowed_visual_ids.clone();
        let work = self.active.take().expect("active checked").work;
        let Ok(candidate) = InterventionCandidate::from_backend_json(&result.text) else {
            self.reduce_failure(&work);
            self.failures.push_back(SidekickFailure {
                work,
                error: ReasoningError::new(
                    ReasoningErrorKind::Protocol,
                    "reasoning backend returned an invalid intervention candidate",
                    true,
                ),
            });
            return;
        };
        let provenance_supported = candidate
            .evidence_ids
            .iter()
            .all(|id| allowed_evidence_ids.contains(id))
            && candidate
                .visual_evidence_ids
                .iter()
                .all(|id| allowed_visual_ids.contains(id));
        if !provenance_supported {
            self.reduce_failure(&work);
            self.failures.push_back(SidekickFailure {
                work,
                error: ReasoningError::new(
                    ReasoningErrorKind::Protocol,
                    "reasoning candidate cited evidence outside its bounded turn window",
                    false,
                ),
            });
            return;
        }
        let reduction = match &work {
            SidekickWork::Background { run_id, invocation } => {
                self.last_background_revision = Some(self.evidence_revision);
                self.session.reduce(AssistanceEvent::BackgroundCompleted {
                    session_id: self.session.id.clone(),
                    run_id: run_id.clone(),
                    invocation: *invocation,
                    candidate: candidate.clone(),
                })
            }
            SidekickWork::Foreground {
                turn_id,
                invocation,
            } => self.session.reduce(AssistanceEvent::ForegroundCompleted {
                session_id: self.session.id.clone(),
                turn_id: turn_id.clone(),
                invocation: *invocation,
                candidate: candidate.clone(),
            }),
        };
        if reduction.actions.iter().any(|action| {
            matches!(
                action,
                AssistanceAction::PublishForegroundResponse { .. }
                    | AssistanceAction::PublishBackgroundInsight { .. }
            )
        }) {
            self.publications.push_back(SidekickPublication {
                work,
                candidate,
                first_token_ms: result.first_token_ms,
                total_ms: result.total_ms,
            });
        }
    }

    fn failed(
        &mut self,
        turn_id: ReasoningTurnId,
        invocation: InvocationIdentity,
        error: ReasoningError,
    ) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if active.turn_id != turn_id || active.work.invocation() != invocation {
            return;
        }
        let work = self.active.take().expect("active checked").work;
        self.reduce_failure(&work);
        self.failures.push_back(SidekickFailure { work, error });
    }

    fn reduce_failure(&mut self, work: &SidekickWork) {
        let event = match work {
            SidekickWork::Background { run_id, invocation } => AssistanceEvent::BackgroundFailed {
                session_id: self.session.id.clone(),
                run_id: run_id.clone(),
                invocation: *invocation,
            },
            SidekickWork::Foreground {
                turn_id,
                invocation,
            } => AssistanceEvent::ForegroundFailed {
                session_id: self.session.id.clone(),
                turn_id: turn_id.clone(),
                invocation: *invocation,
            },
        };
        let _ = self.session.reduce(event);
    }

    fn restart_backend(&mut self) -> Result<(), ReasoningError> {
        if let Some(mut provider) = self.backend_session.take() {
            provider.close();
        }
        let capture_session_id = self.capture_id()?;
        self.backend_session = Some(self.backend.start_session(ReasoningSessionConfig {
            base_instructions: self.config.base_instructions.clone(),
            developer_instructions: self.config.developer_instructions.clone(),
            latency_class: ReasoningLatencyClass::Realtime,
            max_window_chars: self.config.max_window_chars,
            ephemeral: true,
            evidence_scope: ReasoningEvidenceScope {
                capture_session_id,
                source_policy_generation: self.session.source_policy_generation,
            },
        })?);
        Ok(())
    }

    pub fn recover_backend(&mut self) -> Result<(), ReasoningError> {
        self.active = None;
        self.restart_backend()
    }

    fn capture_id(&self) -> Result<CaptureSessionId, ReasoningError> {
        self.session.capture_session_id.clone().ok_or_else(|| {
            ReasoningError::invalid_request("Sidekick is not attached to a capture session")
        })
    }

    fn prepared_context_snapshot(&self) -> String {
        let mut context = format!(
            "{}\nuser_role={:?}\nposture={:?}\nrole_revision={}",
            self.config.prepared_context,
            self.session.user_role.value,
            self.session.posture,
            self.session.user_role.revision,
        );
        for correction in self.session.speaker_corrections.values() {
            context.push_str(&format!(
                "\nspeaker_correction[{}]={} (revision {})",
                correction.source_label, correction.corrected_label, correction.revision
            ));
        }
        context
    }

    fn trim_transcript(&mut self) {
        while self.transcript.len() > self.config.max_transcript_items
            || self
                .transcript
                .iter()
                .map(|item| item.text.len() + item.speaker_label.as_ref().map_or(0, String::len))
                .sum::<usize>()
                > self
                    .config
                    .max_window_chars
                    .saturating_sub(self.config.prepared_context.len())
        {
            self.transcript.pop_front();
        }
    }

    fn allowed_provenance(
        request: &ReasoningTurnRequest,
    ) -> (BTreeSet<EvidenceId>, BTreeSet<EvidenceId>) {
        let transcript = request
            .window
            .transcript
            .iter()
            .map(|item| item.evidence_id.clone())
            .collect();
        let visual = request
            .window
            .latest_image
            .iter()
            .map(|item| item.evidence_id.clone())
            .collect();
        (transcript, visual)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(|error| error.into_inner())
    }

    struct FakeTurn {
        id: ReasoningTurnId,
        request: ReasoningTurnRequest,
        sink: Arc<dyn ReasoningEventSink>,
    }

    #[derive(Default)]
    struct FakeState {
        sessions_started: usize,
        turns: Vec<FakeTurn>,
        steer_fails: bool,
    }

    #[derive(Clone, Default)]
    struct FakeBackend {
        state: Arc<Mutex<FakeState>>,
        steerable: bool,
    }

    impl FakeBackend {
        fn complete(&self, index: usize, candidate: InterventionCandidate) {
            let state = lock(&self.state);
            let turn = &state.turns[index];
            turn.sink.on_event(ReasoningStreamEvent::Completed {
                turn_id: turn.id.clone(),
                invocation: turn.request.invocation,
                result: ReasoningTurnResult {
                    text: serde_json::to_string(&candidate).unwrap(),
                    first_token_ms: Some(250),
                    total_ms: 500,
                },
            });
        }
    }

    impl PersistentReasoningBackend for FakeBackend {
        fn descriptor(&self) -> ReasoningBackendDescriptor {
            ReasoningBackendDescriptor {
                provider: "fake".into(),
                model: "deterministic".into(),
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
            lock(&self.state).sessions_started += 1;
            Ok(Box::new(FakeSession {
                id: ReasoningSessionId::new("fake-session"),
                state: Arc::clone(&self.state),
            }))
        }
    }

    struct FakeSession {
        id: ReasoningSessionId,
        state: Arc<Mutex<FakeState>>,
    }

    impl PersistentReasoningSession for FakeSession {
        fn id(&self) -> &ReasoningSessionId {
            &self.id
        }

        fn start_turn(
            &mut self,
            request: ReasoningTurnRequest,
            sink: Arc<dyn ReasoningEventSink>,
        ) -> Result<ReasoningTurnId, ReasoningError> {
            let mut state = lock(&self.state);
            let id = ReasoningTurnId::new(format!("fake-turn-{}", state.turns.len() + 1));
            state.turns.push(FakeTurn {
                id: id.clone(),
                request,
                sink,
            });
            Ok(id)
        }

        fn steer_turn(
            &mut self,
            turn_id: &ReasoningTurnId,
            request: ReasoningTurnRequest,
        ) -> Result<(), ReasoningError> {
            let mut state = lock(&self.state);
            if state.steer_fails {
                return Err(ReasoningError::new(
                    ReasoningErrorKind::Unavailable,
                    "synthetic steer miss",
                    true,
                ));
            }
            let turn = state
                .turns
                .iter_mut()
                .find(|turn| &turn.id == turn_id)
                .ok_or_else(|| ReasoningError::invalid_request("unknown fake turn"))?;
            turn.request = request;
            Ok(())
        }

        fn interrupt_turn(&mut self, _turn_id: &ReasoningTurnId) -> Result<(), ReasoningError> {
            Ok(())
        }

        fn close(&mut self) {}
    }

    fn engine(backend: FakeBackend) -> LiveSidekickEngine {
        let mut engine = LiveSidekickEngine::new(
            "assist".into(),
            AssistanceSurface::NativeRecall,
            UserRole::DecisionMaker,
            AssistancePosture::Strategist,
            Arc::new(backend),
            LiveSidekickEngineConfig {
                base_instructions: "You are a meeting strategist.".into(),
                developer_instructions: "Return the intervention contract.".into(),
                prepared_context: "Protect decision quality.".into(),
                max_window_chars: 1_000,
                max_transcript_items: 4,
            },
        )
        .unwrap();
        engine
            .start_capture("capture".into(), CaptureMode::Recording)
            .unwrap();
        engine
    }

    fn observe(engine: &mut LiveSidekickEngine, id: &str, text: &str) {
        assert!(
            engine
                .observe_transcript(ReasoningTranscriptEvidence {
                    evidence_id: id.into(),
                    text: text.into(),
                    speaker_label: None,
                    speaker_verified: false,
                    offset_ms: 0,
                    duration_ms: 100,
                })
                .unwrap()
                .accepted
        );
    }

    fn speak(evidence_ids: &[&str], text: &str) -> InterventionCandidate {
        InterventionCandidate {
            decision: InterventionDecision::Speak,
            kind: Some("insight".into()),
            text: Some(text.into()),
            evidence_ids: evidence_ids.iter().map(|id| EvidenceId::new(*id)).collect(),
            visual_evidence_ids: Vec::new(),
            confidence: 90,
        }
    }

    #[test]
    fn engine_owns_bounded_evidence_quiet_policy_and_publication() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        for index in 1..=6 {
            observe(&mut engine, &format!("e-{index}"), &format!("fact {index}"));
        }
        engine.evaluate_background().unwrap().unwrap();
        {
            let state = lock(&backend.state);
            assert_eq!(state.turns[0].request.window.transcript.len(), 4);
            assert_eq!(
                state.turns[0].request.window.transcript[0]
                    .evidence_id
                    .as_str(),
                "e-3"
            );
        }
        backend.complete(0, speak(&["e-3", "e-6"], "A material synthesis."));
        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        assert_eq!(
            publications[0].candidate.text.as_deref(),
            Some("A material synthesis.")
        );
        assert!(engine.evaluate_background().unwrap().is_none());
    }

    #[test]
    fn foreground_steer_changes_invocation_authority_without_a_second_turn() {
        let backend = FakeBackend {
            steerable: true,
            ..FakeBackend::default()
        };
        let mut engine = engine(backend.clone());
        observe(&mut engine, "fact", "A material risk exists.");
        engine.evaluate_background().unwrap();
        engine.send_user("What should I do?").unwrap();
        assert_eq!(lock(&backend.state).turns.len(), 1);
        backend.complete(0, speak(&["fact"], "Take the reversible path."));
        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        assert!(matches!(
            publications[0].work,
            SidekickWork::Foreground { .. }
        ));
    }

    #[test]
    fn stop_and_policy_epoch_reject_late_provider_history() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        observe(&mut engine, "fact", "Old-policy fact.");
        engine.evaluate_background().unwrap();
        engine.invalidate_source_policy(1).unwrap();
        assert_eq!(lock(&backend.state).sessions_started, 2);
        backend.complete(0, speak(&["fact"], "Stale old-policy answer."));
        assert!(engine.take_publications().is_empty());

        observe(&mut engine, "new-fact", "New-policy fact.");
        engine.evaluate_background().unwrap();
        engine.stop_capture().unwrap();
        backend.complete(1, speak(&["new-fact"], "Late after stop."));
        assert!(engine.take_publications().is_empty());
    }
}
