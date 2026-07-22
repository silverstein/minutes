use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub(crate) fn is_valid(&self) -> bool {
                !self.0.trim().is_empty()
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }
    };
}

string_id!(LiveAssistanceSessionId);
string_id!(CaptureSessionId);
string_id!(ForegroundTurnId);
string_id!(BackgroundRunId);
string_id!(EvidenceId);
string_id!(MeetingRef);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistanceScope {
    LiveCapture,
    FinalizedMeeting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistanceSurface {
    TerminalSidekick,
    CoachHud,
    NativeRecall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistancePhase {
    Idle,
    Ready,
    MeetingEnded,
    Processing,
    Finalized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    Presenter,
    Participant,
    Observer,
    DecisionMaker,
    TechnicalResponder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistancePosture {
    OnDemand,
    Strategist,
    SilentWatch,
    DecisionTracker,
}

impl AssistancePosture {
    fn permits_background(self) -> bool {
        !matches!(self, Self::OnDemand)
    }
}

/// The capture entry point. Both variants have identical normalized live
/// evidence semantics; `Recording` only promises an additional durable-media
/// lifecycle outside this reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    Live,
    Recording,
}

impl CaptureMode {
    pub fn normalized_semantics(self) -> NormalizedCaptureSemantics {
        NormalizedCaptureSemantics::LiveTranscript
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizedCaptureSemantics {
    LiveTranscript,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSourceKind {
    TranscriptFinal,
    ScreenImage,
    DesktopMetadata,
    MeetingArtifact,
    CoachNudge,
    RepositoryResult,
    UserStatement,
}

impl EvidenceSourceKind {
    fn requires_capture_session(self) -> bool {
        matches!(
            self,
            Self::TranscriptFinal | Self::ScreenImage | Self::DesktopMetadata | Self::CoachNudge
        )
    }
}

/// A provenance-only reference to untrusted evidence. The reducer never turns
/// this data into an external mutation action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UntrustedEvidence {
    pub id: EvidenceId,
    pub source_kind: EvidenceSourceKind,
    pub capture_session_id: Option<CaptureSessionId>,
    #[serde(default)]
    pub finalized_meeting_ref: Option<MeetingRef>,
}

/// Reducer-issued identity for exactly one inference invocation.
///
/// Provider adapters must echo the complete value on completion. The
/// monotonically increasing sequence prevents an old completion from being
/// mistaken for a newer invocation when a caller reuses a turn or run ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvocationIdentity {
    pub sequence: u64,
    pub source_policy_generation: u64,
    pub user_generation: u64,
}

impl InvocationIdentity {
    pub(crate) fn is_valid(self) -> bool {
        self.sequence != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupersedingValue<T> {
    pub value: T,
    pub revision: u64,
    pub supersedes_revision: Option<u64>,
    pub source_event_id: Option<EvidenceId>,
}

impl<T> SupersedingValue<T> {
    fn initial(value: T) -> Self {
        Self {
            value,
            revision: 0,
            supersedes_revision: None,
            source_event_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakerCorrection {
    pub source_label: String,
    pub corrected_label: String,
    pub revision: u64,
    pub supersedes_revision: Option<u64>,
    pub source_event_id: EvidenceId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForegroundTurn {
    pub id: ForegroundTurnId,
    pub source_event_id: EvidenceId,
    pub invocation: InvocationIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundRun {
    pub id: BackgroundRunId,
    pub invocation: InvocationIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveAssistanceSession {
    pub id: LiveAssistanceSessionId,
    pub scope: AssistanceScope,
    pub surface: AssistanceSurface,
    pub phase: AssistancePhase,
    pub user_role: SupersedingValue<UserRole>,
    pub posture: AssistancePosture,
    pub capture_mode: Option<CaptureMode>,
    pub capture_session_id: Option<CaptureSessionId>,
    pub finalized_meeting_ref: Option<MeetingRef>,
    pub source_policy_generation: u64,
    pub user_generation: u64,
    pub foreground_turn: Option<ForegroundTurn>,
    pub background_run: Option<BackgroundRun>,
    /// Immutable evidence provenance keyed by an event ID that is unique
    /// within the retained source-policy generation.
    pub evidence: BTreeMap<EvidenceId, UntrustedEvidence>,
    pub speaker_corrections: BTreeMap<String, SpeakerCorrection>,
    #[serde(default = "default_next_invocation_sequence")]
    next_invocation_sequence: u64,
    #[serde(default = "default_next_correction_revision")]
    next_correction_revision: u64,
}

const fn default_next_invocation_sequence() -> u64 {
    1
}

const fn default_next_correction_revision() -> u64 {
    1
}

impl LiveAssistanceSession {
    pub fn new(
        id: LiveAssistanceSessionId,
        surface: AssistanceSurface,
        user_role: UserRole,
        posture: AssistancePosture,
    ) -> Self {
        Self {
            id,
            scope: AssistanceScope::LiveCapture,
            surface,
            phase: AssistancePhase::Idle,
            user_role: SupersedingValue::initial(user_role),
            posture,
            capture_mode: None,
            capture_session_id: None,
            finalized_meeting_ref: None,
            source_policy_generation: 0,
            user_generation: 0,
            foreground_turn: None,
            background_run: None,
            evidence: BTreeMap::new(),
            speaker_corrections: BTreeMap::new(),
            next_invocation_sequence: default_next_invocation_sequence(),
            next_correction_revision: default_next_correction_revision(),
        }
    }

    /// Apply one already-ordered event. The returned action order is part of
    /// the contract: internal cancellation precedes the next visible user
    /// acknowledgement, and background publication is impossible after user
    /// generation changes.
    pub fn reduce(&mut self, event: AssistanceEvent) -> Reduction {
        if !self.id.is_valid() || !event.session_id().is_valid() {
            return Reduction::rejected(RejectionReason::InvalidValue);
        }
        if event.session_id() != &self.id {
            return Reduction::rejected(RejectionReason::WrongAssistanceSession);
        }

        match event {
            AssistanceEvent::CaptureStarted {
                capture_session_id,
                mode,
                ..
            } => self.capture_started(capture_session_id, mode),
            AssistanceEvent::EvidenceObserved { evidence, .. } => self.evidence_observed(evidence),
            AssistanceEvent::UserMessage {
                turn_id,
                source_event_id,
                text,
                ..
            } => self.user_message(turn_id, source_event_id, text),
            AssistanceEvent::RoleCorrected {
                role,
                source_event_id,
                ..
            } => self.role_corrected(role, source_event_id),
            AssistanceEvent::PostureChanged { posture, .. } => {
                if !self.accepts_user_control() {
                    return Reduction::rejected(RejectionReason::InvalidTransition);
                }
                if self.posture == posture {
                    return Reduction::rejected(RejectionReason::NoStateChange);
                }
                let Some(next_user_generation) = self.user_generation.checked_add(1) else {
                    return Reduction::rejected(RejectionReason::GenerationExhausted);
                };
                self.user_generation = next_user_generation;
                self.posture = posture;
                let mut actions = self.cancel_in_flight(InvalidationReason::PostureChanged);
                actions.push(AssistanceAction::PostureUpdated { posture });
                Reduction::accepted(actions)
            }
            AssistanceEvent::SpeakerCorrected {
                source_label,
                corrected_label,
                source_event_id,
                ..
            } => self.speaker_corrected(source_label, corrected_label, source_event_id),
            AssistanceEvent::BackgroundStarted { run_id, .. } => self.background_started(run_id),
            AssistanceEvent::BackgroundCompleted {
                run_id,
                invocation,
                candidate,
                ..
            } => self.background_completed(run_id, invocation, candidate),
            AssistanceEvent::ForegroundCompleted {
                turn_id,
                invocation,
                candidate,
                ..
            } => self.foreground_completed(turn_id, invocation, candidate),
            AssistanceEvent::BackgroundFailed {
                run_id, invocation, ..
            } => self.background_failed(run_id, invocation),
            AssistanceEvent::ForegroundFailed {
                turn_id,
                invocation,
                ..
            } => self.foreground_failed(turn_id, invocation),
            AssistanceEvent::SourcePolicyInvalidated { new_generation, .. } => {
                self.policy_invalidated(new_generation)
            }
            AssistanceEvent::CaptureStopped {
                capture_session_id, ..
            } => self.capture_stopped(capture_session_id),
            AssistanceEvent::ProcessingStarted {
                capture_session_id, ..
            } => self.processing_started(capture_session_id),
            AssistanceEvent::MeetingFinalized {
                capture_session_id,
                meeting_ref,
                ..
            } => self.meeting_finalized(capture_session_id, meeting_ref),
        }
    }

    fn capture_started(
        &mut self,
        capture_session_id: CaptureSessionId,
        mode: CaptureMode,
    ) -> Reduction {
        if self.phase != AssistancePhase::Idle {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        if !capture_session_id.is_valid() {
            return Reduction::rejected(RejectionReason::InvalidValue);
        }
        self.capture_session_id = Some(capture_session_id.clone());
        self.capture_mode = Some(mode);
        self.scope = AssistanceScope::LiveCapture;
        self.phase = AssistancePhase::Ready;
        Reduction::accepted(vec![AssistanceAction::LiveTranscriptAttached {
            capture_session_id,
        }])
    }

    fn evidence_observed(&mut self, evidence: UntrustedEvidence) -> Reduction {
        if !self.accepts_evidence(evidence.source_kind) {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        if !evidence.id.is_valid()
            || evidence
                .capture_session_id
                .as_ref()
                .is_some_and(|capture_id| !capture_id.is_valid())
            || evidence
                .finalized_meeting_ref
                .as_ref()
                .is_some_and(|meeting_ref| !meeting_ref.is_valid())
        {
            return Reduction::rejected(RejectionReason::InvalidValue);
        }
        if evidence.source_kind == EvidenceSourceKind::UserStatement {
            return Reduction::rejected(RejectionReason::TypedUserEventRequired);
        }
        match self.phase {
            AssistancePhase::Ready
            | AssistancePhase::MeetingEnded
            | AssistancePhase::Processing => {
                let missing_required_capture = evidence.source_kind.requires_capture_session()
                    && evidence.capture_session_id.is_none();
                let mismatched_capture = evidence
                    .capture_session_id
                    .as_ref()
                    .is_some_and(|capture_id| Some(capture_id) != self.capture_session_id.as_ref());
                if missing_required_capture || mismatched_capture {
                    return Reduction::rejected(RejectionReason::WrongCaptureSession);
                }
                if evidence.finalized_meeting_ref.is_some() {
                    return Reduction::rejected(RejectionReason::WrongFinalizedMeeting);
                }
            }
            AssistancePhase::Finalized => {
                if evidence.capture_session_id.is_some() {
                    return Reduction::rejected(RejectionReason::WrongCaptureSession);
                }
                if evidence.finalized_meeting_ref.as_ref() != self.finalized_meeting_ref.as_ref() {
                    return Reduction::rejected(RejectionReason::WrongFinalizedMeeting);
                }
            }
            AssistancePhase::Idle => {
                return Reduction::rejected(RejectionReason::InvalidTransition);
            }
        }
        if self.evidence.contains_key(&evidence.id) {
            return Reduction::rejected(RejectionReason::DuplicateEvidence);
        }
        self.evidence.insert(evidence.id.clone(), evidence.clone());
        Reduction::accepted(vec![AssistanceAction::EvidenceAccepted {
            evidence_id: evidence.id,
            source_kind: evidence.source_kind,
        }])
    }

    fn user_message(
        &mut self,
        turn_id: ForegroundTurnId,
        source_event_id: EvidenceId,
        text: String,
    ) -> Reduction {
        if !self.accepts_user_control() {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        if !turn_id.is_valid() || !source_event_id.is_valid() || text.trim().is_empty() {
            return Reduction::rejected(RejectionReason::InvalidValue);
        }
        if self.evidence.contains_key(&source_event_id) {
            return Reduction::rejected(RejectionReason::DuplicateEvidence);
        }
        if self
            .foreground_turn
            .as_ref()
            .is_some_and(|turn| turn.id == turn_id)
        {
            return Reduction::rejected(RejectionReason::DuplicateTurn);
        }

        let Some(next_user_generation) = self.user_generation.checked_add(1) else {
            return Reduction::rejected(RejectionReason::GenerationExhausted);
        };
        let Some((invocation, next_invocation_sequence)) =
            self.next_invocation_identity(next_user_generation)
        else {
            return Reduction::rejected(RejectionReason::GenerationExhausted);
        };
        self.user_generation = next_user_generation;
        self.next_invocation_sequence = next_invocation_sequence;
        let mut actions = self.cancel_background(InvalidationReason::TypedUserInput);
        if let Some(previous) = self.foreground_turn.take() {
            actions.push(AssistanceAction::CancelForeground {
                turn_id: previous.id,
                invocation: previous.invocation,
                reason: InvalidationReason::TypedUserInput,
            });
        }

        self.insert_user_statement(source_event_id.clone());
        self.foreground_turn = Some(ForegroundTurn {
            id: turn_id.clone(),
            source_event_id: source_event_id.clone(),
            invocation,
        });
        actions.push(AssistanceAction::AcknowledgeForeground {
            turn_id: turn_id.clone(),
            invocation,
        });
        actions.push(AssistanceAction::RequestReadOnlyForegroundInference {
            turn_id,
            invocation,
            source_event_id,
            text,
        });
        Reduction::accepted(actions)
    }

    fn role_corrected(&mut self, role: UserRole, source_event_id: EvidenceId) -> Reduction {
        if !self.accepts_user_control() {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        if !source_event_id.is_valid() {
            return Reduction::rejected(RejectionReason::InvalidValue);
        }
        if self.user_role.value == role {
            return Reduction::rejected(RejectionReason::InvalidCorrection);
        }
        if self.evidence.contains_key(&source_event_id) {
            return Reduction::rejected(RejectionReason::DuplicateEvidence);
        }
        let Some(next_user_generation) = self.user_generation.checked_add(1) else {
            return Reduction::rejected(RejectionReason::GenerationExhausted);
        };
        let Some((revision, next_correction_revision)) = self.next_correction_revision() else {
            return Reduction::rejected(RejectionReason::GenerationExhausted);
        };
        self.user_generation = next_user_generation;
        self.next_correction_revision = next_correction_revision;
        let mut actions = self.cancel_in_flight(InvalidationReason::Correction);
        let supersedes_revision = Some(self.user_role.revision);
        self.insert_user_statement(source_event_id.clone());
        self.user_role = SupersedingValue {
            value: role,
            revision,
            supersedes_revision,
            source_event_id: Some(source_event_id),
        };
        actions.push(AssistanceAction::RoleUpdated {
            role,
            revision,
            supersedes_revision,
        });
        Reduction::accepted(actions)
    }

    fn speaker_corrected(
        &mut self,
        source_label: String,
        corrected_label: String,
        source_event_id: EvidenceId,
    ) -> Reduction {
        if !self.accepts_user_control() {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        if !source_event_id.is_valid()
            || source_label.trim().is_empty()
            || corrected_label.trim().is_empty()
        {
            return Reduction::rejected(RejectionReason::InvalidValue);
        }
        if source_label == corrected_label
            || self
                .speaker_corrections
                .get(&source_label)
                .is_some_and(|prior| prior.corrected_label == corrected_label)
        {
            return Reduction::rejected(RejectionReason::InvalidCorrection);
        }
        if self.evidence.contains_key(&source_event_id) {
            return Reduction::rejected(RejectionReason::DuplicateEvidence);
        }
        let Some(next_user_generation) = self.user_generation.checked_add(1) else {
            return Reduction::rejected(RejectionReason::GenerationExhausted);
        };
        let Some((revision, next_correction_revision)) = self.next_correction_revision() else {
            return Reduction::rejected(RejectionReason::GenerationExhausted);
        };
        self.user_generation = next_user_generation;
        self.next_correction_revision = next_correction_revision;
        let mut actions = self.cancel_in_flight(InvalidationReason::Correction);
        let supersedes_revision = self
            .speaker_corrections
            .get(&source_label)
            .map(|prior| prior.revision);
        let correction = SpeakerCorrection {
            source_label: source_label.clone(),
            corrected_label: corrected_label.clone(),
            revision,
            supersedes_revision,
            source_event_id: source_event_id.clone(),
        };
        self.insert_user_statement(source_event_id);
        self.speaker_corrections
            .insert(source_label.clone(), correction);
        actions.push(AssistanceAction::SpeakerCorrectionUpdated {
            source_label,
            corrected_label,
            revision,
            supersedes_revision,
        });
        Reduction::accepted(actions)
    }

    fn background_started(&mut self, run_id: BackgroundRunId) -> Reduction {
        if self.phase != AssistancePhase::Ready
            || !self.posture.permits_background()
            || self.foreground_turn.is_some()
        {
            return Reduction::rejected(RejectionReason::BackgroundNotAllowed);
        }
        if self.background_run.is_some() {
            return Reduction::rejected(RejectionReason::BackgroundAlreadyRunning);
        }
        if !run_id.is_valid() {
            return Reduction::rejected(RejectionReason::InvalidValue);
        }
        let Some((invocation, next_invocation_sequence)) =
            self.next_invocation_identity(self.user_generation)
        else {
            return Reduction::rejected(RejectionReason::GenerationExhausted);
        };
        self.next_invocation_sequence = next_invocation_sequence;
        self.background_run = Some(BackgroundRun {
            id: run_id.clone(),
            invocation,
        });
        Reduction::accepted(vec![AssistanceAction::BackgroundInvocationRegistered {
            run_id,
            invocation,
        }])
    }

    fn background_completed(
        &mut self,
        run_id: BackgroundRunId,
        invocation: InvocationIdentity,
        candidate: InterventionCandidate,
    ) -> Reduction {
        if !run_id.is_valid() || !invocation.is_valid() {
            return Reduction::rejected(RejectionReason::InvalidValue);
        }
        let Some(run) = self.background_run.as_ref() else {
            return Reduction::rejected(RejectionReason::StaleBackgroundResult);
        };
        if run.id != run_id
            || run.invocation != invocation
            || invocation.user_generation != self.user_generation
            || invocation.source_policy_generation != self.source_policy_generation
            || self.foreground_turn.is_some()
            || self.phase != AssistancePhase::Ready
        {
            return Reduction::rejected(RejectionReason::StaleBackgroundResult);
        }
        self.background_run = None;
        match self.validate_candidate(&candidate, true) {
            Ok(()) => Reduction::accepted(vec![AssistanceAction::PublishBackgroundInsight {
                run_id,
                invocation,
                candidate,
            }]),
            Err(reason) => Reduction::accepted(vec![AssistanceAction::SuppressCandidate {
                invocation,
                reason,
            }]),
        }
    }

    fn foreground_completed(
        &mut self,
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
        candidate: InterventionCandidate,
    ) -> Reduction {
        if !turn_id.is_valid() || !invocation.is_valid() {
            return Reduction::rejected(RejectionReason::InvalidValue);
        }
        let Some(turn) = self.foreground_turn.as_ref() else {
            return Reduction::rejected(RejectionReason::StaleForegroundResult);
        };
        if turn.id != turn_id
            || turn.invocation != invocation
            || invocation.source_policy_generation != self.source_policy_generation
            || invocation.user_generation != self.user_generation
            || !self.accepts_user_control()
        {
            return Reduction::rejected(RejectionReason::StaleForegroundResult);
        }
        self.foreground_turn = None;
        match self.validate_candidate(&candidate, false) {
            Ok(()) => Reduction::accepted(vec![AssistanceAction::PublishForegroundResponse {
                turn_id,
                invocation,
                candidate,
            }]),
            Err(reason) => Reduction::accepted(vec![AssistanceAction::SuppressCandidate {
                invocation,
                reason,
            }]),
        }
    }

    fn background_failed(
        &mut self,
        run_id: BackgroundRunId,
        invocation: InvocationIdentity,
    ) -> Reduction {
        let Some(run) = self.background_run.as_ref() else {
            return Reduction::rejected(RejectionReason::StaleBackgroundResult);
        };
        if run.id != run_id || run.invocation != invocation {
            return Reduction::rejected(RejectionReason::StaleBackgroundResult);
        }
        self.background_run = None;
        Reduction::accepted(vec![AssistanceAction::SuppressCandidate {
            invocation,
            reason: CandidateSuppressionReason::BackendFailure,
        }])
    }

    fn foreground_failed(
        &mut self,
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
    ) -> Reduction {
        let Some(turn) = self.foreground_turn.as_ref() else {
            return Reduction::rejected(RejectionReason::StaleForegroundResult);
        };
        if turn.id != turn_id || turn.invocation != invocation {
            return Reduction::rejected(RejectionReason::StaleForegroundResult);
        }
        self.foreground_turn = None;
        Reduction::accepted(vec![AssistanceAction::SuppressCandidate {
            invocation,
            reason: CandidateSuppressionReason::BackendFailure,
        }])
    }

    fn validate_candidate(
        &self,
        candidate: &InterventionCandidate,
        background: bool,
    ) -> Result<(), CandidateSuppressionReason> {
        if candidate.confidence > 100
            || (candidate.decision == InterventionDecision::Speak
                && candidate
                    .text
                    .as_deref()
                    .is_none_or(|text| text.trim().is_empty()))
            || (candidate.decision == InterventionDecision::Silent && candidate.text.is_some())
        {
            return Err(CandidateSuppressionReason::InvalidCandidate);
        }
        if candidate
            .evidence_ids
            .iter()
            .any(|id| !self.evidence.contains_key(id))
            || candidate.visual_evidence_ids.iter().any(|id| {
                self.evidence
                    .get(id)
                    .is_none_or(|evidence| evidence.source_kind != EvidenceSourceKind::ScreenImage)
            })
        {
            return Err(CandidateSuppressionReason::UnsupportedProvenance);
        }
        if candidate.claims_visual_observation == candidate.visual_evidence_ids.is_empty() {
            return Err(CandidateSuppressionReason::UnsupportedProvenance);
        }
        if candidate.decision == InterventionDecision::Silent {
            return Err(CandidateSuppressionReason::ModelChoseSilence);
        }
        if background
            && candidate.evidence_ids.is_empty()
            && candidate.visual_evidence_ids.is_empty()
        {
            return Err(CandidateSuppressionReason::UnsupportedProvenance);
        }
        if background && candidate.confidence < MINIMUM_PROACTIVE_CONFIDENCE {
            return Err(CandidateSuppressionReason::BelowInterventionThreshold);
        }
        Ok(())
    }

    fn policy_invalidated(&mut self, new_generation: u64) -> Reduction {
        if self.phase == AssistancePhase::Idle {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        if new_generation <= self.source_policy_generation {
            return Reduction::rejected(RejectionReason::PolicyGenerationNotAdvanced);
        }
        let mut actions = self.cancel_background(InvalidationReason::SourcePolicyChanged);
        if let Some(turn) = self.foreground_turn.take() {
            actions.push(AssistanceAction::CancelForeground {
                turn_id: turn.id,
                invocation: turn.invocation,
                reason: InvalidationReason::SourcePolicyChanged,
            });
        }
        self.source_policy_generation = new_generation;
        self.evidence.clear();
        self.speaker_corrections.clear();
        self.user_role.source_event_id = None;
        actions.push(AssistanceAction::PolicyBoundStateCleared { new_generation });
        Reduction::accepted(actions)
    }

    fn capture_stopped(&mut self, capture_session_id: CaptureSessionId) -> Reduction {
        if !capture_session_id.is_valid() {
            return Reduction::rejected(RejectionReason::InvalidValue);
        }
        if !self.matches_capture(&capture_session_id) {
            return Reduction::rejected(RejectionReason::WrongCaptureSession);
        }
        if self.phase != AssistancePhase::Ready {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        let mut actions = self.cancel_in_flight(InvalidationReason::MeetingEnded);
        self.phase = AssistancePhase::MeetingEnded;
        actions.push(AssistanceAction::MeetingEnded);
        Reduction::accepted(actions)
    }

    fn processing_started(&mut self, capture_session_id: CaptureSessionId) -> Reduction {
        if !capture_session_id.is_valid() {
            return Reduction::rejected(RejectionReason::InvalidValue);
        }
        if !self.matches_capture(&capture_session_id) {
            return Reduction::rejected(RejectionReason::WrongCaptureSession);
        }
        if self.phase != AssistancePhase::MeetingEnded {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        self.phase = AssistancePhase::Processing;
        Reduction::accepted(vec![AssistanceAction::FinalTranscriptProcessing])
    }

    fn meeting_finalized(
        &mut self,
        capture_session_id: CaptureSessionId,
        meeting_ref: MeetingRef,
    ) -> Reduction {
        if !capture_session_id.is_valid() || !meeting_ref.is_valid() {
            return Reduction::rejected(RejectionReason::InvalidValue);
        }
        if !self.matches_capture(&capture_session_id) {
            return Reduction::rejected(RejectionReason::WrongCaptureSession);
        }
        if !matches!(
            self.phase,
            AssistancePhase::MeetingEnded | AssistancePhase::Processing
        ) {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        let mut actions = self.cancel_in_flight(InvalidationReason::LifecycleChanged);
        self.scope = AssistanceScope::FinalizedMeeting;
        self.phase = AssistancePhase::Finalized;
        self.finalized_meeting_ref = Some(meeting_ref.clone());
        actions.push(AssistanceAction::FinalizedMeetingAttached { meeting_ref });
        Reduction::accepted(actions)
    }

    fn matches_capture(&self, capture_session_id: &CaptureSessionId) -> bool {
        self.capture_session_id.as_ref() == Some(capture_session_id)
    }

    fn cancel_background(&mut self, reason: InvalidationReason) -> Vec<AssistanceAction> {
        self.background_run
            .take()
            .map(|run| {
                vec![AssistanceAction::CancelBackground {
                    run_id: run.id,
                    invocation: run.invocation,
                    reason,
                }]
            })
            .unwrap_or_default()
    }

    fn cancel_in_flight(&mut self, reason: InvalidationReason) -> Vec<AssistanceAction> {
        let mut actions = self.cancel_background(reason);
        if let Some(turn) = self.foreground_turn.take() {
            actions.push(AssistanceAction::CancelForeground {
                turn_id: turn.id,
                invocation: turn.invocation,
                reason,
            });
        }
        actions
    }

    fn next_invocation_identity(&self, user_generation: u64) -> Option<(InvocationIdentity, u64)> {
        let sequence = self.next_invocation_sequence;
        if sequence == 0 {
            return None;
        }
        let next_sequence = sequence.checked_add(1)?;
        Some((
            InvocationIdentity {
                sequence,
                source_policy_generation: self.source_policy_generation,
                user_generation,
            },
            next_sequence,
        ))
    }

    fn next_correction_revision(&self) -> Option<(u64, u64)> {
        let revision = self.next_correction_revision;
        if revision == 0 {
            return None;
        }
        Some((revision, revision.checked_add(1)?))
    }

    fn accepts_user_control(&self) -> bool {
        self.phase != AssistancePhase::Idle
    }

    fn accepts_evidence(&self, source_kind: EvidenceSourceKind) -> bool {
        // Capture-bound visual/utterance evidence is live-lifecycle only.
        // Finalized sessions accept only evidence that can be rebound to the
        // exact finalized meeting reference in `evidence_observed`.
        match self.phase {
            AssistancePhase::Ready => true,
            AssistancePhase::MeetingEnded | AssistancePhase::Processing => matches!(
                source_kind,
                EvidenceSourceKind::TranscriptFinal
                    | EvidenceSourceKind::MeetingArtifact
                    | EvidenceSourceKind::RepositoryResult
            ),
            AssistancePhase::Finalized => matches!(
                source_kind,
                EvidenceSourceKind::MeetingArtifact | EvidenceSourceKind::RepositoryResult
            ),
            AssistancePhase::Idle => false,
        }
    }

    fn insert_user_statement(&mut self, source_event_id: EvidenceId) {
        let evidence = UntrustedEvidence {
            id: source_event_id.clone(),
            source_kind: EvidenceSourceKind::UserStatement,
            capture_session_id: if self.scope == AssistanceScope::LiveCapture {
                self.capture_session_id.clone()
            } else {
                None
            },
            finalized_meeting_ref: if self.scope == AssistanceScope::FinalizedMeeting {
                self.finalized_meeting_ref.clone()
            } else {
                None
            },
        };
        self.evidence.insert(source_event_id, evidence);
    }
}

const MINIMUM_PROACTIVE_CONFIDENCE: u8 = 70;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterventionDecision {
    Silent,
    Speak,
}

/// A backend proposal. Minutes still validates provenance and decides whether
/// anything is allowed to reach the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterventionCandidate {
    pub decision: InterventionDecision,
    pub kind: Option<String>,
    pub text: Option<String>,
    pub evidence_ids: Vec<EvidenceId>,
    pub visual_evidence_ids: Vec<EvidenceId>,
    /// Explicit structural declaration that visible response text relies on
    /// pixels from the exact-session image supplied for this turn.
    pub claims_visual_observation: bool,
    pub confidence: u8,
}

impl InterventionCandidate {
    /// Parse an untrusted backend proposal. Publication remains impossible
    /// until the reducer validates this candidate against active identity,
    /// policy, confidence, and evidence provenance.
    pub fn from_backend_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSuppressionReason {
    ModelChoseSilence,
    BelowInterventionThreshold,
    UnsupportedProvenance,
    InvalidCandidate,
    BackendFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistanceEvent {
    CaptureStarted {
        session_id: LiveAssistanceSessionId,
        capture_session_id: CaptureSessionId,
        mode: CaptureMode,
    },
    EvidenceObserved {
        session_id: LiveAssistanceSessionId,
        evidence: UntrustedEvidence,
    },
    UserMessage {
        session_id: LiveAssistanceSessionId,
        turn_id: ForegroundTurnId,
        source_event_id: EvidenceId,
        text: String,
    },
    RoleCorrected {
        session_id: LiveAssistanceSessionId,
        role: UserRole,
        source_event_id: EvidenceId,
    },
    PostureChanged {
        session_id: LiveAssistanceSessionId,
        posture: AssistancePosture,
    },
    SpeakerCorrected {
        session_id: LiveAssistanceSessionId,
        source_label: String,
        corrected_label: String,
        source_event_id: EvidenceId,
    },
    BackgroundStarted {
        session_id: LiveAssistanceSessionId,
        run_id: BackgroundRunId,
    },
    BackgroundCompleted {
        session_id: LiveAssistanceSessionId,
        run_id: BackgroundRunId,
        invocation: InvocationIdentity,
        candidate: InterventionCandidate,
    },
    ForegroundCompleted {
        session_id: LiveAssistanceSessionId,
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
        candidate: InterventionCandidate,
    },
    BackgroundFailed {
        session_id: LiveAssistanceSessionId,
        run_id: BackgroundRunId,
        invocation: InvocationIdentity,
    },
    ForegroundFailed {
        session_id: LiveAssistanceSessionId,
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
    },
    SourcePolicyInvalidated {
        session_id: LiveAssistanceSessionId,
        new_generation: u64,
    },
    CaptureStopped {
        session_id: LiveAssistanceSessionId,
        capture_session_id: CaptureSessionId,
    },
    ProcessingStarted {
        session_id: LiveAssistanceSessionId,
        capture_session_id: CaptureSessionId,
    },
    MeetingFinalized {
        session_id: LiveAssistanceSessionId,
        capture_session_id: CaptureSessionId,
        meeting_ref: MeetingRef,
    },
}

impl AssistanceEvent {
    fn session_id(&self) -> &LiveAssistanceSessionId {
        match self {
            Self::CaptureStarted { session_id, .. }
            | Self::EvidenceObserved { session_id, .. }
            | Self::UserMessage { session_id, .. }
            | Self::RoleCorrected { session_id, .. }
            | Self::PostureChanged { session_id, .. }
            | Self::SpeakerCorrected { session_id, .. }
            | Self::BackgroundStarted { session_id, .. }
            | Self::BackgroundCompleted { session_id, .. }
            | Self::ForegroundCompleted { session_id, .. }
            | Self::BackgroundFailed { session_id, .. }
            | Self::ForegroundFailed { session_id, .. }
            | Self::SourcePolicyInvalidated { session_id, .. }
            | Self::CaptureStopped { session_id, .. }
            | Self::ProcessingStarted { session_id, .. }
            | Self::MeetingFinalized { session_id, .. } => session_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidationReason {
    TypedUserInput,
    SourcePolicyChanged,
    PostureChanged,
    Correction,
    MeetingEnded,
    LifecycleChanged,
}

/// Reducer outputs are deliberately limited to orchestration and read-only
/// inference. There is no action capable of representing a reminder, message,
/// command, tool call, or any other external mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistanceAction {
    LiveTranscriptAttached {
        capture_session_id: CaptureSessionId,
    },
    EvidenceAccepted {
        evidence_id: EvidenceId,
        source_kind: EvidenceSourceKind,
    },
    CancelBackground {
        run_id: BackgroundRunId,
        invocation: InvocationIdentity,
        reason: InvalidationReason,
    },
    CancelForeground {
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
        reason: InvalidationReason,
    },
    AcknowledgeForeground {
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
    },
    RequestReadOnlyForegroundInference {
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
        source_event_id: EvidenceId,
        text: String,
    },
    BackgroundInvocationRegistered {
        run_id: BackgroundRunId,
        invocation: InvocationIdentity,
    },
    PublishForegroundResponse {
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
        candidate: InterventionCandidate,
    },
    PublishBackgroundInsight {
        run_id: BackgroundRunId,
        invocation: InvocationIdentity,
        candidate: InterventionCandidate,
    },
    SuppressCandidate {
        invocation: InvocationIdentity,
        reason: CandidateSuppressionReason,
    },
    RoleUpdated {
        role: UserRole,
        revision: u64,
        supersedes_revision: Option<u64>,
    },
    PostureUpdated {
        posture: AssistancePosture,
    },
    SpeakerCorrectionUpdated {
        source_label: String,
        corrected_label: String,
        revision: u64,
        supersedes_revision: Option<u64>,
    },
    PolicyBoundStateCleared {
        new_generation: u64,
    },
    MeetingEnded,
    FinalTranscriptProcessing,
    FinalizedMeetingAttached {
        meeting_ref: MeetingRef,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionReason {
    WrongAssistanceSession,
    WrongCaptureSession,
    WrongFinalizedMeeting,
    InvalidTransition,
    InvalidValue,
    NoStateChange,
    InvalidCorrection,
    DuplicateEvidence,
    TypedUserEventRequired,
    DuplicateTurn,
    BackgroundNotAllowed,
    BackgroundAlreadyRunning,
    StaleBackgroundResult,
    StaleForegroundResult,
    PolicyGenerationNotAdvanced,
    GenerationExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reduction {
    pub accepted: bool,
    pub actions: Vec<AssistanceAction>,
    pub rejection: Option<RejectionReason>,
}

impl Reduction {
    fn accepted(actions: Vec<AssistanceAction>) -> Self {
        Self {
            accepted: true,
            actions,
            rejection: None,
        }
    }

    fn rejected(reason: RejectionReason) -> Self {
        Self {
            accepted: false,
            actions: Vec::new(),
            rejection: Some(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(mode: CaptureMode) -> LiveAssistanceSession {
        let mut session = LiveAssistanceSession::new(
            "assist-1".into(),
            AssistanceSurface::NativeRecall,
            UserRole::Observer,
            AssistancePosture::Strategist,
        );
        let reduction = session.reduce(AssistanceEvent::CaptureStarted {
            session_id: "assist-1".into(),
            capture_session_id: "capture-1".into(),
            mode,
        });
        assert!(reduction.accepted);
        session
    }

    fn start_background(session: &mut LiveAssistanceSession, run_id: &str) -> InvocationIdentity {
        let reduction = session.reduce(AssistanceEvent::BackgroundStarted {
            session_id: "assist-1".into(),
            run_id: run_id.into(),
        });
        assert!(reduction.accepted);
        session.background_run.as_ref().unwrap().invocation
    }

    fn candidate() -> InterventionCandidate {
        InterventionCandidate {
            decision: InterventionDecision::Speak,
            kind: Some("insight".into()),
            text: Some("Material grounded guidance.".into()),
            evidence_ids: Vec::new(),
            visual_evidence_ids: Vec::new(),
            claims_visual_observation: false,
            confidence: 90,
        }
    }

    fn ask(
        session: &mut LiveAssistanceSession,
        turn_id: &str,
        source_event_id: &str,
        text: &str,
    ) -> (InvocationIdentity, Reduction) {
        let reduction = session.reduce(AssistanceEvent::UserMessage {
            session_id: "assist-1".into(),
            turn_id: turn_id.into(),
            source_event_id: source_event_id.into(),
            text: text.into(),
        });
        let invocation = session.foreground_turn.as_ref().unwrap().invocation;
        (invocation, reduction)
    }

    fn assert_rejected_unchanged(
        session: &mut LiveAssistanceSession,
        event: AssistanceEvent,
        reason: RejectionReason,
    ) {
        let before = session.clone();
        let reduction = session.reduce(event);
        assert!(!reduction.accepted);
        assert_eq!(reduction.rejection, Some(reason));
        assert_eq!(*session, before, "rejected events must be state-atomic");
    }

    #[test]
    fn typed_user_input_preempts_and_invalidates_background_work() {
        let mut session = session(CaptureMode::Live);
        let background_invocation = start_background(&mut session, "background-1");

        let (foreground_invocation, reduction) =
            ask(&mut session, "turn-1", "user-event-1", "What changed?");

        assert_eq!(
            reduction.actions,
            vec![
                AssistanceAction::CancelBackground {
                    run_id: "background-1".into(),
                    invocation: background_invocation,
                    reason: InvalidationReason::TypedUserInput,
                },
                AssistanceAction::AcknowledgeForeground {
                    turn_id: "turn-1".into(),
                    invocation: foreground_invocation,
                },
                AssistanceAction::RequestReadOnlyForegroundInference {
                    turn_id: "turn-1".into(),
                    invocation: foreground_invocation,
                    source_event_id: "user-event-1".into(),
                    text: "What changed?".into(),
                },
            ]
        );
        assert!(session.background_run.is_none());

        let late = session.reduce(AssistanceEvent::BackgroundCompleted {
            session_id: "assist-1".into(),
            run_id: "background-1".into(),
            invocation: background_invocation,
            candidate: candidate(),
        });
        assert_eq!(late.rejection, Some(RejectionReason::StaleBackgroundResult));
    }

    #[test]
    fn rejects_events_for_the_wrong_assistance_or_capture_session() {
        let mut session = session(CaptureMode::Live);
        let wrong_session = session.reduce(AssistanceEvent::UserMessage {
            session_id: "assist-other".into(),
            turn_id: "turn-1".into(),
            source_event_id: "user-event-1".into(),
            text: "Hello".into(),
        });
        assert_eq!(
            wrong_session.rejection,
            Some(RejectionReason::WrongAssistanceSession)
        );

        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::EvidenceObserved {
                session_id: "assist-1".into(),
                evidence: UntrustedEvidence {
                    id: "transcript-1".into(),
                    source_kind: EvidenceSourceKind::TranscriptFinal,
                    capture_session_id: Some("capture-other".into()),
                    finalized_meeting_ref: None,
                },
            },
            RejectionReason::WrongCaptureSession,
        );
        assert!(session.evidence.is_empty());
    }

    #[test]
    fn role_and_speaker_corrections_supersede_without_rewriting_raw_evidence() {
        let mut session = session(CaptureMode::Recording);
        let transcript = UntrustedEvidence {
            id: "transcript-1".into(),
            source_kind: EvidenceSourceKind::TranscriptFinal,
            capture_session_id: Some("capture-1".into()),
            finalized_meeting_ref: None,
        };
        assert!(
            session
                .reduce(AssistanceEvent::EvidenceObserved {
                    session_id: "assist-1".into(),
                    evidence: transcript.clone(),
                })
                .accepted
        );
        session.reduce(AssistanceEvent::RoleCorrected {
            session_id: "assist-1".into(),
            role: UserRole::TechnicalResponder,
            source_event_id: "role-correction".into(),
        });
        assert_eq!(session.user_role.value, UserRole::TechnicalResponder);
        assert_eq!(session.user_role.supersedes_revision, Some(0));

        session.reduce(AssistanceEvent::SpeakerCorrected {
            session_id: "assist-1".into(),
            source_label: "SPEAKER_A".into(),
            corrected_label: "ENGINEER_A".into(),
            source_event_id: "speaker-correction-1".into(),
        });
        let first_revision = session.speaker_corrections["SPEAKER_A"].revision;
        session.reduce(AssistanceEvent::SpeakerCorrected {
            session_id: "assist-1".into(),
            source_label: "SPEAKER_A".into(),
            corrected_label: "REVIEWER".into(),
            source_event_id: "speaker-correction-2".into(),
        });
        let corrected = &session.speaker_corrections["SPEAKER_A"];
        assert_eq!(corrected.corrected_label, "REVIEWER");
        assert_eq!(corrected.supersedes_revision, Some(first_revision));
        assert_eq!(session.evidence.get(&transcript.id), Some(&transcript));
    }

    #[test]
    fn policy_invalidation_cancels_work_and_clears_policy_bound_state() {
        let mut session = session(CaptureMode::Live);
        session.reduce(AssistanceEvent::EvidenceObserved {
            session_id: "assist-1".into(),
            evidence: UntrustedEvidence {
                id: "screen-1".into(),
                source_kind: EvidenceSourceKind::ScreenImage,
                capture_session_id: Some("capture-1".into()),
                finalized_meeting_ref: None,
            },
        });
        let background_invocation = start_background(&mut session, "background-1");

        let reduction = session.reduce(AssistanceEvent::SourcePolicyInvalidated {
            session_id: "assist-1".into(),
            new_generation: 2,
        });

        assert_eq!(session.source_policy_generation, 2);
        assert!(session.evidence.is_empty());
        assert!(session.background_run.is_none());
        assert_eq!(
            reduction.actions,
            vec![
                AssistanceAction::CancelBackground {
                    run_id: "background-1".into(),
                    invocation: background_invocation,
                    reason: InvalidationReason::SourcePolicyChanged,
                },
                AssistanceAction::PolicyBoundStateCleared { new_generation: 2 },
            ]
        );
    }

    #[test]
    fn live_and_recording_have_identical_normalized_assistance_semantics() {
        let mut live = LiveAssistanceSession::new(
            "assist-1".into(),
            AssistanceSurface::TerminalSidekick,
            UserRole::Participant,
            AssistancePosture::OnDemand,
        );
        let mut recording = live.clone();
        let live_actions = live
            .reduce(AssistanceEvent::CaptureStarted {
                session_id: "assist-1".into(),
                capture_session_id: "capture-1".into(),
                mode: CaptureMode::Live,
            })
            .actions;
        let recording_actions = recording
            .reduce(AssistanceEvent::CaptureStarted {
                session_id: "assist-1".into(),
                capture_session_id: "capture-1".into(),
                mode: CaptureMode::Recording,
            })
            .actions;

        assert_eq!(live_actions, recording_actions);
        assert_eq!(live.phase, recording.phase);
        assert_eq!(
            live.capture_mode.unwrap().normalized_semantics(),
            recording.capture_mode.unwrap().normalized_semantics()
        );
    }

    #[test]
    fn meeting_end_transitions_through_processing_to_finalized_artifact() {
        let mut session = session(CaptureMode::Recording);
        let ended = session.reduce(AssistanceEvent::CaptureStopped {
            session_id: "assist-1".into(),
            capture_session_id: "capture-1".into(),
        });
        assert_eq!(ended.actions, vec![AssistanceAction::MeetingEnded]);
        assert_eq!(session.phase, AssistancePhase::MeetingEnded);

        let processing = session.reduce(AssistanceEvent::ProcessingStarted {
            session_id: "assist-1".into(),
            capture_session_id: "capture-1".into(),
        });
        assert_eq!(
            processing.actions,
            vec![AssistanceAction::FinalTranscriptProcessing]
        );
        assert_eq!(session.phase, AssistancePhase::Processing);
        assert!(session.finalized_meeting_ref.is_none());

        let finalized = session.reduce(AssistanceEvent::MeetingFinalized {
            session_id: "assist-1".into(),
            capture_session_id: "capture-1".into(),
            meeting_ref: "meeting-1".into(),
        });
        assert_eq!(
            finalized.actions,
            vec![AssistanceAction::FinalizedMeetingAttached {
                meeting_ref: "meeting-1".into(),
            }]
        );
        assert_eq!(session.scope, AssistanceScope::FinalizedMeeting);
        assert_eq!(session.phase, AssistancePhase::Finalized);
    }

    #[test]
    fn untrusted_transcript_evidence_can_only_be_accepted_as_evidence() {
        let mut session = session(CaptureMode::Live);
        let reduction = session.reduce(AssistanceEvent::EvidenceObserved {
            session_id: "assist-1".into(),
            evidence: UntrustedEvidence {
                id: "transcript-with-imperative".into(),
                source_kind: EvidenceSourceKind::TranscriptFinal,
                capture_session_id: Some("capture-1".into()),
                finalized_meeting_ref: None,
            },
        });
        assert_eq!(
            reduction.actions,
            vec![AssistanceAction::EvidenceAccepted {
                evidence_id: "transcript-with-imperative".into(),
                source_kind: EvidenceSourceKind::TranscriptFinal,
            }]
        );
        assert_eq!(
            session.evidence[&EvidenceId::from("transcript-with-imperative")].capture_session_id,
            Some("capture-1".into())
        );
    }

    #[test]
    fn foreground_completion_identity_prevents_aba_publication_after_turn_id_reuse() {
        let mut session = session(CaptureMode::Live);
        let (first_invocation, _) = ask(&mut session, "turn-reused", "user-event-1", "First?");
        assert!(
            session
                .reduce(AssistanceEvent::ForegroundCompleted {
                    session_id: "assist-1".into(),
                    turn_id: "turn-reused".into(),
                    invocation: first_invocation,
                    candidate: candidate(),
                })
                .accepted
        );

        let (second_invocation, _) = ask(&mut session, "turn-reused", "user-event-2", "Second?");
        assert_ne!(first_invocation, second_invocation);
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::ForegroundCompleted {
                session_id: "assist-1".into(),
                turn_id: "turn-reused".into(),
                invocation: first_invocation,
                candidate: candidate(),
            },
            RejectionReason::StaleForegroundResult,
        );

        let current = session.reduce(AssistanceEvent::ForegroundCompleted {
            session_id: "assist-1".into(),
            turn_id: "turn-reused".into(),
            invocation: second_invocation,
            candidate: candidate(),
        });
        assert_eq!(
            current.actions,
            vec![AssistanceAction::PublishForegroundResponse {
                turn_id: "turn-reused".into(),
                invocation: second_invocation,
                candidate: candidate(),
            }]
        );
    }

    #[test]
    fn background_completion_identity_prevents_aba_publication_after_run_id_reuse() {
        let mut session = session(CaptureMode::Live);
        let first_invocation = start_background(&mut session, "run-reused");
        assert!(
            session
                .reduce(AssistanceEvent::BackgroundCompleted {
                    session_id: "assist-1".into(),
                    run_id: "run-reused".into(),
                    invocation: first_invocation,
                    candidate: candidate(),
                })
                .accepted
        );

        let second_invocation = start_background(&mut session, "run-reused");
        assert_ne!(first_invocation, second_invocation);
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::BackgroundCompleted {
                session_id: "assist-1".into(),
                run_id: "run-reused".into(),
                invocation: first_invocation,
                candidate: candidate(),
            },
            RejectionReason::StaleBackgroundResult,
        );

        assert!(
            session
                .reduce(AssistanceEvent::BackgroundCompleted {
                    session_id: "assist-1".into(),
                    run_id: "run-reused".into(),
                    invocation: second_invocation,
                    candidate: candidate(),
                })
                .accepted
        );
    }

    #[test]
    fn minutes_suppresses_silent_low_confidence_and_unsupported_candidates() {
        let mut session = session(CaptureMode::Live);
        assert!(
            session
                .reduce(AssistanceEvent::EvidenceObserved {
                    session_id: "assist-1".into(),
                    evidence: UntrustedEvidence {
                        id: "grounding".into(),
                        source_kind: EvidenceSourceKind::TranscriptFinal,
                        capture_session_id: Some("capture-1".into()),
                        finalized_meeting_ref: None,
                    },
                })
                .accepted
        );
        for (index, candidate, reason) in [
            (
                1,
                InterventionCandidate {
                    decision: InterventionDecision::Silent,
                    text: None,
                    ..candidate()
                },
                CandidateSuppressionReason::ModelChoseSilence,
            ),
            (
                2,
                InterventionCandidate {
                    confidence: 42,
                    evidence_ids: vec!["grounding".into()],
                    ..candidate()
                },
                CandidateSuppressionReason::BelowInterventionThreshold,
            ),
            (
                3,
                InterventionCandidate {
                    evidence_ids: vec!["invented-evidence".into()],
                    ..candidate()
                },
                CandidateSuppressionReason::UnsupportedProvenance,
            ),
        ] {
            let run_id = format!("run-{index}");
            let invocation = start_background(&mut session, &run_id);
            let reduced = session.reduce(AssistanceEvent::BackgroundCompleted {
                session_id: "assist-1".into(),
                run_id: run_id.into(),
                invocation,
                candidate,
            });
            assert_eq!(
                reduced.actions,
                vec![AssistanceAction::SuppressCandidate { invocation, reason }]
            );
        }
    }

    #[test]
    fn proactive_output_requires_grounding_and_visual_claims_require_a_receipt() {
        let mut session = session(CaptureMode::Recording);

        let invocation = start_background(&mut session, "ungrounded");
        let reduced = session.reduce(AssistanceEvent::BackgroundCompleted {
            session_id: "assist-1".into(),
            run_id: "ungrounded".into(),
            invocation,
            candidate: candidate(),
        });
        assert_eq!(
            reduced.actions,
            vec![AssistanceAction::SuppressCandidate {
                invocation,
                reason: CandidateSuppressionReason::UnsupportedProvenance,
            }]
        );

        let (invocation, _) = ask(
            &mut session,
            "turn-visual",
            "typed-visual",
            "What is on screen?",
        );
        let reduced = session.reduce(AssistanceEvent::ForegroundCompleted {
            session_id: "assist-1".into(),
            turn_id: "turn-visual".into(),
            invocation,
            candidate: InterventionCandidate {
                claims_visual_observation: true,
                ..candidate()
            },
        });
        assert_eq!(
            reduced.actions,
            vec![AssistanceAction::SuppressCandidate {
                invocation,
                reason: CandidateSuppressionReason::UnsupportedProvenance,
            }]
        );
    }

    #[test]
    fn lifecycle_cancels_live_work_and_rejects_disallowed_or_post_finalization_evidence() {
        let mut session = session(CaptureMode::Recording);
        let (invocation, _) = ask(&mut session, "turn-1", "user-event-1", "Still live?");
        let stopped = session.reduce(AssistanceEvent::CaptureStopped {
            session_id: "assist-1".into(),
            capture_session_id: "capture-1".into(),
        });
        assert_eq!(
            stopped.actions,
            vec![
                AssistanceAction::CancelForeground {
                    turn_id: "turn-1".into(),
                    invocation,
                    reason: InvalidationReason::MeetingEnded,
                },
                AssistanceAction::MeetingEnded,
            ]
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::ForegroundCompleted {
                session_id: "assist-1".into(),
                turn_id: "turn-1".into(),
                invocation,
                candidate: candidate(),
            },
            RejectionReason::StaleForegroundResult,
        );

        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::EvidenceObserved {
                session_id: "assist-1".into(),
                evidence: UntrustedEvidence {
                    id: "screen-after-stop".into(),
                    source_kind: EvidenceSourceKind::ScreenImage,
                    capture_session_id: Some("capture-1".into()),
                    finalized_meeting_ref: None,
                },
            },
            RejectionReason::InvalidTransition,
        );

        assert!(
            session
                .reduce(AssistanceEvent::EvidenceObserved {
                    session_id: "assist-1".into(),
                    evidence: UntrustedEvidence {
                        id: "late-final-utterance".into(),
                        source_kind: EvidenceSourceKind::TranscriptFinal,
                        capture_session_id: Some("capture-1".into()),
                        finalized_meeting_ref: None,
                    },
                })
                .accepted
        );
        assert!(
            session
                .reduce(AssistanceEvent::MeetingFinalized {
                    session_id: "assist-1".into(),
                    capture_session_id: "capture-1".into(),
                    meeting_ref: "meeting-1".into(),
                })
                .accepted
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::EvidenceObserved {
                session_id: "assist-1".into(),
                evidence: UntrustedEvidence {
                    id: "wrong-final-artifact".into(),
                    source_kind: EvidenceSourceKind::MeetingArtifact,
                    capture_session_id: None,
                    finalized_meeting_ref: Some("meeting-other".into()),
                },
            },
            RejectionReason::WrongFinalizedMeeting,
        );
        for (id, source_kind) in [
            ("final-artifact", EvidenceSourceKind::MeetingArtifact),
            (
                "final-repository-result",
                EvidenceSourceKind::RepositoryResult,
            ),
        ] {
            let evidence = UntrustedEvidence {
                id: id.into(),
                source_kind,
                capture_session_id: None,
                finalized_meeting_ref: Some("meeting-1".into()),
            };
            assert!(
                session
                    .reduce(AssistanceEvent::EvidenceObserved {
                        session_id: "assist-1".into(),
                        evidence: evidence.clone(),
                    })
                    .accepted
            );
            assert_eq!(session.evidence[&evidence.id], evidence);
        }
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::EvidenceObserved {
                session_id: "assist-1".into(),
                evidence: UntrustedEvidence {
                    id: "post-final".into(),
                    source_kind: EvidenceSourceKind::TranscriptFinal,
                    capture_session_id: Some("capture-1".into()),
                    finalized_meeting_ref: None,
                },
            },
            RejectionReason::InvalidTransition,
        );
    }

    #[test]
    fn duplicate_evidence_ids_never_overwrite_immutable_provenance() {
        let mut session = session(CaptureMode::Live);
        let original = UntrustedEvidence {
            id: "evidence-1".into(),
            source_kind: EvidenceSourceKind::TranscriptFinal,
            capture_session_id: Some("capture-1".into()),
            finalized_meeting_ref: None,
        };
        assert!(
            session
                .reduce(AssistanceEvent::EvidenceObserved {
                    session_id: "assist-1".into(),
                    evidence: original.clone(),
                })
                .accepted
        );
        for duplicate in [
            original.clone(),
            UntrustedEvidence {
                id: original.id.clone(),
                source_kind: EvidenceSourceKind::RepositoryResult,
                capture_session_id: None,
                finalized_meeting_ref: None,
            },
        ] {
            assert_rejected_unchanged(
                &mut session,
                AssistanceEvent::EvidenceObserved {
                    session_id: "assist-1".into(),
                    evidence: duplicate,
                },
                RejectionReason::DuplicateEvidence,
            );
        }
        assert_eq!(session.evidence[&original.id], original);
    }

    #[test]
    fn invalid_user_control_events_are_rejected_without_partial_state_changes() {
        let mut session = session(CaptureMode::Live);
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::BackgroundStarted {
                session_id: "assist-1".into(),
                run_id: " ".into(),
            },
            RejectionReason::InvalidValue,
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::ForegroundCompleted {
                session_id: "assist-1".into(),
                turn_id: " ".into(),
                invocation: InvocationIdentity {
                    sequence: 1,
                    source_policy_generation: 0,
                    user_generation: 0,
                },
                candidate: candidate(),
            },
            RejectionReason::InvalidValue,
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::BackgroundCompleted {
                session_id: "assist-1".into(),
                run_id: "run-1".into(),
                invocation: InvocationIdentity {
                    sequence: 0,
                    source_policy_generation: 0,
                    user_generation: 0,
                },
                candidate: candidate(),
            },
            RejectionReason::InvalidValue,
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::PostureChanged {
                session_id: "assist-1".into(),
                posture: AssistancePosture::Strategist,
            },
            RejectionReason::NoStateChange,
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::RoleCorrected {
                session_id: "assist-1".into(),
                role: UserRole::Observer,
                source_event_id: "role-noop".into(),
            },
            RejectionReason::InvalidCorrection,
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::SpeakerCorrected {
                session_id: "assist-1".into(),
                source_label: " ".into(),
                corrected_label: "REVIEWER".into(),
                source_event_id: "speaker-blank".into(),
            },
            RejectionReason::InvalidValue,
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::SpeakerCorrected {
                session_id: "assist-1".into(),
                source_label: "SPEAKER_A".into(),
                corrected_label: "SPEAKER_A".into(),
                source_event_id: "speaker-noop".into(),
            },
            RejectionReason::InvalidCorrection,
        );

        assert!(
            session
                .reduce(AssistanceEvent::SpeakerCorrected {
                    session_id: "assist-1".into(),
                    source_label: "SPEAKER_A".into(),
                    corrected_label: "REVIEWER".into(),
                    source_event_id: "speaker-valid".into(),
                })
                .accepted
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::RoleCorrected {
                session_id: "assist-1".into(),
                role: UserRole::Presenter,
                source_event_id: "speaker-valid".into(),
            },
            RejectionReason::DuplicateEvidence,
        );
    }

    #[test]
    fn invalid_values_and_counter_exhaustion_are_state_atomic() {
        let mut idle = LiveAssistanceSession::new(
            "assist-1".into(),
            AssistanceSurface::NativeRecall,
            UserRole::Observer,
            AssistancePosture::Strategist,
        );
        assert_rejected_unchanged(
            &mut idle,
            AssistanceEvent::CaptureStarted {
                session_id: "assist-1".into(),
                capture_session_id: "  ".into(),
                mode: CaptureMode::Live,
            },
            RejectionReason::InvalidValue,
        );

        let mut session = session(CaptureMode::Live);
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::UserMessage {
                session_id: "assist-1".into(),
                turn_id: "turn-1".into(),
                source_event_id: "user-event-1".into(),
                text: "  ".into(),
            },
            RejectionReason::InvalidValue,
        );

        session.next_invocation_sequence = u64::MAX;
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::BackgroundStarted {
                session_id: "assist-1".into(),
                run_id: "run-1".into(),
            },
            RejectionReason::GenerationExhausted,
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::UserMessage {
                session_id: "assist-1".into(),
                turn_id: "turn-1".into(),
                source_event_id: "user-event-1".into(),
                text: "Question".into(),
            },
            RejectionReason::GenerationExhausted,
        );

        session.next_invocation_sequence = 1;
        session.user_generation = u64::MAX;
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::PostureChanged {
                session_id: "assist-1".into(),
                posture: AssistancePosture::SilentWatch,
            },
            RejectionReason::GenerationExhausted,
        );

        session.user_generation = 0;
        session.next_correction_revision = u64::MAX;
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::RoleCorrected {
                session_id: "assist-1".into(),
                role: UserRole::Presenter,
                source_event_id: "role-event".into(),
            },
            RejectionReason::GenerationExhausted,
        );
    }

    #[test]
    fn policy_invalidation_rejections_are_atomic_and_old_foreground_cannot_publish() {
        let mut idle = LiveAssistanceSession::new(
            "assist-1".into(),
            AssistanceSurface::NativeRecall,
            UserRole::Observer,
            AssistancePosture::Strategist,
        );
        assert_rejected_unchanged(
            &mut idle,
            AssistanceEvent::SourcePolicyInvalidated {
                session_id: "assist-1".into(),
                new_generation: 1,
            },
            RejectionReason::InvalidTransition,
        );

        let mut session = session(CaptureMode::Live);
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::SourcePolicyInvalidated {
                session_id: "assist-1".into(),
                new_generation: 0,
            },
            RejectionReason::PolicyGenerationNotAdvanced,
        );
        let (invocation, _) = ask(&mut session, "turn-1", "user-event-1", "Question?");
        let invalidated = session.reduce(AssistanceEvent::SourcePolicyInvalidated {
            session_id: "assist-1".into(),
            new_generation: 1,
        });
        assert_eq!(session.source_policy_generation, 1);
        assert!(session.foreground_turn.is_none());
        assert_eq!(
            invalidated.actions.first(),
            Some(&AssistanceAction::CancelForeground {
                turn_id: "turn-1".into(),
                invocation,
                reason: InvalidationReason::SourcePolicyChanged,
            })
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::ForegroundCompleted {
                session_id: "assist-1".into(),
                turn_id: "turn-1".into(),
                invocation,
                candidate: candidate(),
            },
            RejectionReason::StaleForegroundResult,
        );
    }
}
