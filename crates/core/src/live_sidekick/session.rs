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
    pub source_policy_generation: u64,
    pub user_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundRun {
    pub id: BackgroundRunId,
    pub source_policy_generation: u64,
    pub user_generation: u64,
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
    pub evidence: BTreeMap<EvidenceId, EvidenceSourceKind>,
    pub speaker_corrections: BTreeMap<String, SpeakerCorrection>,
    next_correction_revision: u64,
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
            next_correction_revision: 1,
        }
    }

    /// Apply one already-ordered event. The returned action order is part of
    /// the contract: internal cancellation precedes the next visible user
    /// acknowledgement, and background publication is impossible after user
    /// generation changes.
    pub fn reduce(&mut self, event: AssistanceEvent) -> Reduction {
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
                self.user_generation = self.user_generation.saturating_add(1);
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
            AssistanceEvent::BackgroundCompleted { run_id, .. } => {
                self.background_completed(run_id)
            }
            AssistanceEvent::ForegroundCompleted { turn_id, .. } => {
                self.foreground_completed(turn_id)
            }
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
        self.capture_session_id = Some(capture_session_id.clone());
        self.capture_mode = Some(mode);
        self.scope = AssistanceScope::LiveCapture;
        self.phase = AssistancePhase::Ready;
        Reduction::accepted(vec![AssistanceAction::LiveTranscriptAttached {
            capture_session_id,
        }])
    }

    fn evidence_observed(&mut self, evidence: UntrustedEvidence) -> Reduction {
        if self.phase == AssistancePhase::Idle {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        let missing_required_capture = evidence.source_kind.requires_capture_session()
            && evidence.capture_session_id.is_none();
        let mismatched_capture = evidence
            .capture_session_id
            .as_ref()
            .is_some_and(|capture_id| Some(capture_id) != self.capture_session_id.as_ref());
        if missing_required_capture || mismatched_capture {
            return Reduction::rejected(RejectionReason::WrongCaptureSession);
        }
        self.evidence
            .insert(evidence.id.clone(), evidence.source_kind);
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
        if matches!(self.phase, AssistancePhase::Idle) {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        if self
            .foreground_turn
            .as_ref()
            .is_some_and(|turn| turn.id == turn_id)
        {
            return Reduction::rejected(RejectionReason::DuplicateTurn);
        }

        self.user_generation = self.user_generation.saturating_add(1);
        let mut actions = self.cancel_background(InvalidationReason::TypedUserInput);
        if let Some(previous) = self.foreground_turn.take() {
            actions.push(AssistanceAction::CancelForeground {
                turn_id: previous.id,
                reason: InvalidationReason::TypedUserInput,
            });
        }

        self.evidence
            .insert(source_event_id.clone(), EvidenceSourceKind::UserStatement);
        self.foreground_turn = Some(ForegroundTurn {
            id: turn_id.clone(),
            source_event_id: source_event_id.clone(),
            source_policy_generation: self.source_policy_generation,
            user_generation: self.user_generation,
        });
        actions.push(AssistanceAction::AcknowledgeForeground {
            turn_id: turn_id.clone(),
        });
        actions.push(AssistanceAction::RequestReadOnlyForegroundInference {
            turn_id,
            source_event_id,
            text,
        });
        Reduction::accepted(actions)
    }

    fn role_corrected(&mut self, role: UserRole, source_event_id: EvidenceId) -> Reduction {
        self.user_generation = self.user_generation.saturating_add(1);
        let mut actions = self.cancel_in_flight(InvalidationReason::Correction);
        let revision = self.take_correction_revision();
        let supersedes_revision = Some(self.user_role.revision);
        self.evidence
            .insert(source_event_id.clone(), EvidenceSourceKind::UserStatement);
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
        self.user_generation = self.user_generation.saturating_add(1);
        let mut actions = self.cancel_in_flight(InvalidationReason::Correction);
        let revision = self.take_correction_revision();
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
        self.evidence
            .insert(source_event_id, EvidenceSourceKind::UserStatement);
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
        self.background_run = Some(BackgroundRun {
            id: run_id,
            source_policy_generation: self.source_policy_generation,
            user_generation: self.user_generation,
        });
        Reduction::accepted(Vec::new())
    }

    fn background_completed(&mut self, run_id: BackgroundRunId) -> Reduction {
        let Some(run) = self.background_run.as_ref() else {
            return Reduction::rejected(RejectionReason::StaleBackgroundResult);
        };
        if run.id != run_id
            || run.user_generation != self.user_generation
            || run.source_policy_generation != self.source_policy_generation
            || self.foreground_turn.is_some()
        {
            return Reduction::rejected(RejectionReason::StaleBackgroundResult);
        }
        self.background_run = None;
        Reduction::accepted(vec![AssistanceAction::PublishBackgroundInsight { run_id }])
    }

    fn foreground_completed(&mut self, turn_id: ForegroundTurnId) -> Reduction {
        let Some(turn) = self.foreground_turn.as_ref() else {
            return Reduction::rejected(RejectionReason::StaleForegroundResult);
        };
        if turn.id != turn_id
            || turn.source_policy_generation != self.source_policy_generation
            || turn.user_generation != self.user_generation
        {
            return Reduction::rejected(RejectionReason::StaleForegroundResult);
        }
        self.foreground_turn = None;
        Reduction::accepted(vec![AssistanceAction::PublishForegroundResponse {
            turn_id,
        }])
    }

    fn policy_invalidated(&mut self, new_generation: u64) -> Reduction {
        if new_generation <= self.source_policy_generation {
            return Reduction::rejected(RejectionReason::PolicyGenerationNotAdvanced);
        }
        let mut actions = self.cancel_background(InvalidationReason::SourcePolicyChanged);
        if let Some(turn) = self.foreground_turn.take() {
            actions.push(AssistanceAction::CancelForeground {
                turn_id: turn.id,
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
        if !self.matches_capture(&capture_session_id) {
            return Reduction::rejected(RejectionReason::WrongCaptureSession);
        }
        if self.phase != AssistancePhase::Ready {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        let mut actions = self.cancel_background(InvalidationReason::MeetingEnded);
        self.phase = AssistancePhase::MeetingEnded;
        actions.push(AssistanceAction::MeetingEnded);
        Reduction::accepted(actions)
    }

    fn processing_started(&mut self, capture_session_id: CaptureSessionId) -> Reduction {
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
        if !self.matches_capture(&capture_session_id) {
            return Reduction::rejected(RejectionReason::WrongCaptureSession);
        }
        if !matches!(
            self.phase,
            AssistancePhase::MeetingEnded | AssistancePhase::Processing
        ) {
            return Reduction::rejected(RejectionReason::InvalidTransition);
        }
        self.scope = AssistanceScope::FinalizedMeeting;
        self.phase = AssistancePhase::Finalized;
        self.finalized_meeting_ref = Some(meeting_ref.clone());
        Reduction::accepted(vec![AssistanceAction::FinalizedMeetingAttached {
            meeting_ref,
        }])
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
                reason,
            });
        }
        actions
    }

    fn take_correction_revision(&mut self) -> u64 {
        let revision = self.next_correction_revision;
        self.next_correction_revision = self.next_correction_revision.saturating_add(1);
        revision
    }
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
    },
    ForegroundCompleted {
        session_id: LiveAssistanceSessionId,
        turn_id: ForegroundTurnId,
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
        reason: InvalidationReason,
    },
    CancelForeground {
        turn_id: ForegroundTurnId,
        reason: InvalidationReason,
    },
    AcknowledgeForeground {
        turn_id: ForegroundTurnId,
    },
    RequestReadOnlyForegroundInference {
        turn_id: ForegroundTurnId,
        source_event_id: EvidenceId,
        text: String,
    },
    PublishForegroundResponse {
        turn_id: ForegroundTurnId,
    },
    PublishBackgroundInsight {
        run_id: BackgroundRunId,
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
    InvalidTransition,
    DuplicateTurn,
    BackgroundNotAllowed,
    BackgroundAlreadyRunning,
    StaleBackgroundResult,
    StaleForegroundResult,
    PolicyGenerationNotAdvanced,
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

    #[test]
    fn typed_user_input_preempts_and_invalidates_background_work() {
        let mut session = session(CaptureMode::Live);
        assert!(
            session
                .reduce(AssistanceEvent::BackgroundStarted {
                    session_id: "assist-1".into(),
                    run_id: "background-1".into(),
                })
                .accepted
        );

        let reduction = session.reduce(AssistanceEvent::UserMessage {
            session_id: "assist-1".into(),
            turn_id: "turn-1".into(),
            source_event_id: "user-event-1".into(),
            text: "What changed?".into(),
        });

        assert_eq!(
            reduction.actions,
            vec![
                AssistanceAction::CancelBackground {
                    run_id: "background-1".into(),
                    reason: InvalidationReason::TypedUserInput,
                },
                AssistanceAction::AcknowledgeForeground {
                    turn_id: "turn-1".into(),
                },
                AssistanceAction::RequestReadOnlyForegroundInference {
                    turn_id: "turn-1".into(),
                    source_event_id: "user-event-1".into(),
                    text: "What changed?".into(),
                },
            ]
        );
        assert!(session.background_run.is_none());

        let late = session.reduce(AssistanceEvent::BackgroundCompleted {
            session_id: "assist-1".into(),
            run_id: "background-1".into(),
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

        let wrong_capture = session.reduce(AssistanceEvent::EvidenceObserved {
            session_id: "assist-1".into(),
            evidence: UntrustedEvidence {
                id: "transcript-1".into(),
                source_kind: EvidenceSourceKind::TranscriptFinal,
                capture_session_id: Some("capture-other".into()),
            },
        });
        assert_eq!(
            wrong_capture.rejection,
            Some(RejectionReason::WrongCaptureSession)
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
        assert_eq!(
            session.evidence.get(&transcript.id),
            Some(&EvidenceSourceKind::TranscriptFinal)
        );
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
            },
        });
        session.reduce(AssistanceEvent::BackgroundStarted {
            session_id: "assist-1".into(),
            run_id: "background-1".into(),
        });

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
            },
        });
        assert_eq!(
            reduction.actions,
            vec![AssistanceAction::EvidenceAccepted {
                evidence_id: "transcript-with-imperative".into(),
                source_kind: EvidenceSourceKind::TranscriptFinal,
            }]
        );
    }
}
