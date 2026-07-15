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

            fn is_valid(&self) -> bool {
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
string_id!(ProviderBindingId);
string_id!(ProviderAttestationId);

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

/// A provider isolation profile whose capability facts are defined by Minutes.
///
/// Adapters cannot assemble a trusted capability proof from independent
/// booleans. They must identify the verified profile and the attestation that
/// established it. The reducer still validates binding identity and monotonic
/// generation before granting Native Recall authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderIsolationProfile {
    VerifiedLoopbackText,
    AgentControlledText,
    AgentControlledExactSessionScreen,
    Unavailable,
}

/// Process-local proof that one provider invocation route matches a known
/// isolation profile. The binding and attestation identifiers are opaque: the
/// host owns their verification, while the reducer owns freshness and scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderBinding {
    binding_id: ProviderBindingId,
    generation: u64,
    attestation_id: ProviderAttestationId,
    profile: ProviderIsolationProfile,
}

impl ProviderBinding {
    pub fn new(
        binding_id: ProviderBindingId,
        generation: u64,
        attestation_id: ProviderAttestationId,
        profile: ProviderIsolationProfile,
    ) -> Option<Self> {
        if !binding_id.is_valid() || generation == 0 || !attestation_id.is_valid() {
            return None;
        }
        Some(Self {
            binding_id,
            generation,
            attestation_id,
            profile,
        })
    }

    pub fn binding_id(&self) -> &ProviderBindingId {
        &self.binding_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn attestation_id(&self) -> &ProviderAttestationId {
        &self.attestation_id
    }

    pub fn profile(&self) -> ProviderIsolationProfile {
        self.profile
    }

    pub fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::for_profile(self.profile)
    }
}

/// Capability facts derived from a verified provider isolation profile.
///
/// Fields intentionally remain private so callers cannot manufacture a trusted
/// bag of booleans. Native Recall may start inference only from a fresh
/// `ProviderBinding` whose derived facts satisfy this contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderCapabilities {
    cancellation: bool,
    bounded_input: bool,
    bounded_output: bool,
    arbitrary_writes_denied: bool,
    arbitrary_shell_denied: bool,
    ambient_filesystem_reads_denied: bool,
    unapproved_tools_denied: bool,
    routing_disclosure: RoutingDisclosure,
    screen_disclosure: ScreenDisclosureCapability,
}

impl ProviderCapabilities {
    fn for_profile(profile: ProviderIsolationProfile) -> Self {
        match profile {
            ProviderIsolationProfile::VerifiedLoopbackText => Self {
                cancellation: true,
                bounded_input: true,
                bounded_output: true,
                arbitrary_writes_denied: true,
                arbitrary_shell_denied: true,
                ambient_filesystem_reads_denied: true,
                unapproved_tools_denied: true,
                routing_disclosure: RoutingDisclosure::VerifiedLoopback,
                screen_disclosure: ScreenDisclosureCapability::Unavailable,
            },
            ProviderIsolationProfile::AgentControlledText => Self {
                cancellation: true,
                bounded_input: true,
                bounded_output: true,
                arbitrary_writes_denied: true,
                arbitrary_shell_denied: true,
                ambient_filesystem_reads_denied: true,
                unapproved_tools_denied: true,
                routing_disclosure: RoutingDisclosure::AgentControlled,
                screen_disclosure: ScreenDisclosureCapability::Unavailable,
            },
            ProviderIsolationProfile::AgentControlledExactSessionScreen => Self {
                cancellation: true,
                bounded_input: true,
                bounded_output: true,
                arbitrary_writes_denied: true,
                arbitrary_shell_denied: true,
                ambient_filesystem_reads_denied: true,
                unapproved_tools_denied: true,
                routing_disclosure: RoutingDisclosure::AgentControlled,
                screen_disclosure: ScreenDisclosureCapability::ExactSessionExplicit,
            },
            ProviderIsolationProfile::Unavailable => Self::default(),
        }
    }

    pub fn supports_native_recall(self) -> bool {
        self.cancellation
            && self.bounded_input
            && self.bounded_output
            && self.arbitrary_writes_denied
            && self.arbitrary_shell_denied
            && self.ambient_filesystem_reads_denied
            && self.unapproved_tools_denied
            && self.routing_disclosure != RoutingDisclosure::Unavailable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoutingDisclosure {
    VerifiedLoopback,
    AgentControlled,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenDisclosureCapability {
    ExactSessionExplicit,
    #[default]
    Unavailable,
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
    fn is_valid(self) -> bool {
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
    /// Process-local provider proof. It is deliberately never restored from a
    /// serialized session; every process must re-attest before Native Recall
    /// can invoke a provider.
    #[serde(skip, default)]
    pub provider_binding: Option<ProviderBinding>,
    /// Monotonic freshness guard for provider events in this process. Like the
    /// proof itself, it resets on process restore because prior queued events
    /// cannot cross a process boundary.
    #[serde(skip, default)]
    provider_binding_generation: u64,
    /// A source-policy counter overflow permanently removes inference authority
    /// from this session instead of preserving the previous proof.
    #[serde(default)]
    pub authority_exhausted: bool,
    /// In-flight work is process-local authority. A serialized session may be
    /// useful for diagnostics, but restoring it must never make an old provider
    /// callback publishable in a new process.
    #[serde(skip, default)]
    pub foreground_turn: Option<ForegroundTurn>,
    #[serde(skip, default)]
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
            provider_binding: None,
            provider_binding_generation: 0,
            authority_exhausted: false,
            foreground_turn: None,
            background_run: None,
            evidence: BTreeMap::new(),
            speaker_corrections: BTreeMap::new(),
            next_invocation_sequence: default_next_invocation_sequence(),
            next_correction_revision: default_next_correction_revision(),
        }
    }

    /// Construct an on-demand session for an already-finalized meeting.
    /// Historical Recall has no capture lifecycle to replay, so this explicit
    /// constructor binds the exact meeting reference without inventing a fake
    /// capture session.
    pub fn new_finalized(
        id: LiveAssistanceSessionId,
        surface: AssistanceSurface,
        user_role: UserRole,
        posture: AssistancePosture,
        meeting_ref: MeetingRef,
    ) -> Result<Self, RejectionReason> {
        if !id.is_valid() || !meeting_ref.is_valid() {
            return Err(RejectionReason::InvalidValue);
        }
        let mut session = Self::new(id, surface, user_role, posture);
        session.scope = AssistanceScope::FinalizedMeeting;
        session.phase = AssistancePhase::Finalized;
        session.finalized_meeting_ref = Some(meeting_ref);
        Ok(session)
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
                run_id, invocation, ..
            } => self.background_completed(run_id, invocation),
            AssistanceEvent::ForegroundCompleted {
                turn_id,
                invocation,
                ..
            } => self.foreground_completed(turn_id, invocation),
            AssistanceEvent::ForegroundCancelled {
                turn_id,
                invocation,
                ..
            } => self.foreground_cancelled(turn_id, invocation),
            AssistanceEvent::ForegroundFailed {
                turn_id,
                invocation,
                ..
            } => self.foreground_failed(turn_id, invocation),
            AssistanceEvent::ProviderBindingChanged { binding, .. } => {
                self.provider_binding_changed(binding)
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
        if self.surface == AssistanceSurface::NativeRecall && !self.native_recall_provider_ready() {
            return Reduction::rejected(RejectionReason::ProviderCapabilitiesInsufficient);
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
        if self.surface == AssistanceSurface::NativeRecall && !self.native_recall_provider_ready() {
            return Reduction::rejected(RejectionReason::ProviderCapabilitiesInsufficient);
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
        Reduction::accepted(vec![AssistanceAction::PublishBackgroundInsight {
            run_id,
            invocation,
        }])
    }

    fn foreground_completed(
        &mut self,
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
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
        Reduction::accepted(vec![AssistanceAction::PublishForegroundResponse {
            turn_id,
            invocation,
        }])
    }

    fn foreground_cancelled(
        &mut self,
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
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
        Reduction::accepted(vec![AssistanceAction::CancelForeground {
            turn_id,
            invocation,
            reason: InvalidationReason::UserCancelled,
        }])
    }

    fn foreground_failed(
        &mut self,
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
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
        Reduction::accepted(vec![AssistanceAction::CancelForeground {
            turn_id,
            invocation,
            reason: InvalidationReason::ProviderFailed,
        }])
    }

    fn provider_binding_changed(&mut self, binding: ProviderBinding) -> Reduction {
        if self.surface != AssistanceSurface::NativeRecall {
            return Reduction::rejected(RejectionReason::ProviderBindingNotApplicable);
        }
        if self.authority_exhausted {
            return Reduction::rejected(RejectionReason::GenerationExhausted);
        }
        if self.provider_binding_generation == u64::MAX {
            return self.exhaust_provider_authority(self.provider_binding_generation);
        }
        if binding.generation() <= self.provider_binding_generation {
            return Reduction::rejected(RejectionReason::ProviderBindingGenerationNotAdvanced);
        }
        // Reserve the terminal value so a proven binding can never make every
        // later revocation numerically stale.
        if binding.generation() == u64::MAX {
            return self.exhaust_provider_authority(binding.generation());
        }
        let pristine_initial_binding = self.provider_binding.is_none()
            && self.phase == AssistancePhase::Idle
            && self.evidence.is_empty()
            && self.foreground_turn.is_none()
            && self.background_run.is_none();
        let new_generation = if pristine_initial_binding {
            self.source_policy_generation
        } else {
            let Some(generation) = self.source_policy_generation.checked_add(1) else {
                return self.exhaust_provider_authority(binding.generation());
            };
            generation
        };
        let capabilities = binding.capabilities();
        let mut actions = self.cancel_in_flight(InvalidationReason::ProviderCapabilitiesChanged);
        self.source_policy_generation = new_generation;
        self.provider_binding_generation = binding.generation();
        self.provider_binding = Some(binding.clone());
        self.evidence.clear();
        self.speaker_corrections.clear();
        self.user_role.source_event_id = None;
        if !pristine_initial_binding {
            actions.push(AssistanceAction::PolicyBoundStateCleared { new_generation });
        }
        actions.push(AssistanceAction::ProviderBindingUpdated {
            binding: Some(binding),
            capabilities,
            native_recall_ready: capabilities.supports_native_recall(),
            new_generation,
            authority_exhausted: false,
        });
        Reduction::accepted(actions)
    }

    fn exhaust_provider_authority(&mut self, incoming_binding_generation: u64) -> Reduction {
        let mut actions = self.cancel_in_flight(InvalidationReason::ProviderCapabilitiesChanged);
        self.provider_binding_generation = incoming_binding_generation;
        self.provider_binding = None;
        self.authority_exhausted = true;
        self.evidence.clear();
        self.speaker_corrections.clear();
        self.user_role.source_event_id = None;
        actions.push(AssistanceAction::PolicyBoundStateCleared {
            new_generation: self.source_policy_generation,
        });
        actions.push(AssistanceAction::ProviderBindingUpdated {
            binding: None,
            capabilities: ProviderCapabilities::default(),
            native_recall_ready: false,
            new_generation: self.source_policy_generation,
            authority_exhausted: true,
        });
        Reduction::accepted(actions)
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
        let Some(new_generation) = self.source_policy_generation.checked_add(1) else {
            return Reduction::rejected(RejectionReason::GenerationExhausted);
        };
        let mut actions = self.cancel_in_flight(InvalidationReason::LifecycleChanged);
        self.scope = AssistanceScope::FinalizedMeeting;
        self.phase = AssistancePhase::Finalized;
        self.finalized_meeting_ref = Some(meeting_ref.clone());
        self.source_policy_generation = new_generation;
        self.evidence.clear();
        self.speaker_corrections.clear();
        self.user_role.source_event_id = None;
        actions.push(AssistanceAction::PolicyBoundStateCleared { new_generation });
        actions.push(AssistanceAction::FinalizedMeetingAttached {
            prior_capture_session_id: capture_session_id,
            meeting_ref,
            new_generation,
        });
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

    pub fn native_recall_provider_ready(&self) -> bool {
        !self.authority_exhausted
            && self
                .provider_binding
                .as_ref()
                .is_some_and(|binding| binding.capabilities().supports_native_recall())
    }

    pub fn provider_binding_generation(&self) -> u64 {
        self.provider_binding_generation
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
    },
    ForegroundCompleted {
        session_id: LiveAssistanceSessionId,
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
    },
    ForegroundCancelled {
        session_id: LiveAssistanceSessionId,
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
    },
    ForegroundFailed {
        session_id: LiveAssistanceSessionId,
        turn_id: ForegroundTurnId,
        invocation: InvocationIdentity,
    },
    ProviderBindingChanged {
        session_id: LiveAssistanceSessionId,
        binding: ProviderBinding,
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
            | Self::ForegroundCancelled { session_id, .. }
            | Self::ForegroundFailed { session_id, .. }
            | Self::ProviderBindingChanged { session_id, .. }
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
    UserCancelled,
    ProviderFailed,
    SourcePolicyChanged,
    ProviderCapabilitiesChanged,
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
    },
    PublishBackgroundInsight {
        run_id: BackgroundRunId,
        invocation: InvocationIdentity,
    },
    ProviderBindingUpdated {
        binding: Option<ProviderBinding>,
        capabilities: ProviderCapabilities,
        native_recall_ready: bool,
        new_generation: u64,
        authority_exhausted: bool,
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
        prior_capture_session_id: CaptureSessionId,
        meeting_ref: MeetingRef,
        new_generation: u64,
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
    ProviderCapabilitiesInsufficient,
    ProviderBindingNotApplicable,
    ProviderBindingGenerationNotAdvanced,
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

    fn provider_binding(
        binding_id: &str,
        generation: u64,
        profile: ProviderIsolationProfile,
    ) -> ProviderBinding {
        ProviderBinding::new(
            binding_id.into(),
            generation,
            format!("attestation-{generation}").into(),
            profile,
        )
        .unwrap()
    }

    fn proven_native_recall_provider(generation: u64) -> ProviderBinding {
        provider_binding(
            "agent-read-only",
            generation,
            ProviderIsolationProfile::AgentControlledText,
        )
    }

    fn session(mode: CaptureMode) -> LiveAssistanceSession {
        let mut session = LiveAssistanceSession::new(
            "assist-1".into(),
            AssistanceSurface::NativeRecall,
            UserRole::Observer,
            AssistancePosture::Strategist,
        );
        let capabilities = session.reduce(AssistanceEvent::ProviderBindingChanged {
            session_id: "assist-1".into(),
            binding: proven_native_recall_provider(1),
        });
        assert!(capabilities.accepted);
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
            vec![
                AssistanceAction::PolicyBoundStateCleared { new_generation: 1 },
                AssistanceAction::FinalizedMeetingAttached {
                    prior_capture_session_id: "capture-1".into(),
                    meeting_ref: "meeting-1".into(),
                    new_generation: 1,
                },
            ]
        );
        assert_eq!(session.scope, AssistanceScope::FinalizedMeeting);
        assert_eq!(session.phase, AssistancePhase::Finalized);
        assert_eq!(session.source_policy_generation, 1);
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
            },
            RejectionReason::StaleForegroundResult,
        );

        let current = session.reduce(AssistanceEvent::ForegroundCompleted {
            session_id: "assist-1".into(),
            turn_id: "turn-reused".into(),
            invocation: second_invocation,
        });
        assert_eq!(
            current.actions,
            vec![AssistanceAction::PublishForegroundResponse {
                turn_id: "turn-reused".into(),
                invocation: second_invocation,
            }]
        );
    }

    #[test]
    fn explicit_foreground_cancel_requires_the_current_invocation_identity() {
        let mut session = session(CaptureMode::Live);
        let (invocation, _) = ask(&mut session, "turn-1", "user-1", "Question?");
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::ForegroundCancelled {
                session_id: "assist-1".into(),
                turn_id: "turn-1".into(),
                invocation: InvocationIdentity {
                    sequence: invocation.sequence + 1,
                    ..invocation
                },
            },
            RejectionReason::StaleForegroundResult,
        );
        let cancelled = session.reduce(AssistanceEvent::ForegroundCancelled {
            session_id: "assist-1".into(),
            turn_id: "turn-1".into(),
            invocation,
        });
        assert_eq!(
            cancelled.actions,
            vec![AssistanceAction::CancelForeground {
                turn_id: "turn-1".into(),
                invocation,
                reason: InvalidationReason::UserCancelled,
            }]
        );
        assert!(session.foreground_turn.is_none());
    }

    #[test]
    fn provider_failure_retires_the_exact_foreground_invocation() {
        let mut session = session(CaptureMode::Live);
        let (invocation, _) = ask(&mut session, "turn-1", "user-1", "Question?");
        let failed = session.reduce(AssistanceEvent::ForegroundFailed {
            session_id: "assist-1".into(),
            turn_id: "turn-1".into(),
            invocation,
        });
        assert_eq!(
            failed.actions,
            vec![AssistanceAction::CancelForeground {
                turn_id: "turn-1".into(),
                invocation,
                reason: InvalidationReason::ProviderFailed,
            }]
        );
        assert!(session.foreground_turn.is_none());
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
            },
            RejectionReason::StaleBackgroundResult,
        );

        assert!(
            session
                .reduce(AssistanceEvent::BackgroundCompleted {
                    session_id: "assist-1".into(),
                    run_id: "run-reused".into(),
                    invocation: second_invocation,
                })
                .accepted
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
            },
            RejectionReason::StaleForegroundResult,
        );
    }

    #[test]
    fn native_recall_fails_closed_until_a_fresh_provider_binding_is_proven() {
        let mut session = LiveAssistanceSession::new(
            "assist-1".into(),
            AssistanceSurface::NativeRecall,
            UserRole::Observer,
            AssistancePosture::OnDemand,
        );
        assert!(
            session
                .reduce(AssistanceEvent::CaptureStarted {
                    session_id: "assist-1".into(),
                    capture_session_id: "capture-1".into(),
                    mode: CaptureMode::Live,
                })
                .accepted
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::UserMessage {
                session_id: "assist-1".into(),
                turn_id: "turn-unproven".into(),
                source_event_id: "user-unproven".into(),
                text: "What changed?".into(),
            },
            RejectionReason::ProviderCapabilitiesInsufficient,
        );

        let unavailable = provider_binding(
            "route-unavailable",
            1,
            ProviderIsolationProfile::Unavailable,
        );
        let bound = session.reduce(AssistanceEvent::ProviderBindingChanged {
            session_id: "assist-1".into(),
            binding: unavailable.clone(),
        });
        assert_eq!(session.source_policy_generation, 1);
        assert_eq!(
            bound.actions,
            vec![
                AssistanceAction::PolicyBoundStateCleared { new_generation: 1 },
                AssistanceAction::ProviderBindingUpdated {
                    binding: Some(unavailable),
                    capabilities: ProviderCapabilities::default(),
                    native_recall_ready: false,
                    new_generation: 1,
                    authority_exhausted: false,
                },
            ]
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::UserMessage {
                session_id: "assist-1".into(),
                turn_id: "turn-incomplete".into(),
                source_event_id: "user-incomplete".into(),
                text: "What changed?".into(),
            },
            RejectionReason::ProviderCapabilitiesInsufficient,
        );

        let proven = proven_native_recall_provider(2);
        let capabilities = proven.capabilities();
        let rebound = session.reduce(AssistanceEvent::ProviderBindingChanged {
            session_id: "assist-1".into(),
            binding: proven.clone(),
        });
        assert_eq!(session.source_policy_generation, 2);
        assert_eq!(
            rebound.actions.last(),
            Some(&AssistanceAction::ProviderBindingUpdated {
                binding: Some(proven),
                capabilities,
                native_recall_ready: true,
                new_generation: 2,
                authority_exhausted: false,
            })
        );
        assert!(
            session
                .reduce(AssistanceEvent::UserMessage {
                    session_id: "assist-1".into(),
                    turn_id: "turn-proven".into(),
                    source_event_id: "user-proven".into(),
                    text: "What changed?".into(),
                })
                .accepted
        );
    }

    #[test]
    fn provider_binding_change_cancels_then_clears_then_publishes_capabilities() {
        let mut session = session(CaptureMode::Recording);
        let (invocation, _) = ask(
            &mut session,
            "turn-1",
            "user-event-1",
            "Summarize the decision.",
        );
        assert!(!session.evidence.is_empty());

        let unavailable = provider_binding(
            "route-unavailable",
            2,
            ProviderIsolationProfile::Unavailable,
        );
        let reduction = session.reduce(AssistanceEvent::ProviderBindingChanged {
            session_id: "assist-1".into(),
            binding: unavailable.clone(),
        });
        assert_eq!(session.source_policy_generation, 1);
        assert!(session.foreground_turn.is_none());
        assert!(session.evidence.is_empty());
        assert_eq!(
            reduction.actions,
            vec![
                AssistanceAction::CancelForeground {
                    turn_id: "turn-1".into(),
                    invocation,
                    reason: InvalidationReason::ProviderCapabilitiesChanged,
                },
                AssistanceAction::PolicyBoundStateCleared { new_generation: 1 },
                AssistanceAction::ProviderBindingUpdated {
                    binding: Some(unavailable),
                    capabilities: ProviderCapabilities::default(),
                    native_recall_ready: false,
                    new_generation: 1,
                    authority_exhausted: false,
                },
            ]
        );
    }

    #[test]
    fn provider_binding_identity_and_generation_prevent_aba_reenable() {
        let mut session = session(CaptureMode::Live);
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::ProviderBindingChanged {
                session_id: "assist-1".into(),
                binding: provider_binding(
                    "delayed-route",
                    1,
                    ProviderIsolationProfile::AgentControlledText,
                ),
            },
            RejectionReason::ProviderBindingGenerationNotAdvanced,
        );

        let replacement = provider_binding(
            "replacement-route",
            2,
            ProviderIsolationProfile::AgentControlledText,
        );
        let changed = session.reduce(AssistanceEvent::ProviderBindingChanged {
            session_id: "assist-1".into(),
            binding: replacement.clone(),
        });
        assert!(changed.accepted);
        assert_eq!(session.provider_binding.as_ref(), Some(&replacement));
        assert_eq!(session.source_policy_generation, 1);

        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::ProviderBindingChanged {
                session_id: "assist-1".into(),
                binding: proven_native_recall_provider(1),
            },
            RejectionReason::ProviderBindingGenerationNotAdvanced,
        );
    }

    #[test]
    fn newer_revocation_with_a_generation_gap_fails_closed() {
        let mut session = session(CaptureMode::Live);
        let unavailable = provider_binding(
            "coalesced-revocation",
            3,
            ProviderIsolationProfile::Unavailable,
        );
        let reduction = session.reduce(AssistanceEvent::ProviderBindingChanged {
            session_id: "assist-1".into(),
            binding: unavailable.clone(),
        });
        assert!(reduction.accepted);
        assert!(!session.native_recall_provider_ready());
        assert_eq!(session.provider_binding.as_ref(), Some(&unavailable));
        assert_eq!(session.provider_binding_generation(), 3);
    }

    #[test]
    fn terminal_provider_binding_generation_cannot_preserve_or_restore_authority() {
        let mut session = session(CaptureMode::Live);
        let (invocation, _) = ask(&mut session, "turn-1", "user-event-1", "Question?");
        session.provider_binding_generation = u64::MAX - 1;
        let reduction = session.reduce(AssistanceEvent::ProviderBindingChanged {
            session_id: "assist-1".into(),
            binding: provider_binding(
                "terminal-generation-route",
                u64::MAX,
                ProviderIsolationProfile::AgentControlledText,
            ),
        });
        assert!(reduction.accepted);
        assert!(session.authority_exhausted);
        assert!(session.provider_binding.is_none());
        assert!(!session.native_recall_provider_ready());
        assert_eq!(
            reduction.actions.first(),
            Some(&AssistanceAction::CancelForeground {
                turn_id: "turn-1".into(),
                invocation,
                reason: InvalidationReason::ProviderCapabilitiesChanged,
            })
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::ProviderBindingChanged {
                session_id: "assist-1".into(),
                binding: provider_binding(
                    "delayed-proven-route",
                    2,
                    ProviderIsolationProfile::AgentControlledText,
                ),
            },
            RejectionReason::GenerationExhausted,
        );
    }

    #[test]
    fn provider_bindings_are_native_only_and_do_not_mutate_other_surfaces() {
        for surface in [
            AssistanceSurface::TerminalSidekick,
            AssistanceSurface::CoachHud,
        ] {
            let mut session = LiveAssistanceSession::new(
                "assist-1".into(),
                surface,
                UserRole::Observer,
                AssistancePosture::OnDemand,
            );
            assert_rejected_unchanged(
                &mut session,
                AssistanceEvent::ProviderBindingChanged {
                    session_id: "assist-1".into(),
                    binding: proven_native_recall_provider(1),
                },
                RejectionReason::ProviderBindingNotApplicable,
            );
        }
    }

    #[test]
    fn provider_revocation_at_policy_generation_exhaustion_fails_closed() {
        let mut session = session(CaptureMode::Live);
        let (invocation, _) = ask(&mut session, "turn-1", "user-event-1", "Question?");
        session.source_policy_generation = u64::MAX;
        let unavailable = provider_binding(
            "route-unavailable",
            2,
            ProviderIsolationProfile::Unavailable,
        );
        let reduction = session.reduce(AssistanceEvent::ProviderBindingChanged {
            session_id: "assist-1".into(),
            binding: unavailable,
        });
        assert!(reduction.accepted);
        assert!(session.authority_exhausted);
        assert!(session.provider_binding.is_none());
        assert!(!session.native_recall_provider_ready());
        assert_eq!(
            reduction.actions,
            vec![
                AssistanceAction::CancelForeground {
                    turn_id: "turn-1".into(),
                    invocation,
                    reason: InvalidationReason::ProviderCapabilitiesChanged,
                },
                AssistanceAction::PolicyBoundStateCleared {
                    new_generation: u64::MAX,
                },
                AssistanceAction::ProviderBindingUpdated {
                    binding: None,
                    capabilities: ProviderCapabilities::default(),
                    native_recall_ready: false,
                    new_generation: u64::MAX,
                    authority_exhausted: true,
                },
            ]
        );
        assert_rejected_unchanged(
            &mut session,
            AssistanceEvent::UserMessage {
                session_id: "assist-1".into(),
                turn_id: "turn-2".into(),
                source_event_id: "user-event-2".into(),
                text: "Question?".into(),
            },
            RejectionReason::ProviderCapabilitiesInsufficient,
        );
    }

    #[test]
    fn finalized_sessions_bind_an_exact_meeting_without_fake_capture_state() {
        assert_eq!(
            LiveAssistanceSession::new_finalized(
                " ".into(),
                AssistanceSurface::NativeRecall,
                UserRole::Observer,
                AssistancePosture::OnDemand,
                "meeting-a".into(),
            ),
            Err(RejectionReason::InvalidValue)
        );

        let mut session = LiveAssistanceSession::new_finalized(
            "assist-final".into(),
            AssistanceSurface::NativeRecall,
            UserRole::DecisionMaker,
            AssistancePosture::OnDemand,
            "meeting-a".into(),
        )
        .unwrap();
        assert_eq!(session.scope, AssistanceScope::FinalizedMeeting);
        assert_eq!(session.phase, AssistancePhase::Finalized);
        assert!(session.capture_session_id.is_none());
        assert_eq!(
            session.finalized_meeting_ref.as_ref().unwrap().as_str(),
            "meeting-a"
        );

        let capabilities = session.reduce(AssistanceEvent::ProviderBindingChanged {
            session_id: "assist-final".into(),
            binding: proven_native_recall_provider(1),
        });
        assert!(capabilities.accepted);
        let question = session.reduce(AssistanceEvent::UserMessage {
            session_id: "assist-final".into(),
            turn_id: "turn-final".into(),
            source_event_id: "user-final".into(),
            text: "What was decided?".into(),
        });
        assert!(question.accepted);
        assert_eq!(
            session
                .evidence
                .get(&EvidenceId::new("user-final"))
                .and_then(|evidence| evidence.finalized_meeting_ref.as_ref())
                .map(MeetingRef::as_str),
            Some("meeting-a")
        );
    }

    #[test]
    fn serialized_sessions_require_fresh_provider_reattestation() {
        let mut session = session(CaptureMode::Live);
        assert!(session.native_recall_provider_ready());
        let (old_invocation, question) = ask(
            &mut session,
            "turn-before-restart",
            "user-before-restart",
            "What changed?",
        );
        assert!(question.accepted);
        let value = serde_json::to_value(session).unwrap();
        assert!(value.get("provider_binding").is_none());
        assert!(value.get("foreground_turn").is_none());
        assert!(value.get("background_run").is_none());
        let mut restored: LiveAssistanceSession = serde_json::from_value(value).unwrap();
        assert_eq!(restored.provider_binding, None);
        assert_eq!(restored.provider_binding_generation(), 0);
        assert!(restored.foreground_turn.is_none());
        assert!(restored.background_run.is_none());
        assert!(!restored.native_recall_provider_ready());
        assert_rejected_unchanged(
            &mut restored,
            AssistanceEvent::ForegroundCompleted {
                session_id: "assist-1".into(),
                turn_id: "turn-before-restart".into(),
                invocation: old_invocation,
            },
            RejectionReason::StaleForegroundResult,
        );
        assert_rejected_unchanged(
            &mut restored,
            AssistanceEvent::UserMessage {
                session_id: "assist-1".into(),
                turn_id: "turn-restored".into(),
                source_event_id: "user-restored".into(),
                text: "What changed?".into(),
            },
            RejectionReason::ProviderCapabilitiesInsufficient,
        );
        assert!(
            restored
                .reduce(AssistanceEvent::ProviderBindingChanged {
                    session_id: "assist-1".into(),
                    binding: proven_native_recall_provider(1),
                })
                .accepted
        );
    }
}
