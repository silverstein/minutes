//! Provider-neutral Sidekick orchestration.
//!
//! This is the intelligence loop owned by Minutes: reducer identity, bounded
//! evidence, correction epochs, intervention policy, and publication. Provider
//! adapters can stream reasoning, but cannot decide what reaches the user.

use super::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

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

/// Immutable receipt for the exact Minutes-owned evidence window handed to a
/// foreground provider turn. This is suitable for audit and acceptance gates;
/// it contains opaque provenance identifiers, never transcript or image bytes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForegroundEvidenceReceipt {
    pub turn_id: ForegroundTurnId,
    pub capture_session_id: CaptureSessionId,
    pub transcript_evidence_ids: Vec<EvidenceId>,
    pub visual_evidence_ids: Vec<EvidenceId>,
}

/// Terminal lifecycle signal for every provider turn that matched the active
/// invocation. UI runtimes can settle loading state even when Minutes elects
/// not to publish model output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidekickLifecycleEvent {
    pub work: SidekickWork,
    pub outcome: SidekickLifecycleOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidekickLifecycleOutcome {
    Published,
    Suppressed(CandidateSuppressionReason),
    Failed(ReasoningError),
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
    backend_sessions_started: u64,
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
    lifecycle_events: VecDeque<SidekickLifecycleEvent>,
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
            backend_sessions_started: 0,
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
            lifecycle_events: VecDeque::new(),
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

    /// Opaque identity of the currently attached provider-neutral reasoning
    /// session. Diagnostics use this to prove sequential turns stayed on one
    /// persistent session without depending on a vendor-specific thread API.
    pub fn reasoning_session_id(&self) -> Option<&ReasoningSessionId> {
        self.backend_session.as_ref().map(|session| session.id())
    }

    /// Number of backend sessions successfully started during this engine's
    /// lifetime. A value above one means recovery or a policy epoch change
    /// replaced the persistent session.
    pub fn reasoning_sessions_started(&self) -> u64 {
        self.backend_sessions_started
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
        let canonical = path.canonicalize().map_err(|error| {
            ReasoningError::invalid_request(format!("screen image is unreadable: {error}"))
        })?;
        let bytes = std::fs::read(&canonical).map_err(|error| {
            ReasoningError::invalid_request(format!("screen image is unreadable: {error}"))
        })?;
        self.observe_screen_bytes(evidence_id, canonical, bytes)
    }

    /// Observe exact image bytes already selected by an evidence adapter.
    /// The provider-neutral window owns these bytes, so a later pathname
    /// replacement cannot change what a backend receives.
    pub fn observe_screen_bytes(
        &mut self,
        evidence_id: EvidenceId,
        provenance_path: PathBuf,
        png_bytes: Vec<u8>,
    ) -> Result<Reduction, ReasoningError> {
        let capture_session_id = self.capture_id()?;
        if !provenance_path.is_absolute()
            || provenance_path.extension().and_then(|value| value.to_str()) != Some("png")
            || !png_bytes.starts_with(b"\x89PNG\r\n\x1a\n")
            || png_bytes.len() > 8 * 1024 * 1024
        {
            return Err(ReasoningError::invalid_request(
                "screen evidence must contain a bounded PNG and absolute provenance path",
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
                path: provenance_path,
                sha256: sha256_hex(&png_bytes),
                png_bytes,
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
            self.record_failure(work, error.clone());
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
        // The current message has its own authority lane. It becomes memory
        // only after this request is handed to a provider, so it is never
        // duplicated inside the same inference payload.
        let request = match self.request_for(
            invocation,
            ReasoningTurnKind::Foreground,
            Some(text.clone()),
        ) {
            Ok(request) => request,
            Err(error) => {
                self.record_failure(work, error.clone());
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
                    self.remember_user_message(text);
                    return Ok(turn_id);
                }
            }
        }
        if let (Some(active), Some(provider)) = (self.active.take(), self.backend_session.as_mut())
        {
            let _ = provider.interrupt_turn(&active.turn_id);
        }
        if let Err(error) = self.start_turn(work.clone(), Some(request)) {
            self.record_failure(work, error.clone());
            return Err(error);
        }
        self.remember_user_message(text);
        Ok(turn_id)
    }

    /// Return the exact evidence provenance currently authorized for a
    /// foreground turn, if that turn still owns the provider invocation.
    pub fn foreground_evidence_receipt(
        &self,
        turn_id: &ForegroundTurnId,
    ) -> Option<ForegroundEvidenceReceipt> {
        let active = self.active.as_ref()?;
        let SidekickWork::Foreground {
            turn_id: active_turn_id,
            ..
        } = &active.work
        else {
            return None;
        };
        if active_turn_id != turn_id {
            return None;
        }
        Some(ForegroundEvidenceReceipt {
            turn_id: turn_id.clone(),
            capture_session_id: self.capture_id().ok()?,
            transcript_evidence_ids: active.allowed_evidence_ids.iter().cloned().collect(),
            visual_evidence_ids: active.allowed_visual_ids.iter().cloned().collect(),
        })
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

    /// Pump provider events and report whether an inference remains active.
    pub fn has_active_turn(&mut self) -> bool {
        self.pump();
        self.active.is_some()
    }

    /// Drain terminal turn outcomes, including intentional suppression.
    pub fn take_lifecycle_events(&mut self) -> Vec<SidekickLifecycleEvent> {
        self.pump();
        self.lifecycle_events.drain(..).collect()
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
        let mut transcript: Vec<ReasoningTranscriptEvidence> =
            self.transcript.iter().cloned().collect();
        let mut authoritative_memory: Vec<String> =
            self.authoritative_memory.iter().cloned().collect();
        let capture_session_id = self.capture_id()?;
        let latest_image = self
            .descriptor
            .image_input
            .then(|| self.latest_image.clone())
            .flatten();
        let prepared_context = self.prepared_context_snapshot();
        let build_request = |transcript: Vec<ReasoningTranscriptEvidence>,
                             authoritative_memory: Vec<String>| {
            ReasoningTurnRequest {
                kind,
                invocation,
                window: BoundedReasoningWindow {
                    capture_session_id: capture_session_id.clone(),
                    transcript,
                    latest_image: latest_image.clone(),
                    prepared_context: prepared_context.clone(),
                },
                authoritative_memory,
                typed_user_message: typed_user_message.clone(),
                output_contract: ReasoningOutputContract::InterventionCandidateV1,
            }
        };
        loop {
            let request = build_request(transcript.clone(), authoritative_memory.clone());
            if request.serialized_evidence_chars() <= self.config.max_window_chars {
                request.validate(self.config.max_window_chars)?;
                return Ok(request);
            }
            // Keep the freshest item in each lane as long as possible, then
            // fail closed if fixed/current context alone exceeds the budget.
            if transcript.len() > 1 {
                transcript.remove(0);
            } else if !authoritative_memory.is_empty() {
                authoritative_memory.remove(0);
            } else if !transcript.is_empty() {
                transcript.remove(0);
            } else {
                request.validate(self.config.max_window_chars)?;
                unreachable!("over-budget request validation must fail")
            }
        }
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
            self.record_failure(
                work,
                ReasoningError::new(
                    ReasoningErrorKind::Protocol,
                    "reasoning backend returned an invalid intervention candidate",
                    true,
                ),
            );
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
            self.record_failure(
                work,
                ReasoningError::new(
                    ReasoningErrorKind::Protocol,
                    "reasoning candidate cited evidence outside its bounded turn window",
                    false,
                ),
            );
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
        let published = reduction.actions.iter().any(|action| {
            matches!(
                action,
                AssistanceAction::PublishForegroundResponse { .. }
                    | AssistanceAction::PublishBackgroundInsight { .. }
            )
        });
        if published {
            self.publications.push_back(SidekickPublication {
                work: work.clone(),
                candidate,
                first_token_ms: result.first_token_ms,
                total_ms: result.total_ms,
            });
            self.lifecycle_events.push_back(SidekickLifecycleEvent {
                work,
                outcome: SidekickLifecycleOutcome::Published,
            });
        } else if let Some(reason) = reduction.actions.iter().find_map(|action| match action {
            AssistanceAction::SuppressCandidate { reason, .. } => Some(*reason),
            _ => None,
        }) {
            self.lifecycle_events.push_back(SidekickLifecycleEvent {
                work,
                outcome: SidekickLifecycleOutcome::Suppressed(reason),
            });
        } else {
            self.record_failure(
                work,
                ReasoningError::new(
                    ReasoningErrorKind::Protocol,
                    "active reasoning completion was rejected by session state",
                    false,
                ),
            );
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
        self.record_failure(work, error);
    }

    fn record_failure(&mut self, work: SidekickWork, error: ReasoningError) {
        self.reduce_failure(&work);
        self.failures.push_back(SidekickFailure {
            work: work.clone(),
            error: error.clone(),
        });
        self.lifecycle_events.push_back(SidekickLifecycleEvent {
            work,
            outcome: SidekickLifecycleOutcome::Failed(error),
        });
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
        let session = self.backend.start_session(ReasoningSessionConfig {
            base_instructions: self.config.base_instructions.clone(),
            developer_instructions: self.config.developer_instructions.clone(),
            latency_class: ReasoningLatencyClass::Realtime,
            max_window_chars: self.config.max_window_chars,
            ephemeral: true,
            evidence_scope: ReasoningEvidenceScope {
                capture_session_id,
                source_policy_generation: self.session.source_policy_generation,
            },
        })?;
        self.backend_sessions_started = self.backend_sessions_started.saturating_add(1);
        self.backend_session = Some(session);
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

    fn remember_user_message(&mut self, text: String) {
        self.authoritative_memory.push_back(text);
        while self.authoritative_memory.len() > 6 {
            self.authoritative_memory.pop_front();
        }
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
            claims_visual_observation: false,
            confidence: 90,
        }
    }

    fn silent() -> InterventionCandidate {
        InterventionCandidate {
            decision: InterventionDecision::Silent,
            kind: None,
            text: None,
            evidence_ids: Vec::new(),
            visual_evidence_ids: Vec::new(),
            claims_visual_observation: false,
            confidence: 95,
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
    fn engine_reports_provider_neutral_persistent_session_identity() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());

        assert_eq!(engine.reasoning_sessions_started(), 1);
        assert_eq!(
            engine
                .reasoning_session_id()
                .map(ReasoningSessionId::as_str),
            Some("fake-session")
        );

        engine.invalidate_source_policy(1).unwrap();
        assert_eq!(engine.reasoning_sessions_started(), 2);
        assert_eq!(lock(&backend.state).sessions_started, 2);
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
    fn foreground_receipt_attests_the_exact_provider_window_and_turn() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend);
        observe(&mut engine, "fixture-1", "First approved fact.");
        observe(&mut engine, "fixture-2", "Second approved fact.");

        let first = engine.send_user("First question").unwrap();
        let first_receipt = engine.foreground_evidence_receipt(&first).unwrap();
        assert_eq!(first_receipt.turn_id, first);
        assert_eq!(first_receipt.capture_session_id.as_str(), "capture");
        assert_eq!(
            first_receipt
                .transcript_evidence_ids
                .iter()
                .map(EvidenceId::as_str)
                .collect::<Vec<_>>(),
            vec!["fixture-1", "fixture-2"]
        );
        assert!(first_receipt.visual_evidence_ids.is_empty());

        let second = engine.send_user("Second question").unwrap();
        assert!(engine.foreground_evidence_receipt(&first).is_none());
        assert_eq!(
            engine.foreground_evidence_receipt(&second).unwrap().turn_id,
            second
        );
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

    #[test]
    fn current_typed_message_is_not_duplicated_in_authoritative_memory() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        observe(&mut engine, "fact", "A material risk exists.");

        engine.send_user("First question").unwrap();
        {
            let state = lock(&backend.state);
            assert_eq!(
                state.turns[0].request.typed_user_message.as_deref(),
                Some("First question")
            );
            assert!(state.turns[0].request.authoritative_memory.is_empty());
        }
        backend.complete(0, speak(&["fact"], "First answer."));

        engine.send_user("Second question").unwrap();
        let state = lock(&backend.state);
        assert_eq!(
            state.turns[1].request.authoritative_memory,
            vec!["First question"]
        );
        assert_eq!(
            state.turns[1].request.typed_user_message.as_deref(),
            Some("Second question")
        );
        assert!(!state.turns[1]
            .request
            .authoritative_memory
            .iter()
            .any(|message| message == "Second question"));
    }

    #[test]
    fn engine_trims_old_lanes_to_the_combined_serialized_budget() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        engine.config.max_window_chars = 700;
        engine.authoritative_memory.push_back("m".repeat(300));
        observe(&mut engine, "large-fact", &"t".repeat(300));

        engine.send_user("What changed?").unwrap();
        let state = lock(&backend.state);
        let request = &state.turns[0].request;
        assert!(request.serialized_evidence_chars() <= 700);
        assert_eq!(request.typed_user_message.as_deref(), Some("What changed?"));
        assert!(request.authoritative_memory.is_empty());
        assert_eq!(request.window.transcript.len(), 1);
    }

    #[test]
    fn suppressed_completion_emits_terminal_lifecycle_event() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        observe(&mut engine, "fact", "Routine transcript movement.");
        engine.evaluate_background().unwrap().unwrap();
        assert!(engine.has_active_turn());

        backend.complete(0, silent());
        assert!(!engine.has_active_turn());
        let events = engine.take_lifecycle_events();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].outcome,
            SidekickLifecycleOutcome::Suppressed(CandidateSuppressionReason::ModelChoseSilence)
        ));
        assert!(engine.take_publications().is_empty());
    }

    #[test]
    fn visual_receipt_must_be_the_exact_image_selected_for_the_turn() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        let directory = std::env::temp_dir();
        let first = directory.join(format!("minutes-sidekick-{}-first.png", std::process::id()));
        let second = directory.join(format!(
            "minutes-sidekick-{}-second.png",
            std::process::id()
        ));
        std::fs::write(&first, b"\x89PNG\r\n\x1a\nfirst").unwrap();
        std::fs::write(&second, b"\x89PNG\r\n\x1a\nsecond").unwrap();

        engine
            .observe_screen("screen-first".into(), first.clone())
            .unwrap();
        engine.evaluate_background().unwrap().unwrap();
        engine
            .observe_screen("screen-second".into(), second.clone())
            .unwrap();
        backend.complete(
            0,
            InterventionCandidate {
                decision: InterventionDecision::Speak,
                kind: Some("insight".into()),
                text: Some("The later screen says something.".into()),
                evidence_ids: Vec::new(),
                visual_evidence_ids: vec!["screen-second".into()],
                claims_visual_observation: true,
                confidence: 90,
            },
        );
        assert!(engine.take_publications().is_empty());
        assert!(matches!(
            engine.take_lifecycle_events().as_slice(),
            [SidekickLifecycleEvent {
                outcome: SidekickLifecycleOutcome::Failed(ReasoningError {
                    kind: ReasoningErrorKind::Protocol,
                    ..
                }),
                ..
            }]
        ));

        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }
}
