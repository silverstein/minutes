//! Provider-neutral Sidekick orchestration.
//!
//! This is the intelligence loop owned by Minutes: reducer identity, bounded
//! evidence, correction epochs, intervention policy, and publication. Provider
//! adapters can stream reasoning, but cannot decide what reaches the user.

use super::*;
use crate::context_card::ContextCard;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

const VERIFIER_BASE_INSTRUCTIONS: &str =
    include_str!("../../../../resources/live_sidekick/verifier_base_instructions.txt");
const VERIFIER_DEVELOPER_INSTRUCTIONS: &str =
    include_str!("../../../../resources/live_sidekick/verifier_developer_instructions.txt");

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn contains_any(text: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| text.contains(term))
}

/// Narrow Minutes-owned completeness rule for an automation decision whose
/// evidence explicitly names human handling as the alternative. The semantic
/// verifier still judges all broader factual, contractual, and visual claims.
fn confidence_gate_omits_human_disposition(
    request: &ReasoningTurnRequest,
    candidate: &InterventionCandidate,
) -> bool {
    let mut decision_context = request
        .window
        .transcript
        .iter()
        .map(|item| item.text.to_lowercase())
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(message) = request.typed_user_message.as_deref() {
        decision_context.push('\n');
        decision_context.push_str(&message.to_lowercase());
    }
    let candidate_text = candidate.text.as_deref().unwrap_or_default().to_lowercase();
    let stakeholder_protection_request =
        request
            .typed_user_message
            .as_deref()
            .is_some_and(|message| {
                let message = message.to_lowercase();
                contains_any(
                    &message,
                    &[
                        "procurement",
                        "customer-side",
                        "customer side",
                        "buyer",
                        "contract",
                        "protection",
                        "remedy",
                        "remedies",
                        "audit right",
                        "service-level",
                        "service level",
                        "sla",
                    ],
                )
            });
    if stakeholder_protection_request {
        return false;
    }

    let frames_automation = contains_any(
        &decision_context,
        &[
            "automation",
            "automated",
            "ai-handled",
            "ai handled",
            "ai-resolved",
            "ai resolved",
            "agent-handled",
            "agent handled",
            "model-handled",
            "model handled",
        ],
    );
    let frames_human_alternative = contains_any(
        &decision_context,
        &[
            "human",
            "manual",
            "people",
            "support team",
            "support rep",
            "support agent",
            "specialist",
            "operator",
        ],
    );
    let frames_decision = contains_any(
        &decision_context,
        &[
            "decide",
            "decision",
            "between",
            "versus",
            " vs ",
            "either",
            "instead",
            "alternative",
            "ship",
            "launch",
            "roll out",
            "deploy",
            "keep",
        ],
    );
    let proposes_automation = contains_any(
        &candidate_text,
        &[
            "automation",
            "automated",
            "ai-handled",
            "ai handled",
            "ai-resolved",
            "ai resolved",
            "agent-handled",
            "agent handled",
            "model-handled",
            "model handled",
        ],
    );
    let proposes_confidence_gate = contains_any(
        &candidate_text,
        &[
            "confidence gate",
            "confidence-gated",
            "confidence gated",
            "confidence threshold",
            "confidence-thresholded",
            "confidence band",
            "confidence cutoff",
            "high-confidence",
            "high confidence",
            "low-confidence",
            "low confidence",
            "above-threshold",
            "below-threshold",
        ],
    );
    let names_human = contains_any(
        &candidate_text,
        &[
            "human",
            "manual",
            "people",
            "support team",
            "support rep",
            "support agent",
            "specialist",
            "operator",
        ],
    );
    let names_rejected_work = contains_any(
        &candidate_text,
        &[
            "uncertain",
            "low-confidence",
            "low confidence",
            "below-threshold",
            "below threshold",
            "below the threshold",
            "below it",
            "remainder",
            "remaining",
            "the rest",
            "the balance",
            "everything else",
            "human fallback",
        ],
    );
    let names_disposition = contains_any(
        &candidate_text,
        &[
            "route",
            "routing",
            "send",
            "return",
            "revert",
            "switch",
            "leave",
            "keep",
            "retain",
            "human handling",
            "human review",
            "human fallback",
        ],
    );

    frames_automation
        && frames_human_alternative
        && frames_decision
        && proposes_automation
        && proposes_confidence_gate
        && !(names_human && names_rejected_work && names_disposition)
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
    pub evidence_verification: EvidenceVerificationReceipt,
    pub first_token_ms: Option<u64>,
    pub total_ms: u64,
}

/// Provider-neutral proof that the visible candidate passed an independent,
/// exact-window evidence check. The digest binds the verdict to the candidate
/// bytes without retaining meeting content in diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceVerificationReceipt {
    pub candidate_sha256: String,
    pub verdict: EvidenceVerificationVerdict,
    pub verifier_session_id: ReasoningSessionId,
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
    pub context_evidence_ids: Vec<EvidenceId>,
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

/// Provider-local turn IDs are only unique inside their own session. Wrap
/// callbacks in Minutes-owned lanes so a delayed event from an older verifier
/// session can never impersonate the active verifier, even when both providers
/// reuse the same opaque turn ID.
enum EngineReasoningEvent {
    Strategist(ReasoningStreamEvent),
    Verifier {
        attempt: u64,
        event: ReasoningStreamEvent,
    },
}

struct ActiveReasoning {
    stage: ActiveReasoningStage,
    work: SidekickWork,
    request: ReasoningTurnRequest,
    policy_feedback: Option<String>,
    allowed_evidence_ids: BTreeSet<EvidenceId>,
    allowed_visual_ids: BTreeSet<EvidenceId>,
    /// Revision of the immutable strategist window that produced candidate.
    generation_evidence_revision: u64,
    /// Exact evidence seal independently checked by the active verifier.
    evidence_revision: u64,
    transcript_revision: u64,
    screen_revision: u64,
    /// At most one fresh verifier window is opened after evidence moves. A
    /// continuously finalizing transcript must not hold a supported response
    /// in verification forever; evidence beyond that refreshed seal remains
    /// eligible for the next background decision window.
    verification_refreshes: u8,
    freshness_retries: u8,
    completeness_retries: u8,
    verification_retries: u8,
    carried_total_ms: u64,
    initial_first_token_ms: Option<u64>,
}

enum ActiveReasoningStage {
    Generating {
        turn_id: ReasoningTurnId,
    },
    Verifying {
        turn_id: ReasoningTurnId,
        verifier_attempt: u64,
        candidate: InterventionCandidate,
        generation_result: ReasoningTurnResult,
    },
}

#[derive(Debug, Clone, Copy)]
enum SemanticRetryKind {
    Completeness,
    Verification,
}

#[derive(Debug, Clone, Copy, Default)]
struct TurnRetryState {
    freshness_retries: u8,
    completeness_retries: u8,
    verification_retries: u8,
    carried_total_ms: u64,
    initial_first_token_ms: Option<u64>,
}

impl ActiveReasoningStage {
    fn turn_id(&self) -> &ReasoningTurnId {
        match self {
            Self::Generating { turn_id } | Self::Verifying { turn_id, .. } => turn_id,
        }
    }
}

pub struct LiveSidekickEngine {
    pub session: LiveAssistanceSession,
    backend: Arc<dyn PersistentReasoningBackend>,
    verifier_backend: Arc<dyn PersistentReasoningBackend>,
    backend_session: Option<Box<dyn PersistentReasoningSession>>,
    ready_verifier_session: Option<Box<dyn PersistentReasoningSession>>,
    active_verifier_session: Option<Box<dyn PersistentReasoningSession>>,
    backend_sessions_started: u64,
    verifier_sessions_started: u64,
    descriptor: ReasoningBackendDescriptor,
    config: LiveSidekickEngineConfig,
    /// Local-only source receipts for the active historical context. Provider
    /// adapters receive only the evidence copied from this receipted card.
    context_card: Option<ContextCard>,
    context_evidence: Vec<ReasoningContextEvidence>,
    transcript: VecDeque<ReasoningTranscriptEvidence>,
    authoritative_memory: VecDeque<String>,
    latest_image: Option<ReasoningImageEvidence>,
    event_sender: mpsc::Sender<EngineReasoningEvent>,
    event_receiver: mpsc::Receiver<EngineReasoningEvent>,
    active: Option<ActiveReasoning>,
    publications: VecDeque<SidekickPublication>,
    failures: VecDeque<SidekickFailure>,
    lifecycle_events: VecDeque<SidekickLifecycleEvent>,
    evidence_revision: u64,
    transcript_revision: u64,
    screen_revision: u64,
    last_background_revision: Option<u64>,
    next_run: u64,
    next_turn: u64,
    next_user_event: u64,
    next_verifier_attempt: u64,
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
        Self::new_with_verifier_backend(
            session_id,
            surface,
            role,
            posture,
            Arc::clone(&backend),
            backend,
            config,
        )
    }

    pub fn new_with_verifier_backend(
        session_id: LiveAssistanceSessionId,
        surface: AssistanceSurface,
        role: UserRole,
        posture: AssistancePosture,
        backend: Arc<dyn PersistentReasoningBackend>,
        verifier_backend: Arc<dyn PersistentReasoningBackend>,
        config: LiveSidekickEngineConfig,
    ) -> Result<Self, ReasoningError> {
        config.validate()?;
        let descriptor = backend.descriptor();
        if !descriptor.persistent || !descriptor.streaming {
            return Err(ReasoningError::invalid_request(
                "Sidekick requires a persistent streaming reasoning backend",
            ));
        }
        let verifier_descriptor = verifier_backend.descriptor();
        if !verifier_descriptor.persistent || !verifier_descriptor.streaming {
            return Err(ReasoningError::invalid_request(
                "Sidekick requires a persistent streaming evidence-verifier backend",
            ));
        }
        let (event_sender, event_receiver) = mpsc::channel();
        Ok(Self {
            session: LiveAssistanceSession::new(session_id, surface, role, posture),
            backend,
            verifier_backend,
            backend_session: None,
            ready_verifier_session: None,
            active_verifier_session: None,
            backend_sessions_started: 0,
            verifier_sessions_started: 0,
            descriptor,
            config,
            context_card: None,
            context_evidence: Vec::new(),
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
            transcript_revision: 0,
            screen_revision: 0,
            last_background_revision: None,
            next_run: 1,
            next_turn: 1,
            next_user_event: 1,
            next_verifier_attempt: 1,
        })
    }

    pub fn descriptor(&self) -> &ReasoningBackendDescriptor {
        &self.descriptor
    }

    pub fn verifier_descriptor(&self) -> ReasoningBackendDescriptor {
        self.verifier_backend.descriptor()
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

    /// Number of independent semantic evidence-verifier sessions started.
    /// This remains separate from the persistent strategist session so a
    /// provider's candidate is never accepted on self-attestation alone.
    pub fn verifier_sessions_started(&self) -> u64 {
        self.verifier_sessions_started
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

    /// Load one Minutes-owned historical context card between reasoning turns.
    ///
    /// The persistent provider session is replaced so it cannot retain an
    /// older context window. Live transcript and user corrections remain
    /// Minutes-owned and survive the provider replacement.
    fn load_context_evidence(
        &mut self,
        context: Vec<ReasoningContextEvidence>,
    ) -> Result<(), ReasoningError> {
        self.pump();
        if self.active.is_some() {
            return Err(ReasoningError::new(
                ReasoningErrorKind::Unavailable,
                "Sidekick context refresh is waiting for the active turn to finish",
                true,
            ));
        }
        if !self.context_evidence.is_empty() {
            return Err(ReasoningError::invalid_request(
                "Sidekick context was already loaded for this capture",
            ));
        }
        let mut context_ids = BTreeSet::new();
        if context.iter().any(|evidence| {
            !evidence.evidence_id.as_str().starts_with("context-")
                || !context_ids.insert(evidence.evidence_id.clone())
                || !matches!(
                    evidence.source_kind,
                    EvidenceSourceKind::MeetingArtifact | EvidenceSourceKind::RepositoryResult
                )
                || self.session.evidence.contains_key(&evidence.evidence_id)
        }) {
            return Err(ReasoningError::invalid_request(
                "Sidekick context requires unique context-scoped meeting or repository evidence ids",
            ));
        }
        self.validate_context_load_budget(&context)?;

        for evidence in &context {
            let reduction = self.session.reduce(AssistanceEvent::EvidenceObserved {
                session_id: self.session.id.clone(),
                evidence: UntrustedEvidence {
                    id: evidence.evidence_id.clone(),
                    source_kind: evidence.source_kind,
                    capture_session_id: None,
                    finalized_meeting_ref: None,
                },
            });
            if !reduction.accepted {
                return Err(ReasoningError::invalid_request(format!(
                    "context evidence was rejected: {:?}",
                    reduction.rejection
                )));
            }
        }
        self.context_evidence = context;
        self.evidence_revision = self.evidence_revision.saturating_add(1);
        self.last_background_revision = None;
        Ok(())
    }

    /// Load a fully receipted Minutes context card. Production callers use
    /// this path so every later strategist/verifier turn can re-check the
    /// exact local bytes selected during retrieval.
    pub fn load_context_card(&mut self, card: ContextCard) -> Result<(), ReasoningError> {
        card.validate_sources_current().map_err(|error| {
            ReasoningError::new(
                ReasoningErrorKind::Unavailable,
                format!("historical context failed source validation: {error}"),
                false,
            )
        })?;
        self.load_context_evidence(card.evidence().to_vec())?;
        self.context_card = Some(card);
        Ok(())
    }

    /// Remove historical context after its local source receipts become stale.
    ///
    /// The provider session is replaced only on removal, because an earlier
    /// turn may already contain the now-invalid context in provider history.
    pub fn clear_context_evidence(&mut self) -> Result<(), ReasoningError> {
        self.pump();
        if self.active.is_some() {
            return Err(ReasoningError::new(
                ReasoningErrorKind::Unavailable,
                "Sidekick context invalidation is waiting for the active turn to finish",
                true,
            ));
        }
        if self.context_evidence.is_empty() {
            return Ok(());
        }
        for evidence in &self.context_evidence {
            self.session.evidence.remove(&evidence.evidence_id);
        }
        self.context_evidence.clear();
        self.context_card = None;
        self.evidence_revision = self.evidence_revision.saturating_add(1);
        self.last_background_revision = None;
        self.restart_backend()
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
            self.transcript_revision = self.transcript_revision.saturating_add(1);
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
            self.screen_revision = self.screen_revision.saturating_add(1);
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
                if let ActiveReasoningStage::Generating {
                    turn_id: active_turn_id,
                } = &active.stage
                {
                    if provider.steer_turn(active_turn_id, request.clone()).is_ok() {
                        let (allowed_evidence_ids, allowed_visual_ids) =
                            Self::allowed_provenance(&request);
                        self.active = Some(ActiveReasoning {
                            stage: ActiveReasoningStage::Generating {
                                turn_id: active_turn_id.clone(),
                            },
                            work,
                            policy_feedback: request.policy_feedback.clone(),
                            request,
                            allowed_evidence_ids,
                            allowed_visual_ids,
                            generation_evidence_revision: self.evidence_revision,
                            evidence_revision: self.evidence_revision,
                            transcript_revision: self.transcript_revision,
                            screen_revision: self.screen_revision,
                            verification_refreshes: 0,
                            freshness_retries: 0,
                            completeness_retries: 0,
                            verification_retries: 0,
                            carried_total_ms: 0,
                            initial_first_token_ms: None,
                        });
                        self.remember_user_message(text);
                        return Ok(turn_id);
                    }
                }
            }
        }
        if let Some(active) = self.active.take() {
            let provider = match &active.stage {
                ActiveReasoningStage::Generating { .. } => self.backend_session.as_mut(),
                ActiveReasoningStage::Verifying { .. } => self.active_verifier_session.as_mut(),
            };
            if let Some(provider) = provider {
                let _ = provider.interrupt_turn(active.stage.turn_id());
            }
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
            transcript_evidence_ids: active
                .request
                .window
                .transcript
                .iter()
                .map(|item| item.evidence_id.clone())
                .collect(),
            context_evidence_ids: active
                .request
                .window
                .context
                .iter()
                .map(|item| item.evidence_id.clone())
                .collect(),
            visual_evidence_ids: active.allowed_visual_ids.iter().cloned().collect(),
        })
    }

    pub fn pump(&mut self) {
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                EngineReasoningEvent::Strategist(event) => {
                    self.handle_provider_event(event, None);
                }
                EngineReasoningEvent::Verifier { attempt, event } => {
                    self.handle_provider_event(event, Some(attempt));
                }
            }
        }
    }

    fn handle_provider_event(
        &mut self,
        event: ReasoningStreamEvent,
        verifier_attempt: Option<u64>,
    ) {
        match event {
            ReasoningStreamEvent::TextDelta { .. } => {}
            ReasoningStreamEvent::Completed {
                turn_id,
                invocation,
                result,
            } => self.complete(turn_id, invocation, result, verifier_attempt),
            ReasoningStreamEvent::Failed {
                turn_id,
                invocation,
                error,
            } => self.failed(turn_id, invocation, error, verifier_attempt),
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
        if let Some(mut verifier) = self.active_verifier_session.take() {
            verifier.close();
        }
        if let Some(mut verifier) = self.ready_verifier_session.take() {
            verifier.close();
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
        self.context_evidence.clear();
        self.context_card = None;
        self.authoritative_memory.clear();
        self.latest_image = None;
        self.active = None;
        self.evidence_revision = 0;
        self.transcript_revision = 0;
        self.screen_revision = 0;
        self.last_background_revision = None;
        self.restart_backend()
    }

    fn start_turn(
        &mut self,
        work: SidekickWork,
        prepared_request: Option<ReasoningTurnRequest>,
    ) -> Result<(), ReasoningError> {
        self.start_turn_with_retry(work, prepared_request, TurnRetryState::default())
    }

    fn start_turn_with_retry(
        &mut self,
        work: SidekickWork,
        prepared_request: Option<ReasoningTurnRequest>,
        retry_state: TurnRetryState,
    ) -> Result<(), ReasoningError> {
        let request = match prepared_request {
            Some(request) => request,
            None => self.request_for(work.invocation(), ReasoningTurnKind::Background, None)?,
        };
        self.ensure_context_sources_current()?;
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
                request.clone(),
                Arc::new(move |event| {
                    let _ = sender.send(EngineReasoningEvent::Strategist(event));
                }),
            )?;
        self.active = Some(ActiveReasoning {
            stage: ActiveReasoningStage::Generating { turn_id },
            work,
            policy_feedback: request.policy_feedback.clone(),
            request,
            allowed_evidence_ids,
            allowed_visual_ids,
            generation_evidence_revision: self.evidence_revision,
            evidence_revision: self.evidence_revision,
            transcript_revision: self.transcript_revision,
            screen_revision: self.screen_revision,
            verification_refreshes: 0,
            freshness_retries: retry_state.freshness_retries,
            completeness_retries: retry_state.completeness_retries,
            verification_retries: retry_state.verification_retries,
            carried_total_ms: retry_state.carried_total_ms,
            initial_first_token_ms: retry_state.initial_first_token_ms,
        });
        Ok(())
    }

    fn request_for(
        &self,
        invocation: InvocationIdentity,
        kind: ReasoningTurnKind,
        typed_user_message: Option<String>,
    ) -> Result<ReasoningTurnRequest, ReasoningError> {
        self.ensure_context_sources_current()?;
        let mut transcript: Vec<ReasoningTranscriptEvidence> =
            self.transcript.iter().cloned().collect();
        let mut authoritative_memory: Vec<String> =
            self.authoritative_memory.iter().cloned().collect();
        // A freshness retry happens after the original foreground message was
        // remembered. Keep it in the dedicated typed-authority lane only.
        if let Some(message) = typed_user_message.as_deref() {
            if authoritative_memory
                .last()
                .is_some_and(|item| item == message)
            {
                authoritative_memory.pop();
            }
        }
        let capture_session_id = self.capture_id()?;
        let latest_image = self
            .descriptor
            .image_input
            .then(|| self.latest_image.clone())
            .flatten();
        let prepared_context = self.prepared_context_snapshot();
        // Leave bounded room for the candidate that Minutes will append to
        // the same exact evidence window during independent verification.
        // This prevents a generation request that barely fits from becoming
        // unverifiable solely because its structured candidate adds bytes.
        let verification_reserve = (self.config.max_window_chars / 4).clamp(256, 1_024);
        let generation_budget = self
            .config
            .max_window_chars
            .saturating_sub(verification_reserve);
        let build_request = |transcript: Vec<ReasoningTranscriptEvidence>,
                             authoritative_memory: Vec<String>| {
            ReasoningTurnRequest {
                kind,
                invocation,
                window: BoundedReasoningWindow {
                    capture_session_id: capture_session_id.clone(),
                    context: self.context_evidence.clone(),
                    transcript,
                    latest_image: latest_image.clone(),
                    prepared_context: prepared_context.clone(),
                },
                authoritative_memory,
                typed_user_message: typed_user_message.clone(),
                policy_feedback: None,
                output_contract: ReasoningOutputContract::InterventionCandidateV1,
                candidate_to_verify: None,
            }
        };
        loop {
            let request = build_request(transcript.clone(), authoritative_memory.clone());
            if request.serialized_evidence_chars() <= generation_budget {
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
                return Err(ReasoningError::invalid_request(
                    "fixed Sidekick context leaves no room for evidence verification",
                ));
            }
        }
    }

    fn complete(
        &mut self,
        turn_id: ReasoningTurnId,
        invocation: InvocationIdentity,
        result: ReasoningTurnResult,
        verifier_attempt: Option<u64>,
    ) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if active.stage.turn_id() != &turn_id || active.work.invocation() != invocation {
            return;
        }
        match &active.stage {
            ActiveReasoningStage::Generating { .. } if verifier_attempt.is_none() => {
                self.complete_generation(result);
            }
            ActiveReasoningStage::Verifying {
                verifier_attempt: active_attempt,
                ..
            } if verifier_attempt == Some(*active_attempt) => {
                self.complete_verification(result);
            }
            _ => {}
        }
    }

    fn complete_generation(&mut self, mut result: ReasoningTurnResult) {
        let active = self.active.take().expect("generation completion is active");
        result.first_token_ms = active.initial_first_token_ms.or(result.first_token_ms);
        result.total_ms = active.carried_total_ms.saturating_add(result.total_ms);
        let allowed_evidence_ids = active.allowed_evidence_ids.clone();
        let allowed_visual_ids = active.allowed_visual_ids.clone();
        let Ok(mut candidate) = InterventionCandidate::from_backend_json(&result.text) else {
            self.record_failure(
                active.work,
                ReasoningError::new(
                    ReasoningErrorKind::Protocol,
                    "reasoning backend returned an invalid intervention candidate",
                    true,
                ),
            );
            return;
        };
        if candidate.decision == InterventionDecision::Speak {
            let maximum_words = match &active.work {
                SidekickWork::Background { .. } => MAXIMUM_BACKGROUND_WORDS,
                SidekickWork::Foreground { .. } => MAXIMUM_FOREGROUND_WORDS,
            };
            if let Some(text) = candidate.text.as_deref() {
                if text.split_whitespace().count() > maximum_words {
                    candidate.text = Some(compact_visible_text(text, maximum_words));
                }
            }
        }
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
                active.work,
                ReasoningError::new(
                    ReasoningErrorKind::Protocol,
                    "reasoning candidate cited evidence outside its bounded turn window",
                    false,
                ),
            );
            return;
        }
        if claims_unverified_live_attribution(&candidate, &active.request.window) {
            self.record_failure(
                active.work,
                ReasoningError::new(
                    ReasoningErrorKind::Protocol,
                    "reasoning candidate attributed a live statement to a person known only from historical context",
                    false,
                ),
            );
            return;
        }
        if candidate.decision == InterventionDecision::Silent {
            let processed_evidence_revision = active.evidence_revision;
            self.finalize_candidate(
                active.work,
                candidate,
                result,
                None,
                processed_evidence_revision,
            );
            return;
        }

        self.start_candidate_verification(active, candidate, result);
    }

    fn start_candidate_verification(
        &mut self,
        mut active: ActiveReasoning,
        candidate: InterventionCandidate,
        generation_result: ReasoningTurnResult,
    ) {
        if confidence_gate_omits_human_disposition(&active.request, &candidate) {
            if matches!(active.work, SidekickWork::Foreground { .. })
                && active.completeness_retries == 0
            {
                active.policy_feedback = Some(
                    "The confidence-gated path left rejected work unresolved. Explicitly route uncertain or below-threshold work to a human."
                        .into(),
                );
                self.restart_for_semantic_rejection(
                    active,
                    generation_result,
                    0,
                    SemanticRetryKind::Completeness,
                );
                return;
            }
            if matches!(active.work, SidekickWork::Background { .. }) {
                self.last_background_revision = Some(active.evidence_revision);
            }
            self.reduce_failure(&active.work);
            self.lifecycle_events.push_back(SidekickLifecycleEvent {
                work: active.work,
                outcome: SidekickLifecycleOutcome::Suppressed(
                    CandidateSuppressionReason::UnsupportedSemanticEvidence,
                ),
            });
            return;
        }
        let kind = match &active.work {
            SidekickWork::Background { .. } => ReasoningTurnKind::Background,
            SidekickWork::Foreground { .. } => ReasoningTurnKind::Foreground,
        };
        let mut verification_request = match self.request_for(
            active.work.invocation(),
            kind,
            active.request.typed_user_message.clone(),
        ) {
            Ok(request) => request,
            Err(error) => {
                self.record_failure(active.work, error);
                return;
            }
        };
        // A candidate that declares no pixel reliance is verified without an
        // image. This prevents a fact from being laundered through screen
        // pixels while carrying transcript-only provenance.
        if !candidate.claims_visual_observation {
            verification_request.window.latest_image = None;
        }
        verification_request.output_contract = ReasoningOutputContract::EvidenceVerificationV1;
        verification_request.candidate_to_verify = Some(candidate.clone());
        if let Err(error) = verification_request.validate(self.config.max_window_chars) {
            self.record_failure(active.work, error);
            return;
        }
        let Some(next_verifier_attempt) = self.next_verifier_attempt.checked_add(1) else {
            self.record_failure(
                active.work,
                ReasoningError::new(
                    ReasoningErrorKind::Unavailable,
                    "Sidekick exhausted verifier attempt identities",
                    false,
                ),
            );
            return;
        };
        let verifier_attempt = self.next_verifier_attempt;
        self.next_verifier_attempt = next_verifier_attempt;
        let sender = self.event_sender.clone();
        if self.ready_verifier_session.is_none() {
            if let Err(error) = self.replenish_ready_verifier() {
                self.record_failure(active.work, error);
                return;
            }
        }
        let mut verifier = self
            .ready_verifier_session
            .take()
            .expect("a ready verifier was ensured");
        if let Err(error) = self.ensure_context_sources_current() {
            verifier.close();
            self.record_failure(active.work, error);
            return;
        }
        let verification_turn_id = match verifier.start_turn(
            verification_request.clone(),
            Arc::new(move |event| {
                let _ = sender.send(EngineReasoningEvent::Verifier {
                    attempt: verifier_attempt,
                    event,
                });
            }),
        ) {
            Ok(turn_id) => turn_id,
            Err(error) => {
                verifier.close();
                self.record_failure(active.work, error);
                return;
            }
        };
        self.active_verifier_session = Some(verifier);
        active.stage = ActiveReasoningStage::Verifying {
            turn_id: verification_turn_id,
            verifier_attempt,
            candidate,
            generation_result,
        };
        active.request = verification_request;
        active.evidence_revision = self.evidence_revision;
        active.transcript_revision = self.transcript_revision;
        active.screen_revision = self.screen_revision;
        self.active = Some(active);
        // Do not synchronously create an unrelated future verifier here. A
        // provider handshake can take tens of seconds and would prevent pump()
        // from publishing the verifier result that is already in flight. The
        // next candidate lazily receives its own fresh session.
    }

    fn complete_verification(&mut self, result: ReasoningTurnResult) {
        let mut active = self
            .active
            .take()
            .expect("verification completion is active");
        let verifier_session_id = self
            .active_verifier_session
            .as_ref()
            .map(|session| session.id().clone());
        if let Some(mut verifier) = self.active_verifier_session.take() {
            verifier.close();
        }
        let ActiveReasoningStage::Verifying {
            candidate,
            generation_result,
            ..
        } = &active.stage
        else {
            unreachable!("verification completion requires verification state")
        };
        let transcript_changed = active.transcript_revision != self.transcript_revision;
        let relevant_screen_changed =
            candidate.claims_visual_observation && active.screen_revision != self.screen_revision;
        if (transcript_changed || relevant_screen_changed) && active.verification_refreshes == 0 {
            active.verification_refreshes = active.verification_refreshes.saturating_add(1);
            let candidate = candidate.clone();
            let mut generation_result = generation_result.clone();
            generation_result.total_ms = generation_result.total_ms.saturating_add(result.total_ms);
            self.start_candidate_verification(active, candidate, generation_result);
            return;
        }
        let verified_newer_than_generation =
            active.evidence_revision != active.generation_evidence_revision;
        let Ok(verdict) = EvidenceVerificationVerdict::from_backend_json(&result.text) else {
            self.record_failure(
                active.work,
                ReasoningError::new(
                    ReasoningErrorKind::Protocol,
                    "evidence verifier returned an invalid verdict",
                    true,
                ),
            );
            return;
        };
        if !verdict.allows_publication() {
            if verified_newer_than_generation {
                self.restart_for_fresh_evidence(active, result.total_ms);
                return;
            }
            let retry_kind = if !matches!(active.work, SidekickWork::Foreground { .. }) {
                None
            } else if verdict.reason_code
                == EvidenceVerificationReason::IncompleteMaterialConsequence
                && active.completeness_retries == 0
            {
                Some(SemanticRetryKind::Completeness)
            } else if verdict.reason_code
                != EvidenceVerificationReason::IncompleteMaterialConsequence
                && active.verification_retries == 0
            {
                Some(SemanticRetryKind::Verification)
            } else {
                None
            };
            if let Some(retry_kind) = retry_kind {
                active.policy_feedback = Some(
                    match verdict.reason_code {
                        EvidenceVerificationReason::IncompleteMaterialConsequence => {
                            "The prior candidate omitted a relevant explicitly evidenced material consequence required by the user's request. Re-read the bounded evidence and produce a complete answer without inventing or broadening terms."
                        }
                        _ => {
                            "The prior candidate did not pass independent evidence verification. Re-read the exact bounded evidence, recompute every material claim, remove unsupported or contradictory statements, and answer the user's request fully without inventing facts."
                        }
                    }
                    .into(),
                );
                let retry_generation_result = generation_result.clone();
                self.restart_for_semantic_rejection(
                    active,
                    retry_generation_result,
                    result.total_ms,
                    retry_kind,
                );
                return;
            }
            if matches!(active.work, SidekickWork::Background { .. }) {
                self.last_background_revision = Some(active.evidence_revision);
            }
            self.reduce_failure(&active.work);
            self.lifecycle_events.push_back(SidekickLifecycleEvent {
                work: active.work,
                outcome: SidekickLifecycleOutcome::Suppressed(
                    CandidateSuppressionReason::UnsupportedSemanticEvidence,
                ),
            });
            return;
        }
        let ActiveReasoningStage::Verifying {
            candidate,
            generation_result,
            ..
        } = active.stage
        else {
            unreachable!("verification completion requires verification state")
        };
        let candidate_sha256 = sha256_hex(
            &serde_json::to_vec(&candidate).expect("intervention candidates are serializable"),
        );
        let Some(verifier_session_id) = verifier_session_id else {
            self.record_failure(
                active.work,
                ReasoningError::new(
                    ReasoningErrorKind::Protocol,
                    "evidence verifier session identity was unavailable",
                    false,
                ),
            );
            return;
        };
        self.finalize_candidate(
            active.work,
            candidate,
            ReasoningTurnResult {
                text: generation_result.text,
                first_token_ms: generation_result.first_token_ms,
                total_ms: generation_result.total_ms.saturating_add(result.total_ms),
            },
            Some(EvidenceVerificationReceipt {
                candidate_sha256,
                verdict,
                verifier_session_id,
            }),
            active.evidence_revision,
        );
    }

    fn finalize_candidate(
        &mut self,
        work: SidekickWork,
        candidate: InterventionCandidate,
        result: ReasoningTurnResult,
        evidence_verification: Option<EvidenceVerificationReceipt>,
        processed_evidence_revision: u64,
    ) {
        if let Err(error) = self.ensure_context_sources_current() {
            self.record_failure(work, error);
            return;
        }
        let reduction = match &work {
            SidekickWork::Background { run_id, invocation } => {
                self.last_background_revision = Some(processed_evidence_revision);
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
            let Some(evidence_verification) = evidence_verification else {
                self.record_failure(
                    work,
                    ReasoningError::new(
                        ReasoningErrorKind::Protocol,
                        "visible Sidekick publication has no independent evidence-verification receipt",
                        false,
                    ),
                );
                return;
            };
            self.publications.push_back(SidekickPublication {
                work: work.clone(),
                candidate,
                evidence_verification,
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
        verifier_attempt: Option<u64>,
    ) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if active.stage.turn_id() != &turn_id || active.work.invocation() != invocation {
            return;
        }
        match &active.stage {
            ActiveReasoningStage::Generating { .. } if verifier_attempt.is_none() => {}
            ActiveReasoningStage::Verifying {
                verifier_attempt: active_attempt,
                ..
            } if verifier_attempt == Some(*active_attempt) => {}
            _ => return,
        }
        let active = self.active.take().expect("active checked");
        if matches!(active.stage, ActiveReasoningStage::Verifying { .. }) {
            if let Some(mut verifier) = self.active_verifier_session.take() {
                verifier.close();
            }
        }
        let work = active.work;
        self.record_failure(work, error);
    }

    fn restart_for_fresh_evidence(&mut self, active: ActiveReasoning, verification_total_ms: u64) {
        if matches!(active.stage, ActiveReasoningStage::Verifying { .. }) {
            if let Some(mut verifier) = self.active_verifier_session.take() {
                verifier.close();
            }
        }
        if active.freshness_retries >= 2 {
            self.record_failure(
                active.work,
                ReasoningError::new(
                    ReasoningErrorKind::Unavailable,
                    "Sidekick evidence changed too quickly to verify a current response",
                    true,
                ),
            );
            return;
        }
        let kind = match &active.work {
            SidekickWork::Background { .. } => ReasoningTurnKind::Background,
            SidekickWork::Foreground { .. } => ReasoningTurnKind::Foreground,
        };
        let typed_user_message = active.request.typed_user_message.clone();
        let mut request = match self.request_for(active.work.invocation(), kind, typed_user_message)
        {
            Ok(request) => request,
            Err(error) => {
                self.record_failure(active.work, error);
                return;
            }
        };
        request.policy_feedback = active.policy_feedback.clone();
        if let Err(error) = request.validate(self.config.max_window_chars) {
            self.record_failure(active.work, error);
            return;
        }
        let (carried_total_ms, initial_first_token_ms) = match &active.stage {
            ActiveReasoningStage::Verifying {
                generation_result, ..
            } => (
                generation_result
                    .total_ms
                    .saturating_add(verification_total_ms),
                generation_result.first_token_ms,
            ),
            ActiveReasoningStage::Generating { .. } => {
                (active.carried_total_ms, active.initial_first_token_ms)
            }
        };
        let work = active.work;
        if let Err(error) = self.start_turn_with_retry(
            work.clone(),
            Some(request),
            TurnRetryState {
                freshness_retries: active.freshness_retries.saturating_add(1),
                completeness_retries: active.completeness_retries,
                verification_retries: active.verification_retries,
                carried_total_ms,
                initial_first_token_ms,
            },
        ) {
            self.record_failure(work, error);
        }
    }

    fn restart_for_semantic_rejection(
        &mut self,
        active: ActiveReasoning,
        generation_result: ReasoningTurnResult,
        verification_total_ms: u64,
        retry_kind: SemanticRetryKind,
    ) {
        let kind = ReasoningTurnKind::Foreground;
        let typed_user_message = active.request.typed_user_message.clone();
        let mut request = match self.request_for(active.work.invocation(), kind, typed_user_message)
        {
            Ok(request) => request,
            Err(error) => {
                self.record_failure(active.work, error);
                return;
            }
        };
        request.policy_feedback = active.policy_feedback.clone().or_else(|| {
            Some(
                "The prior candidate omitted a relevant explicitly evidenced material consequence required by the user's request. Re-read the bounded evidence and produce a complete answer without inventing or broadening terms."
                    .into(),
            )
        });
        if let Err(error) = request.validate(self.config.max_window_chars) {
            self.record_failure(active.work, error);
            return;
        }
        let work = active.work;
        if let Err(error) = self.start_turn_with_retry(
            work.clone(),
            Some(request),
            TurnRetryState {
                freshness_retries: active.freshness_retries,
                completeness_retries: active.completeness_retries.saturating_add(u8::from(
                    matches!(retry_kind, SemanticRetryKind::Completeness),
                )),
                verification_retries: active.verification_retries.saturating_add(u8::from(
                    matches!(retry_kind, SemanticRetryKind::Verification),
                )),
                carried_total_ms: generation_result
                    .total_ms
                    .saturating_add(verification_total_ms),
                initial_first_token_ms: generation_result.first_token_ms,
            },
        ) {
            self.record_failure(work, error);
        }
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
        if let Some(mut verifier) = self.active_verifier_session.take() {
            verifier.close();
        }
        if let Some(mut verifier) = self.ready_verifier_session.take() {
            verifier.close();
        }
        let capture_session_id = self.capture_id()?;
        let evidence_scope = ReasoningEvidenceScope {
            capture_session_id,
            source_policy_generation: self.session.source_policy_generation,
        };
        let session = self.backend.start_session(ReasoningSessionConfig {
            base_instructions: self.config.base_instructions.clone(),
            developer_instructions: self.config.developer_instructions.clone(),
            latency_class: ReasoningLatencyClass::Realtime,
            max_window_chars: self.config.max_window_chars,
            ephemeral: true,
            evidence_scope: evidence_scope.clone(),
        })?;
        self.backend_sessions_started = self.backend_sessions_started.saturating_add(1);
        self.backend_session = Some(session);
        if let Err(error) = self.replenish_ready_verifier() {
            if let Some(mut session) = self.backend_session.take() {
                session.close();
            }
            return Err(error);
        }
        Ok(())
    }

    fn replenish_ready_verifier(&mut self) -> Result<(), ReasoningError> {
        if self.ready_verifier_session.is_some() {
            return Ok(());
        }
        let evidence_scope = ReasoningEvidenceScope {
            capture_session_id: self.capture_id()?,
            source_policy_generation: self.session.source_policy_generation,
        };
        let verifier = self
            .verifier_backend
            .start_session(ReasoningSessionConfig {
                base_instructions: VERIFIER_BASE_INSTRUCTIONS.into(),
                developer_instructions: VERIFIER_DEVELOPER_INSTRUCTIONS.into(),
                latency_class: ReasoningLatencyClass::Realtime,
                max_window_chars: self.config.max_window_chars,
                ephemeral: true,
                evidence_scope: evidence_scope.clone(),
            })?;
        // Starting a speculative inference turn here creates a provider race:
        // a verifier needed immediately can arrive before app-server marks the
        // warm-up active, making interrupt fail and causing both requests to
        // share the warm-up event identity. Keep the independent verifier's
        // process and thread hot, but reserve its first turn for real evidence.
        self.verifier_sessions_started = self.verifier_sessions_started.saturating_add(1);
        self.ready_verifier_session = Some(verifier);
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
        let mut transcript = request
            .window
            .transcript
            .iter()
            .map(|item| item.evidence_id.clone())
            .collect::<BTreeSet<_>>();
        transcript.extend(
            request
                .window
                .context
                .iter()
                .map(|item| item.evidence_id.clone()),
        );
        let visual = request
            .window
            .latest_image
            .iter()
            .map(|item| item.evidence_id.clone())
            .collect();
        (transcript, visual)
    }

    fn ensure_context_sources_current(&self) -> Result<(), ReasoningError> {
        let Some(card) = &self.context_card else {
            return Ok(());
        };
        card.validate_sources_current().map_err(|error| {
            ReasoningError::new(
                ReasoningErrorKind::Unavailable,
                format!(
                    "historical context changed before provider egress or publication: {error}"
                ),
                false,
            )
        })
    }

    fn validate_context_load_budget(
        &self,
        context: &[ReasoningContextEvidence],
    ) -> Result<(), ReasoningError> {
        let request = ReasoningTurnRequest {
            kind: ReasoningTurnKind::Background,
            invocation: InvocationIdentity {
                sequence: 1,
                source_policy_generation: self.session.source_policy_generation,
                user_generation: self.session.user_generation,
            },
            window: BoundedReasoningWindow {
                capture_session_id: self.capture_id()?,
                context: context.to_vec(),
                transcript: Vec::new(),
                latest_image: None,
                prepared_context: self.prepared_context_snapshot(),
            },
            authoritative_memory: Vec::new(),
            typed_user_message: None,
            policy_feedback: None,
            output_contract: ReasoningOutputContract::InterventionCandidateV1,
            candidate_to_verify: None,
        };
        request.validate(self.config.max_window_chars)?;
        let verification_reserve = (self.config.max_window_chars / 4).clamp(256, 1_024);
        let generation_budget = self
            .config
            .max_window_chars
            .saturating_sub(verification_reserve);
        if request.serialized_evidence_chars() > generation_budget {
            return Err(ReasoningError::invalid_request(
                "historical context leaves no room for evidence verification",
            ));
        }
        Ok(())
    }
}

fn claims_unverified_live_attribution(
    candidate: &InterventionCandidate,
    window: &BoundedReasoningWindow,
) -> bool {
    let Some(text) = candidate.text.as_deref() else {
        return false;
    };
    let anonymous_live_ids = window
        .transcript
        .iter()
        .filter(|item| !item.speaker_verified)
        .map(|item| &item.evidence_id)
        .collect::<BTreeSet<_>>();
    if !candidate
        .evidence_ids
        .iter()
        .any(|id| anonymous_live_ids.contains(id))
    {
        return false;
    }
    const UNVERIFIED_IDENTITY_CLAIMS: &[&str] = &[
        "anonymous speaker is",
        "the speaker is",
        "current speaker is",
        "live speaker is",
        "person speaking is",
        "this speaker is",
        "the voice is",
        "person who just spoke",
        "person who spoke",
        "is the person speaking",
    ];
    let verified_speakers = window
        .transcript
        .iter()
        .filter(|item| item.speaker_verified)
        .filter_map(|item| item.speaker_label.as_deref())
        .map(normalize_identity_claim_text)
        .collect::<BTreeSet<_>>();
    const HISTORICAL_FRAMES: &[&str] = &[
        "prior meeting",
        "previous meeting",
        "last meeting",
        "earlier meeting",
        "historical context",
        "past notes",
        "prepared brief",
        "previously",
        "historically",
        "prior commitment",
        "prior decision",
        "on january",
        "on february",
        "on march",
        "on april",
        "on may",
        "on june",
        "on july",
        "on august",
        "on september",
        "on october",
        "on november",
        "on december",
    ];
    const ATTRIBUTION_VERBS: &[&str] = &[
        "said",
        "says",
        "asked",
        "asks",
        "approved",
        "approves",
        "agreed",
        "agrees",
        "wants",
        "needs",
        "thinks",
        "believes",
        "owns",
        "committed",
        "commits",
        "proposed",
        "proposes",
        "rejected",
        "rejects",
        "confirmed",
        "confirms",
        "objected",
        "objects",
    ];
    const STRONG_LIVE_MARKERS: &[&str] = &[
        "just",
        "now",
        "currently",
        "speaker",
        "speaking",
        "spoke",
        "voice",
    ];
    let mut historical_labels = window
        .context
        .iter()
        .flat_map(|item| item.subject_labels.iter())
        .map(|label| normalize_identity_claim_text(label))
        .filter(|label| !label.is_empty() && !verified_speakers.contains(label))
        .collect::<BTreeSet<_>>();
    let explicit_aliases = historical_labels
        .iter()
        .flat_map(|label| {
            let words = label.split_whitespace().collect::<Vec<_>>();
            if words.len() < 2 {
                return Vec::new();
            }
            [words.first(), words.last()]
                .into_iter()
                .flatten()
                .filter(|word| word.len() >= 3)
                .map(|word| (*word).to_string())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    historical_labels.extend(explicit_aliases);
    for evidence in &window.context {
        historical_labels.extend(context_identity_phrases(&evidence.text));
    }

    text.split(['.', '!', '?', ';', '\n']).any(|raw_clause| {
        let clause = normalize_identity_claim_text(raw_clause);
        if clause.is_empty() {
            return false;
        }
        let padded_clause = format!(" {clause} ");
        if UNVERIFIED_IDENTITY_CLAIMS
            .iter()
            .any(|claim| padded_clause.contains(&format!(" {claim} ")))
        {
            return true;
        }
        let clause_words = clause.split_whitespace().collect::<Vec<_>>();
        historical_labels.iter().any(|label| {
            let label_words = label.split_whitespace().collect::<Vec<_>>();
            if label_words.is_empty() || label_words.len() > clause_words.len() {
                return false;
            }
            let occurrences = clause_words
                .windows(label_words.len())
                .enumerate()
                .filter(|(_, words)| *words == label_words.as_slice())
                .map(|(start, _)| start)
                .collect::<Vec<_>>();
            occurrences
                .iter()
                .enumerate()
                .any(|(occurrence_index, &start)| {
                    let end = start + label_words.len();
                    let after = &clause_words[end..clause_words.len().min(end + 6)];
                    let has_strong_live_marker =
                        after.iter().any(|word| STRONG_LIVE_MARKERS.contains(word));
                    let has_direct_attribution = after
                        .first()
                        .is_some_and(|word| ATTRIBUTION_VERBS.contains(word))
                        || (after.first() == Some(&"just")
                            && after
                                .get(1)
                                .is_some_and(|word| ATTRIBUTION_VERBS.contains(word)));
                    if !has_strong_live_marker && !has_direct_attribution {
                        return false;
                    }
                    let previous_end = occurrence_index
                        .checked_sub(1)
                        .and_then(|index| occurrences.get(index))
                        .map(|previous| previous + label_words.len())
                        .unwrap_or(0);
                    let history_start = start.saturating_sub(4).max(previous_end);
                    let history_end = clause_words.len().min(end + 6);
                    let local_history =
                        format!(" {} ", clause_words[history_start..history_end].join(" "));
                    let is_explicitly_historical = HISTORICAL_FRAMES
                        .iter()
                        .any(|frame| local_history.contains(&format!(" {frame} ")))
                        || clause_words[history_start..history_end].iter().any(|word| {
                            word.len() == 4 && word.chars().all(|ch| ch.is_ascii_digit())
                        });
                    !is_explicitly_historical
                })
        })
    })
}

fn context_identity_phrases(value: &str) -> BTreeSet<String> {
    const STOPWORDS: &[&str] = &[
        "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "have", "he", "her",
        "his", "i", "in", "is", "it", "my", "of", "on", "or", "our", "she", "that", "the", "their",
        "they", "this", "to", "was", "we", "were", "with", "you", "your",
    ];
    let normalized = normalize_identity_claim_text(value);
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    let mut phrases = BTreeSet::new();
    for size in 2..=3 {
        for window in words.windows(size) {
            if window
                .iter()
                .all(|word| word.len() >= 2 && !STOPWORDS.contains(word))
            {
                phrases.insert(window.join(" "));
            }
        }
    }
    phrases
}

fn normalize_identity_claim_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
        verification_turns: Vec<FakeTurn>,
        verification_turns_started: usize,
        closed_sessions: Vec<String>,
        steer_fails: bool,
        defer_verification: bool,
        reuse_verifier_turn_ids: bool,
        verification_verdicts: VecDeque<EvidenceVerificationVerdict>,
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

        fn complete_verification(&self, index: usize, verdict: EvidenceVerificationVerdict) {
            let state = lock(&self.state);
            let turn = &state.verification_turns[index];
            turn.sink.on_event(ReasoningStreamEvent::Completed {
                turn_id: turn.id.clone(),
                invocation: turn.request.invocation,
                result: ReasoningTurnResult {
                    text: serde_json::to_string(&verdict).unwrap(),
                    first_token_ms: Some(50),
                    total_ms: 100,
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
            let mut state = lock(&self.state);
            state.sessions_started += 1;
            let session_number = state.sessions_started;
            drop(state);
            Ok(Box::new(FakeSession {
                id: ReasoningSessionId::new(format!("fake-session-{session_number}")),
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
            if request.output_contract == ReasoningOutputContract::EvidenceVerificationV1 {
                state.verification_turns_started += 1;
                let is_warmup = request
                    .candidate_to_verify
                    .as_ref()
                    .is_some_and(|candidate| {
                        candidate
                            .evidence_ids
                            .iter()
                            .any(|id| id.as_str() == "synthetic-verifier-warmup")
                    });
                let id = if state.reuse_verifier_turn_ids {
                    ReasoningTurnId::new("provider-local-turn-1")
                } else {
                    ReasoningTurnId::new(format!(
                        "fake-verification-{}",
                        state.verification_verdicts.len() + state.turns.len() + 1
                    ))
                };
                let verdict = (!is_warmup)
                    .then(|| state.verification_verdicts.pop_front())
                    .flatten()
                    .unwrap_or(EvidenceVerificationVerdict {
                        decision: EvidenceVerificationDecision::Allow,
                        reason_code: EvidenceVerificationReason::Supported,
                    });
                let invocation = request.invocation;
                if !is_warmup && state.defer_verification {
                    state.verification_turns.push(FakeTurn {
                        id: id.clone(),
                        request,
                        sink,
                    });
                    return Ok(id);
                }
                drop(state);
                sink.on_event(ReasoningStreamEvent::Completed {
                    turn_id: id.clone(),
                    invocation,
                    result: ReasoningTurnResult {
                        text: serde_json::to_string(&verdict).unwrap(),
                        first_token_ms: Some(50),
                        total_ms: 100,
                    },
                });
                return Ok(id);
            }
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

        fn close(&mut self) {
            lock(&self.state)
                .closed_sessions
                .push(self.id.as_str().to_string());
        }
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
    fn historical_context_is_bounded_citable_and_separate_from_live_speakers() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        engine
            .load_context_evidence(vec![ReasoningContextEvidence {
                evidence_id: "context-prior-decision".into(),
                source_id: "source-prior-meeting".into(),
                source_kind: EvidenceSourceKind::MeetingArtifact,
                context_class: "decision".into(),
                text: "The prior meeting approved a reversible pilot.".into(),
                evidence_only: true,
                subject_labels: vec!["Sam Lee".into()],
            }])
            .unwrap();
        observe(
            &mut engine,
            "live-anonymous",
            "I want to revisit the rollout.",
        );

        let turn_id = engine.send_user("What should I protect?").unwrap();
        let state = lock(&backend.state);
        let request = &state.turns.last().unwrap().request;
        assert_eq!(request.window.context.len(), 1);
        assert_eq!(
            request.window.context[0].evidence_id.as_str(),
            "context-prior-decision"
        );
        assert_eq!(
            request.window.transcript[0].speaker_label.as_deref(),
            None,
            "history must not identify an anonymous live speaker"
        );
        drop(state);

        let receipt = engine.foreground_evidence_receipt(&turn_id).unwrap();
        assert_eq!(
            receipt.context_evidence_ids,
            vec![EvidenceId::new("context-prior-decision")]
        );
        assert_eq!(
            receipt.transcript_evidence_ids,
            vec![EvidenceId::new("live-anonymous")]
        );
    }

    fn receipted_context_card(path: &std::path::Path, text: &str) -> ContextCard {
        std::fs::write(path, text).unwrap();
        let mut request = crate::context_card::ContextCardRequest::new("pricing");
        request.prepared_brief_path = Some(path.to_path_buf());
        ContextCard::assemble(&crate::config::Config::default(), request).unwrap()
    }

    #[test]
    fn context_source_change_blocks_internal_verifier_egress_and_publication() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("meeting.md");
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        let card = receipted_context_card(&source, "Sam Lee supported the prior pilot.");
        let context_id = card.evidence()[0].evidence_id.clone();
        engine.load_context_card(card).unwrap();
        observe(&mut engine, "live", "I support the pilot.");
        engine.send_user("What changed?").unwrap();

        std::fs::write(
            &source,
            "---\nsensitivity: restricted\n---\nrestricted source\n",
        )
        .unwrap();
        backend.complete(
            0,
            speak(&[context_id.as_str(), "live"], "The pilot has support."),
        );

        let failures = engine.take_failures();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].error.message.contains("provider egress"));
        assert!(engine.take_publications().is_empty());
        assert_eq!(
            lock(&backend.state).turns.len(),
            1,
            "a stale card must not reach the independent verifier"
        );
    }

    #[test]
    fn oversized_context_is_rejected_atomically_before_it_can_poison_turns() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        let error = engine
            .load_context_evidence(vec![ReasoningContextEvidence {
                evidence_id: "context-too-large".into(),
                source_id: "source-too-large".into(),
                source_kind: EvidenceSourceKind::MeetingArtifact,
                context_class: "decision".into(),
                text: "x".repeat(350),
                evidence_only: true,
                subject_labels: vec!["Sam Lee".into()],
            }])
            .unwrap_err();
        assert!(error.message.contains("evidence verification"));

        observe(&mut engine, "live", "Current evidence remains usable.");
        engine.send_user("Can you still help?").unwrap();
        assert!(lock(&backend.state).turns[0]
            .request
            .window
            .context
            .is_empty());
    }

    #[test]
    fn policy_invalidation_clears_historical_context() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        engine
            .load_context_evidence(vec![ReasoningContextEvidence {
                evidence_id: "context-prior".into(),
                source_id: "source-prior".into(),
                source_kind: EvidenceSourceKind::MeetingArtifact,
                context_class: "decision".into(),
                text: "A historical decision.".into(),
                evidence_only: true,
                subject_labels: vec!["Sam Lee".into()],
            }])
            .unwrap();

        engine.invalidate_source_policy(1).unwrap();
        observe(&mut engine, "current", "Current evidence.");
        engine.send_user("What is current?").unwrap();
        let state = lock(&backend.state);
        assert!(state
            .turns
            .last()
            .unwrap()
            .request
            .window
            .context
            .is_empty());
    }

    #[test]
    fn history_cannot_identify_an_unverified_live_speaker() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        engine
            .load_context_evidence(vec![ReasoningContextEvidence {
                evidence_id: "context-sam".into(),
                source_id: "source-sam".into(),
                source_kind: EvidenceSourceKind::MeetingArtifact,
                context_class: "prior_meeting".into(),
                text: "Sam Lee attended the prior meeting.".into(),
                evidence_only: true,
                subject_labels: vec!["Sam Lee".into()],
            }])
            .unwrap();
        observe(&mut engine, "live", "I approve the pilot.");
        engine.send_user("What changed?").unwrap();
        backend.complete(
            0,
            speak(
                &["context-sam", "live"],
                "Sam Lee said the pilot is approved.",
            ),
        );

        let failures = engine.take_failures();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].error.message.contains("historical context"));
        assert!(engine.take_publications().is_empty());
    }

    #[test]
    fn history_can_support_strategy_about_a_named_person() {
        let window = BoundedReasoningWindow {
            capture_session_id: "capture-a".into(),
            context: vec![ReasoningContextEvidence {
                evidence_id: "context-sam".into(),
                source_id: "source-sam".into(),
                source_kind: EvidenceSourceKind::MeetingArtifact,
                context_class: "commitment".into(),
                text: "Sam Lee owns the follow-up.".into(),
                evidence_only: true,
                subject_labels: vec!["Sam Lee".into()],
            }],
            transcript: vec![ReasoningTranscriptEvidence {
                evidence_id: "live".into(),
                text: "What should we ask next?".into(),
                speaker_label: None,
                speaker_verified: false,
                offset_ms: 0,
                duration_ms: 1,
            }],
            latest_image: None,
            prepared_context: String::new(),
        };
        let candidate = |text| speak(&["context-sam", "live"], text);
        assert!(!claims_unverified_live_attribution(
            &candidate("Ask Sam Lee about the prior follow-up."),
            &window
        ));
        assert!(!claims_unverified_live_attribution(
            &candidate("Send Sam Lee the recap."),
            &window
        ));
        assert!(claims_unverified_live_attribution(
            &candidate("The anonymous speaker is Sam Lee."),
            &window
        ));
        assert!(claims_unverified_live_attribution(
            &candidate("Sam Lee wants the pilot."),
            &window
        ));
        assert!(claims_unverified_live_attribution(
            &candidate("The person who just spoke is Sam Lee."),
            &window
        ));
        assert!(!claims_unverified_live_attribution(
            &candidate("In the prior meeting, Sam Lee owned the follow-up."),
            &window
        ));
        assert!(!claims_unverified_live_attribution(
            &candidate("On June 11, Sam Lee owned the follow-up."),
            &window
        ));
        assert!(claims_unverified_live_attribution(
            &candidate("In the prior meeting Sam Lee owned pricing; Sam Lee just approved it."),
            &window
        ));
        assert!(claims_unverified_live_attribution(
            &candidate("Ask Sam Lee why they just approved."),
            &window
        ));
        assert!(claims_unverified_live_attribution(
            &candidate("Sam just approved the pilot."),
            &window
        ));
        assert!(claims_unverified_live_attribution(
            &candidate("Historically Sam Lee owned pricing, and Sam Lee just approved the pilot."),
            &window
        ));
        assert!(claims_unverified_live_attribution(
            &candidate("Historically Sam Lee owned pricing and Sam Lee just approved the pilot."),
            &window
        ));
        assert!(claims_unverified_live_attribution(
            &candidate("Historically Sam Lee agreed and Sam just approved."),
            &window
        ));

        let mut lowercase_window = window.clone();
        lowercase_window.context[0].text = "sam lee attended the prepared customer review".into();
        lowercase_window.context[0].subject_labels = vec!["sam lee".into()];
        assert!(claims_unverified_live_attribution(
            &candidate("sam lee just approved the pilot."),
            &lowercase_window
        ));
        assert!(claims_unverified_live_attribution(
            &candidate("sam just approved the pilot."),
            &lowercase_window
        ));

        let mut short_name_window = window.clone();
        short_name_window.context[0].text = "Ann owned the prior follow-up.".into();
        short_name_window.context[0].subject_labels = vec!["Ann".into()];
        assert!(!claims_unverified_live_attribution(
            &candidate("Will this annual plan work?"),
            &short_name_window
        ));
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
        assert_eq!(engine.verifier_sessions_started(), 1);
        assert_eq!(
            engine
                .reasoning_session_id()
                .map(ReasoningSessionId::as_str),
            Some("fake-session-1")
        );

        engine.invalidate_source_policy(1).unwrap();
        assert_eq!(engine.reasoning_sessions_started(), 2);
        assert_eq!(engine.verifier_sessions_started(), 2);
        assert_eq!(lock(&backend.state).sessions_started, 4);
    }

    #[test]
    fn ready_verifier_reserves_its_first_turn_for_real_evidence() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());

        assert_eq!(lock(&backend.state).verification_turns_started, 0);
        observe(
            &mut engine,
            "decision",
            "The team approved the staged rollout.",
        );
        engine.send_user("What was approved?").unwrap();
        backend.complete(
            0,
            speak(&["decision"], "The team approved the staged rollout."),
        );

        assert_eq!(engine.take_publications().len(), 1);
        assert_eq!(lock(&backend.state).verification_turns_started, 1);
    }

    #[test]
    fn incomplete_foreground_candidate_gets_one_policy_guided_retry() {
        let backend = FakeBackend::default();
        {
            let mut state = lock(&backend.state);
            state
                .verification_verdicts
                .push_back(EvidenceVerificationVerdict {
                    decision: EvidenceVerificationDecision::Reject,
                    reason_code: EvidenceVerificationReason::IncompleteMaterialConsequence,
                });
            state
                .verification_verdicts
                .push_back(EvidenceVerificationVerdict {
                    decision: EvidenceVerificationDecision::Allow,
                    reason_code: EvidenceVerificationReason::Supported,
                });
        }
        let mut engine = engine(backend.clone());
        observe(
            &mut engine,
            "remedy",
            "The vendor owes the customer $200 for every wrong automated resolution.",
        );
        engine
            .send_user("Now advise me as the customer procurement lead.")
            .unwrap();
        backend.complete(0, speak(&["remedy"], "Require audit rights."));
        assert!(engine.has_active_turn());
        {
            let state = lock(&backend.state);
            assert_eq!(state.turns.len(), 2);
            assert!(state.turns[1]
                .request
                .policy_feedback
                .as_deref()
                .unwrap()
                .contains(
                    "prior candidate omitted a relevant explicitly evidenced material consequence"
                ));
        }

        backend.complete(
            1,
            speak(
                &["remedy"],
                "Require the vendor to owe the customer $200 for every wrong automated resolution.",
            ),
        );
        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].first_token_ms, Some(250));
        assert_eq!(publications[0].total_ms, 1_200);
    }

    #[test]
    fn unsupported_foreground_candidate_gets_one_fresh_grounded_retry() {
        let backend = FakeBackend::default();
        {
            let mut state = lock(&backend.state);
            state
                .verification_verdicts
                .push_back(EvidenceVerificationVerdict {
                    decision: EvidenceVerificationDecision::Reject,
                    reason_code: EvidenceVerificationReason::UnsupportedFact,
                });
            state
                .verification_verdicts
                .push_back(EvidenceVerificationVerdict {
                    decision: EvidenceVerificationDecision::Allow,
                    reason_code: EvidenceVerificationReason::Supported,
                });
        }
        let mut engine = engine(backend.clone());
        observe(
            &mut engine,
            "decision",
            "The team approved a staged rollout.",
        );
        engine.send_user("What was approved?").unwrap();
        backend.complete(
            0,
            speak(&["decision"], "The team approved a blanket rollout."),
        );
        assert!(engine.has_active_turn());
        {
            let state = lock(&backend.state);
            assert_eq!(state.turns.len(), 2);
            assert!(state.turns[1]
                .request
                .policy_feedback
                .as_deref()
                .unwrap()
                .contains("did not pass independent evidence verification"));
        }

        backend.complete(
            1,
            speak(&["decision"], "The team approved a staged rollout."),
        );
        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        assert_eq!(
            publications[0].candidate.text.as_deref(),
            Some("The team approved a staged rollout.")
        );
        assert_eq!(lock(&backend.state).verification_turns_started, 2);
    }

    #[test]
    fn confidence_gate_without_human_disposition_retries_before_model_verification() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        observe(
            &mut engine,
            "decision",
            "We must decide between full automation and keeping a human in the loop.",
        );
        engine
            .send_user("What is the real risk, and what should I ask?")
            .unwrap();
        backend.complete(
            0,
            speak(
                &["decision"],
                "Stage automated resolution behind a confidence gate. Ask for the confidence-band error distribution.",
            ),
        );
        assert!(engine.has_active_turn());

        {
            let state = lock(&backend.state);
            assert_eq!(state.verification_turns_started, 0);
            assert_eq!(state.turns.len(), 2);
            assert!(state.turns[1]
                .request
                .policy_feedback
                .as_deref()
                .unwrap()
                .contains("route uncertain or below-threshold work to a human"));
        }

        backend.complete(
            1,
            speak(
                &["decision"],
                "Ship confidence-gated automation and route below-threshold work to a human.",
            ),
        );
        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        assert_eq!(lock(&backend.state).verification_turns_started, 1);
    }

    #[test]
    fn completeness_and_verifier_recovery_have_separate_bounded_budgets() {
        let backend = FakeBackend::default();
        {
            let mut state = lock(&backend.state);
            state
                .verification_verdicts
                .push_back(EvidenceVerificationVerdict {
                    decision: EvidenceVerificationDecision::Reject,
                    reason_code: EvidenceVerificationReason::UnsupportedFact,
                });
            state
                .verification_verdicts
                .push_back(EvidenceVerificationVerdict {
                    decision: EvidenceVerificationDecision::Allow,
                    reason_code: EvidenceVerificationReason::Supported,
                });
        }
        let mut engine = engine(backend.clone());
        observe(
            &mut engine,
            "decision",
            "We must decide between full automation and keeping a human in the loop.",
        );
        engine.send_user("What is the real risk?").unwrap();
        backend.complete(
            0,
            speak(
                &["decision"],
                "Stage automated resolution behind a confidence gate.",
            ),
        );
        assert!(engine.has_active_turn());

        backend.complete(
            1,
            speak(
                &["decision"],
                "Ship confidence-gated automation and route below-threshold work to humans.",
            ),
        );
        assert!(engine.has_active_turn());
        assert_eq!(lock(&backend.state).turns.len(), 3);

        backend.complete(
            2,
            speak(
                &["decision"],
                "Ship confidence-gated automation and route below-threshold work to humans.",
            ),
        );
        assert_eq!(engine.take_publications().len(), 1);
        assert_eq!(lock(&backend.state).verification_turns_started, 2);
    }

    #[test]
    fn procurement_protections_remain_subject_to_the_semantic_verifier() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        observe(
            &mut engine,
            "decision",
            "We must decide between full automation and keeping a human in the loop.",
        );
        engine
            .send_user("Advise me as the customer procurement lead. What protections do I need?")
            .unwrap();
        backend.complete(
            0,
            speak(
                &["decision"],
                "Require a confidence-threshold SLA and a unilateral right to revert affected work to humans.",
            ),
        );

        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        assert_eq!(lock(&backend.state).verification_turns_started, 1);
    }

    #[test]
    fn completeness_then_freshness_preserves_policy_and_full_latency() {
        let backend = FakeBackend::default();
        lock(&backend.state).defer_verification = true;
        let mut engine = engine(backend.clone());
        observe(&mut engine, "e1", "A $200 remedy applies.");
        engine.send_user("Advise me on the decision.").unwrap();
        backend.complete(0, speak(&["e1"], "Require audit rights."));
        assert!(engine.has_active_turn());
        backend.complete_verification(
            0,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Reject,
                reason_code: EvidenceVerificationReason::IncompleteMaterialConsequence,
            },
        );
        assert!(engine.has_active_turn());

        backend.complete(1, speak(&["e1"], "Preserve the $200 remedy."));
        assert!(engine.has_active_turn());
        observe(&mut engine, "e2", "The first correction changes scope.");
        backend.complete_verification(
            1,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        assert!(engine.has_active_turn());
        observe(
            &mut engine,
            "e3",
            "The second correction changes scope again.",
        );
        backend.complete_verification(
            2,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Reject,
                reason_code: EvidenceVerificationReason::Contradiction,
            },
        );
        assert!(engine.has_active_turn());
        {
            let state = lock(&backend.state);
            assert!(state.turns[2]
                .request
                .policy_feedback
                .as_deref()
                .unwrap()
                .contains("prior candidate omitted"));
        }

        backend.complete(
            2,
            speak(&["e1", "e3"], "Preserve the corrected $200 remedy."),
        );
        assert!(engine.has_active_turn());
        backend.complete_verification(
            3,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].first_token_ms, Some(250));
        assert_eq!(publications[0].total_ms, 1_900);
    }

    #[test]
    fn freshness_then_completeness_preserves_full_latency() {
        let backend = FakeBackend::default();
        lock(&backend.state).defer_verification = true;
        let mut engine = engine(backend.clone());
        observe(&mut engine, "e1", "A $200 remedy applies.");
        engine.send_user("Advise me on the decision.").unwrap();
        backend.complete(0, speak(&["e1"], "Preserve the remedy."));
        assert!(engine.has_active_turn());
        observe(&mut engine, "e2", "The first correction changes scope.");
        backend.complete_verification(
            0,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        assert!(engine.has_active_turn());
        observe(
            &mut engine,
            "e3",
            "The second correction changes scope again.",
        );
        backend.complete_verification(
            1,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Reject,
                reason_code: EvidenceVerificationReason::Contradiction,
            },
        );
        assert!(engine.has_active_turn());

        backend.complete(1, speak(&["e3"], "Require audit rights."));
        assert!(engine.has_active_turn());
        backend.complete_verification(
            2,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Reject,
                reason_code: EvidenceVerificationReason::IncompleteMaterialConsequence,
            },
        );
        assert!(engine.has_active_turn());
        backend.complete(
            2,
            speak(&["e1", "e3"], "Preserve the corrected $200 remedy."),
        );
        assert!(engine.has_active_turn());
        backend.complete_verification(
            3,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].first_token_ms, Some(250));
        assert_eq!(publications[0].total_ms, 1_900);
    }

    #[test]
    fn overlong_provider_candidate_is_compacted_before_verification_and_publication() {
        let backend = FakeBackend::default();
        let mut engine = engine(backend.clone());
        observe(
            &mut engine,
            "exposure",
            "The contract creates an $800,000 monthly exposure.",
        );
        engine.evaluate_background().unwrap().unwrap();
        let filler = std::iter::repeat_n("material", 55)
            .collect::<Vec<_>>()
            .join(" ");
        backend.complete(
            0,
            speak(
                &["exposure"],
                &format!(
                    "The contractual exposure is $800,000 monthly. {filler}. What confidence threshold changes the decision?"
                ),
            ),
        );

        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        let text = publications[0].candidate.text.as_deref().unwrap();
        assert!(text.split_whitespace().count() <= MAXIMUM_BACKGROUND_WORDS);
        assert!(text.starts_with("The contractual exposure is $800,000 monthly."));
        assert!(text.ends_with("What confidence threshold changes the decision?"));
    }

    #[test]
    fn independent_verifier_blocks_real_but_irrelevant_receipt_laundering() {
        let backend = FakeBackend::default();
        lock(&backend.state).verification_verdicts.extend([
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Reject,
                reason_code: EvidenceVerificationReason::UnsupportedFact,
            },
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Reject,
                reason_code: EvidenceVerificationReason::UnsupportedFact,
            },
        ]);
        let mut engine = engine(backend.clone());
        observe(&mut engine, "weather", "Nice weather today.");
        let turn_id = engine.send_user("What did they approve?").unwrap();
        backend.complete(
            0,
            speak(
                &["weather"],
                "They approved a one million dollar commitment.",
            ),
        );
        assert!(engine.has_active_turn());
        backend.complete(
            1,
            speak(
                &["weather"],
                "They approved a one million dollar commitment.",
            ),
        );

        assert!(engine.take_publications().is_empty());
        let events = engine.take_lifecycle_events();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            SidekickLifecycleEvent {
                work: SidekickWork::Foreground {
                    turn_id: actual_turn_id,
                    ..
                },
                outcome: SidekickLifecycleOutcome::Suppressed(
                    CandidateSuppressionReason::UnsupportedSemanticEvidence,
                ),
            } if actual_turn_id == &turn_id
        ));
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
        assert_eq!(lock(&backend.state).sessions_started, 4);
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
        engine.config.max_window_chars = 1_400;
        engine.authoritative_memory.push_back("m".repeat(600));
        observe(&mut engine, "large-fact", &"t".repeat(600));

        engine.send_user("What changed?").unwrap();
        let state = lock(&backend.state);
        let request = &state.turns[0].request;
        assert!(request.serialized_evidence_chars() <= 1_050);
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
        lock(&backend.state).defer_verification = true;
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
        backend.complete(
            0,
            InterventionCandidate {
                decision: InterventionDecision::Speak,
                kind: Some("insight".into()),
                text: Some("The first screen says something.".into()),
                evidence_ids: Vec::new(),
                visual_evidence_ids: vec!["screen-first".into()],
                claims_visual_observation: true,
                confidence: 90,
            },
        );
        assert!(engine.has_active_turn());
        engine
            .observe_screen("screen-second".into(), second.clone())
            .unwrap();
        backend.complete_verification(
            0,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        assert!(engine.has_active_turn());
        assert!(engine.take_publications().is_empty());
        {
            let state = lock(&backend.state);
            assert_eq!(state.turns.len(), 1);
            assert_eq!(
                state.verification_turns[1]
                    .request
                    .window
                    .latest_image
                    .as_ref()
                    .unwrap()
                    .evidence_id
                    .as_str(),
                "screen-second"
            );
        }
        backend.complete_verification(
            1,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Reject,
                reason_code: EvidenceVerificationReason::Contradiction,
            },
        );
        assert!(engine.has_active_turn());
        backend.complete(
            1,
            InterventionCandidate {
                decision: InterventionDecision::Speak,
                kind: Some("insight".into()),
                text: Some("The current screen says something.".into()),
                evidence_ids: Vec::new(),
                visual_evidence_ids: vec!["screen-second".into()],
                claims_visual_observation: true,
                confidence: 90,
            },
        );
        assert!(engine.has_active_turn());
        backend.complete_verification(
            2,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        assert_eq!(engine.take_publications().len(), 1);
        assert!(engine
            .take_lifecycle_events()
            .iter()
            .all(|event| !matches!(event.outcome, SidekickLifecycleOutcome::Failed(_))));

        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_file(second);
    }

    #[test]
    fn transcript_correction_during_verification_restarts_on_the_latest_window() {
        let backend = FakeBackend::default();
        lock(&backend.state).defer_verification = true;
        let mut engine = engine(backend.clone());
        observe(&mut engine, "approval", "The launch is approved.");
        engine.send_user("Should we proceed?").unwrap();
        backend.complete(0, speak(&["approval"], "Proceed; the launch is approved."));
        assert!(engine.has_active_turn());

        observe(
            &mut engine,
            "correction",
            "That authorization has been rescinded.",
        );
        backend.complete_verification(
            0,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        assert!(engine.has_active_turn());
        assert!(engine.take_publications().is_empty());
        {
            let state = lock(&backend.state);
            assert_eq!(state.turns.len(), 1);
            assert_eq!(
                state.verification_turns[1]
                    .request
                    .window
                    .transcript
                    .last()
                    .unwrap()
                    .evidence_id
                    .as_str(),
                "correction"
            );
        }

        backend.complete_verification(
            1,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Reject,
                reason_code: EvidenceVerificationReason::Contradiction,
            },
        );
        assert!(engine.has_active_turn());

        backend.complete(1, speak(&["correction"], "Stop; approval was withdrawn."));
        assert!(engine.has_active_turn());
        backend.complete_verification(
            2,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        assert_eq!(
            publications[0].candidate.text.as_deref(),
            Some("Stop; approval was withdrawn.")
        );
    }

    #[test]
    fn old_verifier_event_cannot_impersonate_a_refreshed_session_that_reuses_turn_ids() {
        let backend = FakeBackend::default();
        {
            let mut state = lock(&backend.state);
            state.defer_verification = true;
            state.reuse_verifier_turn_ids = true;
        }
        let mut engine = engine(backend.clone());
        observe(&mut engine, "approval", "The launch is approved.");
        engine.send_user("Should we proceed?").unwrap();
        backend.complete(0, speak(&["approval"], "Proceed; the launch is approved."));
        assert!(engine.has_active_turn());

        observe(
            &mut engine,
            "correction",
            "That authorization has been rescinded.",
        );
        backend.complete_verification(
            0,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        assert!(engine.has_active_turn());
        {
            let state = lock(&backend.state);
            assert_eq!(state.verification_turns.len(), 2);
            assert_eq!(
                state.verification_turns[0].id,
                state.verification_turns[1].id
            );
        }

        // A delayed duplicate from verifier A has the same provider-local
        // turn ID and invocation as verifier B. Its Minutes-owned attempt lane
        // must still make it stale.
        backend.complete_verification(
            0,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        assert!(engine.take_publications().is_empty());
        assert!(engine.has_active_turn());

        backend.complete_verification(
            1,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Reject,
                reason_code: EvidenceVerificationReason::Contradiction,
            },
        );
        assert!(engine.has_active_turn());
        backend.complete(1, speak(&["correction"], "Stop; approval was withdrawn."));
        assert!(engine.has_active_turn());
        backend.complete_verification(
            2,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        assert_eq!(
            publications[0].candidate.text.as_deref(),
            Some("Stop; approval was withdrawn.")
        );
        assert_eq!(
            publications[0]
                .evidence_verification
                .verifier_session_id
                .as_str(),
            "fake-session-4"
        );
    }

    #[test]
    fn continuous_live_transcript_churn_publishes_after_one_fresh_verifier_window() {
        let backend = FakeBackend::default();
        lock(&backend.state).defer_verification = true;
        let mut engine = engine(backend.clone());
        observe(&mut engine, "approval", "The launch is approved.");
        engine.evaluate_background().unwrap().unwrap();

        observe(
            &mut engine,
            "routine-1",
            "Routine live transcript movement one.",
        );
        backend.complete(0, speak(&["approval"], "Proceed; the launch is approved."));
        assert!(engine.has_active_turn());
        assert_eq!(engine.verifier_sessions_started(), 1);

        observe(
            &mut engine,
            "routine-2",
            "Routine live transcript movement two.",
        );
        backend.complete_verification(
            0,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );

        assert!(engine.has_active_turn());
        assert!(engine.take_publications().is_empty());
        {
            let state = lock(&backend.state);
            assert_eq!(state.turns.len(), 1);
            assert_eq!(state.verification_turns.len(), 2);
            assert_eq!(
                state.verification_turns[1]
                    .request
                    .window
                    .transcript
                    .last()
                    .unwrap()
                    .evidence_id
                    .as_str(),
                "routine-2"
            );
        }
        observe(
            &mut engine,
            "routine-3",
            "Routine live transcript movement three.",
        );
        backend.complete_verification(
            1,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );

        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        assert_eq!(
            publications[0].candidate.text.as_deref(),
            Some("Proceed; the launch is approved.")
        );
        let state = lock(&backend.state);
        assert_eq!(
            state.turns.len(),
            1,
            "routine chatter must not restart generation"
        );
        assert_eq!(
            state.sessions_started, 3,
            "publication must not wait for a future verifier session handshake"
        );
        drop(state);
        assert!(
            engine.evaluate_background().unwrap().is_some(),
            "evidence newer than the bounded verifier seal must remain eligible for the next decision window"
        );
        assert_eq!(lock(&backend.state).turns.len(), 2);
    }

    #[test]
    fn typed_question_invalidates_an_old_verifier_event_already_in_the_queue() {
        let backend = FakeBackend::default();
        lock(&backend.state).defer_verification = true;
        let mut engine = engine(backend.clone());
        observe(&mut engine, "fact", "A material decision is pending.");
        let first = engine.send_user("Question A").unwrap();
        backend.complete(0, speak(&["fact"], "Answer A."));
        assert!(engine.has_active_turn());
        backend.complete_verification(
            0,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );

        let second = engine.send_user("Question B").unwrap();
        assert!(engine.take_publications().is_empty());
        assert!(engine.foreground_evidence_receipt(&first).is_none());
        assert!(engine.foreground_evidence_receipt(&second).is_some());

        backend.complete(1, speak(&["fact"], "Answer B."));
        assert!(engine.has_active_turn());
        backend.complete_verification(
            1,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        let publications = engine.take_publications();
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].candidate.text.as_deref(), Some("Answer B."));
    }

    #[test]
    fn each_candidate_uses_a_fresh_verifier_with_only_its_bounded_window() {
        let backend = FakeBackend::default();
        lock(&backend.state).defer_verification = true;
        let mut engine = engine(backend.clone());
        engine.config.max_transcript_items = 1;

        observe(
            &mut engine,
            "old-approval",
            "The $1M commitment is approved.",
        );
        engine.send_user("What was approved?").unwrap();
        backend.complete(
            0,
            speak(&["old-approval"], "The $1M commitment is approved."),
        );
        assert!(engine.has_active_turn());
        backend.complete_verification(
            0,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Allow,
                reason_code: EvidenceVerificationReason::Supported,
            },
        );
        let first = engine.take_publications().pop().unwrap();
        let first_verifier = first.evidence_verification.verifier_session_id;

        observe(&mut engine, "weather", "Nice weather today.");
        engine.send_user("What is approved now?").unwrap();
        backend.complete(1, speak(&["weather"], "The $1M commitment is approved."));
        assert!(engine.has_active_turn());
        let second_verifier = engine
            .active_verifier_session
            .as_ref()
            .unwrap()
            .id()
            .clone();
        assert_ne!(first_verifier, second_verifier);
        {
            let state = lock(&backend.state);
            assert_eq!(
                state.verification_turns[1]
                    .request
                    .window
                    .transcript
                    .iter()
                    .map(|item| item.evidence_id.as_str())
                    .collect::<Vec<_>>(),
                vec!["weather"]
            );
            assert!(state
                .closed_sessions
                .contains(&first_verifier.as_str().to_string()));
        }
        backend.complete_verification(
            1,
            EvidenceVerificationVerdict {
                decision: EvidenceVerificationDecision::Reject,
                reason_code: EvidenceVerificationReason::UnsupportedFact,
            },
        );
        assert!(engine.take_publications().is_empty());
    }
}
