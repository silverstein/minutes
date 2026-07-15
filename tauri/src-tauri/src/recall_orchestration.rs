//! Native Recall's host-side bridge to the surface-neutral assistance reducer.
//!
//! This module deliberately contains no provider process or UI code. It turns
//! exact host focus and provider attestations into reducer events, then returns
//! immutable dispatch tokens that adapters must echo before output can be
//! published. Keeping this bridge pure makes late-result and focus-race tests
//! deterministic.

use minutes_core::live_sidekick::{
    AssistanceAction, AssistanceEvent, AssistancePosture, AssistanceSurface, CaptureMode,
    CaptureSessionId, EvidenceId, ForegroundTurnId, InvocationIdentity, LiveAssistanceSession,
    LiveAssistanceSessionId, MeetingRef, ProviderAttestationId, ProviderBinding, ProviderBindingId,
    ProviderIsolationProfile, RejectionReason, UserRole,
};
use serde::{Serialize, Serializer};

const RECALL_ENVELOPE_SCHEMA_VERSION: u8 = 2;
const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn is_opaque_wire_id(value: &str) -> bool {
    (1..=256).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_whitespace())
}

fn is_client_request_id(value: &str) -> bool {
    (8..=128).contains(&value.len())
        && value
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

fn is_wire_counter(value: u64, minimum: u64) -> bool {
    value >= minimum && value <= JS_MAX_SAFE_INTEGER
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecallSource {
    Live {
        context_session_id: String,
        mode: CaptureMode,
    },
    Finalized {
        meeting_ref: String,
    },
}

impl RecallSource {
    fn key(&self) -> &str {
        match self {
            Self::Live {
                context_session_id, ..
            } => context_session_id,
            Self::Finalized { meeting_ref } => meeting_ref,
        }
    }
}

/// Authority carried from reducer acceptance to exactly one provider turn.
///
/// The host-only generations are checked before the reducer checks its own
/// invocation identity. This prevents a completion from an old UI focus from
/// being accepted merely because a caller reused a request or turn ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallInvocation {
    pub schema_version: u8,
    pub process_epoch: String,
    pub source_binding_id: String,
    pub assistance_session_id: LiveAssistanceSessionId,
    pub foreground_turn_id: ForegroundTurnId,
    #[serde(serialize_with = "serialize_invocation_identity")]
    pub invocation: InvocationIdentity,
    pub focus_generation: u64,
    pub provider: RecallProviderDispatch,
    pub client_request_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecallInvocationIdentityWire {
    sequence: u64,
    source_policy_generation: u64,
    user_generation: u64,
}

fn serialize_invocation_identity<S>(
    invocation: &InvocationIdentity,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    RecallInvocationIdentityWire {
        sequence: invocation.sequence,
        source_policy_generation: invocation.source_policy_generation,
        user_generation: invocation.user_generation,
    }
    .serialize(serializer)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallProviderDispatch {
    pub binding_id: ProviderBindingId,
    pub generation: u64,
    pub attestation_id: ProviderAttestationId,
    pub profile: ProviderIsolationProfile,
}

impl From<&ProviderBinding> for RecallProviderDispatch {
    fn from(binding: &ProviderBinding) -> Self {
        Self {
            binding_id: binding.binding_id().clone(),
            generation: binding.generation(),
            attestation_id: binding.attestation_id().clone(),
            profile: binding.profile(),
        }
    }
}

/// Every streamed provider event carries the same immutable authority token.
/// `event_sequence` is transport ordering only and grants no authority by
/// itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallStreamEnvelope<T> {
    pub authority: RecallInvocation,
    pub event_sequence: u64,
    pub event_kind: RecallStreamEventKind,
    pub payload: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecallStreamEventKind {
    Status,
    Text,
    Error,
    Done,
    Cancelled,
    Retracted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecallTerminalReason {
    Completed,
    UserCancelled,
    Superseded,
    ProviderChanged,
    FocusChanged,
    SourcePolicyChanged,
    MeetingEnded,
    LifecycleChanged,
    ProviderFailed,
    InternalFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallTerminalPayload {
    pub reason: RecallTerminalReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallTransition {
    pub effects: Vec<RecallEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecallEffect {
    ReducerAction(AssistanceAction),
    EmitTerminal(RecallStreamEnvelope<RecallTerminalPayload>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallBegin {
    pub invocation: RecallInvocation,
    pub effects: Vec<RecallEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionDisposition {
    Publish(Box<RecallTransition>),
    Failed {
        transition: Box<RecallTransition>,
        rejection: CompletionRejection,
    },
    Retract(CompletionRejection),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionRejection {
    WrongProcess,
    WrongSource,
    WrongFocus,
    WrongProviderBinding,
    InvalidEventSequence,
    TerminalEventRequiresTransition,
    StreamClosed,
    Reducer(RejectionReason),
    MissingPublishAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecallOrchestrationError {
    InvalidSource,
    InvalidGeneration,
    Rejected(RejectionReason),
    MissingInvocationAction,
    TerminalStream(CompletionRejection),
}

/// Process-local Native Recall session truth.
///
/// This type must never be persisted as provider authority. The reducer skips
/// provider proof during serialization, and a restored process must re-attest
/// its provider route before accepting a new user message.
#[derive(Debug)]
pub struct RecallOrchestration {
    process_epoch: String,
    source_binding_id: String,
    source: RecallSource,
    focus_generation: u64,
    session: LiveAssistanceSession,
    stream_authority: Option<RecallInvocation>,
    stream_event_sequence: u64,
    stream_closed: bool,
}

impl RecallOrchestration {
    pub fn new(
        assistance_session_id: LiveAssistanceSessionId,
        process_epoch: String,
        source_binding_id: String,
        source: RecallSource,
        focus_generation: u64,
    ) -> Result<Self, RecallOrchestrationError> {
        if !is_opaque_wire_id(&process_epoch)
            || !is_opaque_wire_id(&source_binding_id)
            || source.key().trim().is_empty()
            || !is_opaque_wire_id(assistance_session_id.as_str())
        {
            return Err(RecallOrchestrationError::InvalidSource);
        }
        if !is_wire_counter(focus_generation, 1) {
            return Err(RecallOrchestrationError::InvalidGeneration);
        }

        let mut session = match &source {
            RecallSource::Live { .. } => LiveAssistanceSession::new(
                assistance_session_id.clone(),
                AssistanceSurface::NativeRecall,
                UserRole::Participant,
                AssistancePosture::OnDemand,
            ),
            RecallSource::Finalized { meeting_ref } => LiveAssistanceSession::new_finalized(
                assistance_session_id.clone(),
                AssistanceSurface::NativeRecall,
                UserRole::Participant,
                AssistancePosture::OnDemand,
                MeetingRef::new(meeting_ref),
            )
            .map_err(RecallOrchestrationError::Rejected)?,
        };

        if let RecallSource::Live {
            context_session_id,
            mode,
        } = &source
        {
            let result = session.reduce(AssistanceEvent::CaptureStarted {
                session_id: assistance_session_id,
                capture_session_id: CaptureSessionId::new(context_session_id),
                mode: *mode,
            });
            if !result.accepted {
                return Err(RecallOrchestrationError::Rejected(
                    result.rejection.expect("a rejected reduction has a reason"),
                ));
            }
        }

        Ok(Self {
            process_epoch,
            source_binding_id,
            source,
            focus_generation,
            session,
            stream_authority: None,
            stream_event_sequence: 0,
            stream_closed: true,
        })
    }

    pub fn source(&self) -> &RecallSource {
        &self.source
    }

    pub fn focus_generation(&self) -> u64 {
        self.focus_generation
    }

    pub fn session(&self) -> &LiveAssistanceSession {
        &self.session
    }

    pub fn bind_provider(
        &mut self,
        binding: ProviderBinding,
    ) -> Result<RecallTransition, RecallOrchestrationError> {
        if !is_opaque_wire_id(binding.binding_id().as_str())
            || !is_opaque_wire_id(binding.attestation_id().as_str())
        {
            return Err(RecallOrchestrationError::InvalidSource);
        }
        if !is_wire_counter(binding.generation(), 1) {
            return Err(RecallOrchestrationError::InvalidGeneration);
        }
        if self.session.source_policy_generation >= JS_MAX_SAFE_INTEGER {
            return Err(RecallOrchestrationError::InvalidGeneration);
        }
        let prepared_terminal = self.prepare_active_terminal(
            RecallStreamEventKind::Retracted,
            RecallTerminalReason::ProviderChanged,
        )?;
        let mut candidate = self.session.clone();
        let result = candidate.reduce(AssistanceEvent::ProviderBindingChanged {
            session_id: self.session.id.clone(),
            binding,
        });
        if result.accepted {
            let terminal = if action_cancelled_foreground(&result.actions) {
                Some(self.commit_terminal(
                    prepared_terminal.ok_or(RecallOrchestrationError::MissingInvocationAction)?,
                ))
            } else {
                None
            };
            self.session = candidate;
            Ok(RecallTransition {
                effects: ordered_effects(result.actions, terminal),
            })
        } else {
            Err(RecallOrchestrationError::Rejected(
                result.rejection.expect("a rejected reduction has a reason"),
            ))
        }
    }

    pub fn begin_foreground(
        &mut self,
        client_request_id: &str,
        turn_id: ForegroundTurnId,
        source_event_id: EvidenceId,
        text: String,
    ) -> Result<RecallBegin, RecallOrchestrationError> {
        if !is_client_request_id(client_request_id) || !is_opaque_wire_id(turn_id.as_str()) {
            return Err(RecallOrchestrationError::InvalidSource);
        }
        let prepared_terminal = self.prepare_active_terminal(
            RecallStreamEventKind::Retracted,
            RecallTerminalReason::Superseded,
        )?;
        let mut candidate = self.session.clone();
        let result = candidate.reduce(AssistanceEvent::UserMessage {
            session_id: self.session.id.clone(),
            turn_id: turn_id.clone(),
            source_event_id,
            text,
        });
        if !result.accepted {
            return Err(RecallOrchestrationError::Rejected(
                result.rejection.expect("a rejected reduction has a reason"),
            ));
        }

        let invocation = result.actions.iter().find_map(|action| match action {
            AssistanceAction::RequestReadOnlyForegroundInference {
                turn_id: issued_turn,
                invocation,
                ..
            } if issued_turn == &turn_id => Some(*invocation),
            _ => None,
        });
        let invocation = invocation.ok_or(RecallOrchestrationError::MissingInvocationAction)?;

        if !is_wire_counter(invocation.sequence, 1)
            || !is_wire_counter(invocation.source_policy_generation, 0)
            || !is_wire_counter(invocation.user_generation, 0)
        {
            return Err(RecallOrchestrationError::InvalidGeneration);
        }

        let provider = candidate
            .provider_binding
            .as_ref()
            .map(RecallProviderDispatch::from)
            .ok_or(RecallOrchestrationError::MissingInvocationAction)?;
        let prior_terminal = if action_cancelled_foreground(&result.actions) {
            Some(self.commit_terminal(
                prepared_terminal.ok_or(RecallOrchestrationError::MissingInvocationAction)?,
            ))
        } else {
            None
        };
        let token = RecallInvocation {
            schema_version: RECALL_ENVELOPE_SCHEMA_VERSION,
            process_epoch: self.process_epoch.clone(),
            source_binding_id: self.source_binding_id.clone(),
            assistance_session_id: self.session.id.clone(),
            foreground_turn_id: turn_id,
            invocation,
            focus_generation: self.focus_generation,
            provider,
            client_request_id: client_request_id.to_owned(),
        };
        self.session = candidate;
        self.stream_authority = Some(token.clone());
        self.stream_event_sequence = 0;
        self.stream_closed = false;

        Ok(RecallBegin {
            invocation: token,
            effects: ordered_effects(result.actions, prior_terminal),
        })
    }

    pub fn complete_foreground(&mut self, token: &RecallInvocation) -> CompletionDisposition {
        if let Err(rejection) = self.authorize_event(token) {
            return CompletionDisposition::Retract(rejection);
        }
        let prepared_done = match self.prepare_terminal(
            token,
            RecallStreamEventKind::Done,
            RecallTerminalReason::Completed,
        ) {
            Ok(terminal) => terminal,
            Err(rejection) => return CompletionDisposition::Retract(rejection),
        };
        let prepared_error = match self.prepare_terminal(
            token,
            RecallStreamEventKind::Error,
            RecallTerminalReason::InternalFailure,
        ) {
            Ok(terminal) => terminal,
            Err(rejection) => return CompletionDisposition::Retract(rejection),
        };

        let result = self.session.reduce(AssistanceEvent::ForegroundCompleted {
            session_id: self.session.id.clone(),
            turn_id: token.foreground_turn_id.clone(),
            invocation: token.invocation,
        });
        if !result.accepted {
            return CompletionDisposition::Retract(CompletionRejection::Reducer(
                result.rejection.expect("a rejected reduction has a reason"),
            ));
        }
        if result.actions.iter().any(|action| {
            matches!(
                action,
                AssistanceAction::PublishForegroundResponse {
                    turn_id,
                    invocation,
                } if turn_id == &token.foreground_turn_id && invocation == &token.invocation
            )
        }) {
            let terminal = self.commit_terminal(prepared_done);
            CompletionDisposition::Publish(Box::new(RecallTransition {
                effects: ordered_effects(result.actions, Some(terminal)),
            }))
        } else {
            let terminal = self.commit_terminal(prepared_error);
            CompletionDisposition::Failed {
                transition: Box::new(RecallTransition {
                    effects: ordered_effects(result.actions, Some(terminal)),
                }),
                rejection: CompletionRejection::MissingPublishAction,
            }
        }
    }

    /// Re-check the complete authority token immediately before provider spawn.
    /// Preparing a request and authorizing its dispatch are deliberately two
    /// separate steps so a queued task cannot cross a provider or focus change.
    pub fn authorize_dispatch(&self, token: &RecallInvocation) -> Result<(), CompletionRejection> {
        self.authorize_event(token)
    }

    /// Gate every chunk, terminal completion, error, and cancel acknowledgement
    /// before it is emitted to the frontend.
    pub fn next_stream_event<T>(
        &mut self,
        token: &RecallInvocation,
        event_kind: RecallStreamEventKind,
        payload: T,
    ) -> Result<RecallStreamEnvelope<T>, CompletionRejection> {
        if !matches!(
            event_kind,
            RecallStreamEventKind::Status | RecallStreamEventKind::Text
        ) {
            return Err(CompletionRejection::TerminalEventRequiresTransition);
        }
        self.authorize_event(token)?;
        let next = self
            .stream_event_sequence
            .checked_add(1)
            .ok_or(CompletionRejection::InvalidEventSequence)?;
        if next >= JS_MAX_SAFE_INTEGER {
            return Err(CompletionRejection::InvalidEventSequence);
        }
        self.stream_event_sequence = next;
        Ok(RecallStreamEnvelope {
            authority: token.clone(),
            event_sequence: next,
            event_kind,
            payload,
        })
    }

    fn authorize_event(&self, token: &RecallInvocation) -> Result<(), CompletionRejection> {
        if self.stream_closed {
            return Err(CompletionRejection::StreamClosed);
        }
        if token.schema_version != RECALL_ENVELOPE_SCHEMA_VERSION
            || token.process_epoch != self.process_epoch
        {
            return Err(CompletionRejection::WrongProcess);
        }
        if token.source_binding_id != self.source_binding_id {
            return Err(CompletionRejection::WrongSource);
        }
        if token.focus_generation != self.focus_generation
            || token.assistance_session_id != self.session.id
        {
            return Err(CompletionRejection::WrongFocus);
        }
        let Some(current_provider) = self.session.provider_binding.as_ref() else {
            return Err(CompletionRejection::WrongProviderBinding);
        };
        if RecallProviderDispatch::from(current_provider) != token.provider {
            return Err(CompletionRejection::WrongProviderBinding);
        }
        if self.stream_authority.as_ref() != Some(token) {
            return Err(CompletionRejection::StreamClosed);
        }
        if self.session.foreground_turn.as_ref().is_none_or(|turn| {
            turn.id != token.foreground_turn_id || turn.invocation != token.invocation
        }) {
            return Err(CompletionRejection::Reducer(
                RejectionReason::StaleForegroundResult,
            ));
        }
        Ok(())
    }

    fn prepare_terminal(
        &self,
        token: &RecallInvocation,
        event_kind: RecallStreamEventKind,
        reason: RecallTerminalReason,
    ) -> Result<RecallStreamEnvelope<RecallTerminalPayload>, CompletionRejection> {
        if !matches!(
            event_kind,
            RecallStreamEventKind::Done
                | RecallStreamEventKind::Cancelled
                | RecallStreamEventKind::Retracted
                | RecallStreamEventKind::Error
        ) {
            return Err(CompletionRejection::TerminalEventRequiresTransition);
        }
        if self.stream_closed || self.stream_authority.as_ref() != Some(token) {
            return Err(CompletionRejection::StreamClosed);
        }
        let next = self
            .stream_event_sequence
            .checked_add(1)
            .ok_or(CompletionRejection::InvalidEventSequence)?;
        if next > JS_MAX_SAFE_INTEGER {
            return Err(CompletionRejection::InvalidEventSequence);
        }
        Ok(RecallStreamEnvelope {
            authority: token.clone(),
            event_sequence: next,
            event_kind,
            payload: RecallTerminalPayload { reason },
        })
    }

    fn prepare_active_terminal(
        &self,
        event_kind: RecallStreamEventKind,
        reason: RecallTerminalReason,
    ) -> Result<Option<RecallStreamEnvelope<RecallTerminalPayload>>, RecallOrchestrationError> {
        if self.session.foreground_turn.is_none() {
            return Ok(None);
        }
        let authority =
            self.stream_authority
                .as_ref()
                .ok_or(RecallOrchestrationError::TerminalStream(
                    CompletionRejection::StreamClosed,
                ))?;
        self.prepare_terminal(authority, event_kind, reason)
            .map(Some)
            .map_err(RecallOrchestrationError::TerminalStream)
    }

    fn commit_terminal(
        &mut self,
        terminal: RecallStreamEnvelope<RecallTerminalPayload>,
    ) -> RecallStreamEnvelope<RecallTerminalPayload> {
        debug_assert_eq!(self.stream_authority.as_ref(), Some(&terminal.authority));
        debug_assert!(!self.stream_closed);
        self.stream_event_sequence = terminal.event_sequence;
        self.stream_closed = true;
        terminal
    }

    pub fn cancel_foreground(
        &mut self,
        token: &RecallInvocation,
    ) -> Result<RecallTransition, CompletionRejection> {
        self.authorize_event(token)?;
        let prepared_terminal = self.prepare_terminal(
            token,
            RecallStreamEventKind::Cancelled,
            RecallTerminalReason::UserCancelled,
        )?;
        let result = self.session.reduce(AssistanceEvent::ForegroundCancelled {
            session_id: self.session.id.clone(),
            turn_id: token.foreground_turn_id.clone(),
            invocation: token.invocation,
        });
        if !result.accepted {
            return Err(CompletionRejection::Reducer(
                result.rejection.expect("a rejected reduction has a reason"),
            ));
        }
        let terminal = self.commit_terminal(prepared_terminal);
        Ok(RecallTransition {
            effects: ordered_effects(result.actions, Some(terminal)),
        })
    }

    pub fn fail_foreground(
        &mut self,
        token: &RecallInvocation,
    ) -> Result<RecallTransition, CompletionRejection> {
        self.authorize_event(token)?;
        let prepared_terminal = self.prepare_terminal(
            token,
            RecallStreamEventKind::Error,
            RecallTerminalReason::ProviderFailed,
        )?;
        let result = self.session.reduce(AssistanceEvent::ForegroundFailed {
            session_id: self.session.id.clone(),
            turn_id: token.foreground_turn_id.clone(),
            invocation: token.invocation,
        });
        if !result.accepted {
            return Err(CompletionRejection::Reducer(
                result.rejection.expect("a rejected reduction has a reason"),
            ));
        }
        let terminal = self.commit_terminal(prepared_terminal);
        Ok(RecallTransition {
            effects: ordered_effects(result.actions, Some(terminal)),
        })
    }

    pub fn capture_stopped(&mut self) -> Result<RecallTransition, RecallOrchestrationError> {
        let RecallSource::Live {
            context_session_id, ..
        } = &self.source
        else {
            return Err(RecallOrchestrationError::InvalidSource);
        };
        let prepared_terminal = self.prepare_active_terminal(
            RecallStreamEventKind::Retracted,
            RecallTerminalReason::MeetingEnded,
        )?;
        let result = self.session.reduce(AssistanceEvent::CaptureStopped {
            session_id: self.session.id.clone(),
            capture_session_id: CaptureSessionId::new(context_session_id),
        });
        accepted_transition(self, result, prepared_terminal)
    }

    pub fn invalidate_source_policy(
        &mut self,
        new_generation: u64,
    ) -> Result<RecallTransition, RecallOrchestrationError> {
        if !is_wire_counter(new_generation, 1) {
            return Err(RecallOrchestrationError::InvalidGeneration);
        }
        let prepared_terminal = self.prepare_active_terminal(
            RecallStreamEventKind::Retracted,
            RecallTerminalReason::SourcePolicyChanged,
        )?;
        let result = self
            .session
            .reduce(AssistanceEvent::SourcePolicyInvalidated {
                session_id: self.session.id.clone(),
                new_generation,
            });
        accepted_transition(self, result, prepared_terminal)
    }

    /// Retire the current source before the manager replaces this orchestration
    /// with a newly verified focus. The old per-turn envelope remains available
    /// only as the returned retraction tombstone.
    pub fn retire_for_focus_change(
        &mut self,
    ) -> Result<RecallTransition, RecallOrchestrationError> {
        let new_generation = self
            .session
            .source_policy_generation
            .checked_add(1)
            .ok_or(RecallOrchestrationError::InvalidGeneration)?;
        if !is_wire_counter(new_generation, 1) {
            return Err(RecallOrchestrationError::InvalidGeneration);
        }
        let prepared_terminal = self.prepare_active_terminal(
            RecallStreamEventKind::Retracted,
            RecallTerminalReason::FocusChanged,
        )?;
        let result = self
            .session
            .reduce(AssistanceEvent::SourcePolicyInvalidated {
                session_id: self.session.id.clone(),
                new_generation,
            });
        accepted_transition(self, result, prepared_terminal)
    }

    pub fn processing_started(&mut self) -> Result<RecallTransition, RecallOrchestrationError> {
        let RecallSource::Live {
            context_session_id, ..
        } = &self.source
        else {
            return Err(RecallOrchestrationError::InvalidSource);
        };
        let result = self.session.reduce(AssistanceEvent::ProcessingStarted {
            session_id: self.session.id.clone(),
            capture_session_id: CaptureSessionId::new(context_session_id),
        });
        accepted_transition(self, result, None)
    }

    /// Atomically rebind live capture to a finalized artifact inside the host
    /// lock. Core rotates source policy and clears source-bound state first;
    /// only an accepted transition updates the host focus/source epoch.
    pub fn finalize(
        &mut self,
        meeting_ref: String,
        new_source_binding_id: String,
        new_focus_generation: u64,
    ) -> Result<RecallTransition, RecallOrchestrationError> {
        if meeting_ref.trim().is_empty()
            || !is_opaque_wire_id(&new_source_binding_id)
            || new_focus_generation <= self.focus_generation
            || !is_wire_counter(new_focus_generation, 1)
            || self.session.source_policy_generation >= JS_MAX_SAFE_INTEGER
        {
            return Err(RecallOrchestrationError::InvalidGeneration);
        }
        let RecallSource::Live {
            context_session_id, ..
        } = &self.source
        else {
            return Err(RecallOrchestrationError::InvalidSource);
        };
        let prior_capture_session_id = CaptureSessionId::new(context_session_id);
        let prepared_terminal = self.prepare_active_terminal(
            RecallStreamEventKind::Retracted,
            RecallTerminalReason::LifecycleChanged,
        )?;
        let result = self.session.reduce(AssistanceEvent::MeetingFinalized {
            session_id: self.session.id.clone(),
            capture_session_id: prior_capture_session_id.clone(),
            meeting_ref: MeetingRef::new(&meeting_ref),
        });
        let transition = accepted_transition(self, result, prepared_terminal)?;
        let has_atomic_handoff = transition.effects.iter().any(|effect| {
            matches!(
                effect,
                RecallEffect::ReducerAction(AssistanceAction::FinalizedMeetingAttached {
                    prior_capture_session_id: action_capture,
                    meeting_ref: action_meeting,
                    new_generation,
                }) if action_capture == &prior_capture_session_id
                    && action_meeting.as_str() == meeting_ref
                    && *new_generation == self.session.source_policy_generation
            )
        });
        if !has_atomic_handoff {
            return Err(RecallOrchestrationError::MissingInvocationAction);
        }
        self.source = RecallSource::Finalized { meeting_ref };
        self.source_binding_id = new_source_binding_id;
        self.focus_generation = new_focus_generation;
        Ok(transition)
    }
}

fn action_cancelled_foreground(actions: &[AssistanceAction]) -> bool {
    actions
        .iter()
        .any(|action| matches!(action, AssistanceAction::CancelForeground { .. }))
}

fn ordered_effects(
    actions: Vec<AssistanceAction>,
    mut terminal: Option<RecallStreamEnvelope<RecallTerminalPayload>>,
) -> Vec<RecallEffect> {
    let mut effects = Vec::with_capacity(actions.len() + usize::from(terminal.is_some()));
    for action in actions {
        let closes_foreground = matches!(action, AssistanceAction::CancelForeground { .. });
        effects.push(RecallEffect::ReducerAction(action));
        if closes_foreground {
            if let Some(event) = terminal.take() {
                effects.push(RecallEffect::EmitTerminal(event));
            }
        }
    }
    if let Some(event) = terminal {
        effects.push(RecallEffect::EmitTerminal(event));
    }
    effects
}

fn accepted_transition(
    orchestration: &mut RecallOrchestration,
    result: minutes_core::live_sidekick::Reduction,
    prepared_terminal: Option<RecallStreamEnvelope<RecallTerminalPayload>>,
) -> Result<RecallTransition, RecallOrchestrationError> {
    if result.accepted {
        let terminal = if action_cancelled_foreground(&result.actions) {
            Some(orchestration.commit_terminal(
                prepared_terminal.ok_or(RecallOrchestrationError::MissingInvocationAction)?,
            ))
        } else {
            None
        };
        Ok(RecallTransition {
            effects: ordered_effects(result.actions, terminal),
        })
    } else {
        Err(RecallOrchestrationError::Rejected(
            result.rejection.expect("a rejected reduction has a reason"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minutes_core::live_sidekick::{
        ProviderAttestationId, ProviderBindingId, ProviderIsolationProfile,
    };

    fn binding(generation: u64, profile: ProviderIsolationProfile) -> ProviderBinding {
        ProviderBinding::new(
            ProviderBindingId::new(format!("binding-{generation}")),
            generation,
            ProviderAttestationId::new(format!("attestation-{generation}")),
            profile,
        )
        .unwrap()
    }

    fn terminal_effect(
        effects: &[RecallEffect],
    ) -> Option<&RecallStreamEnvelope<RecallTerminalPayload>> {
        effects.iter().find_map(|effect| match effect {
            RecallEffect::EmitTerminal(terminal) => Some(terminal),
            RecallEffect::ReducerAction(_) => None,
        })
    }

    fn reducer_actions(effects: &[RecallEffect]) -> Vec<&AssistanceAction> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                RecallEffect::ReducerAction(action) => Some(action),
                RecallEffect::EmitTerminal(_) => None,
            })
            .collect()
    }

    fn live_session() -> RecallOrchestration {
        RecallOrchestration::new(
            LiveAssistanceSessionId::new("recall-session-a"),
            "process-epoch-a".into(),
            "source-binding-live-a".into(),
            RecallSource::Live {
                context_session_id: "context-session-a".into(),
                mode: CaptureMode::Recording,
            },
            7,
        )
        .unwrap()
    }

    fn start_turn(session: &mut RecallOrchestration) -> RecallInvocation {
        session
            .bind_provider(binding(1, ProviderIsolationProfile::AgentControlledText))
            .unwrap();
        session
            .begin_foreground(
                "client-a",
                ForegroundTurnId::new("turn-a"),
                EvidenceId::new("user-a"),
                "What changed?".into(),
            )
            .unwrap()
            .invocation
    }

    #[test]
    fn native_recall_requires_attested_provider_before_dispatch() {
        let mut session = live_session();
        let result = session.begin_foreground(
            "client-a",
            ForegroundTurnId::new("turn-a"),
            EvidenceId::new("user-a"),
            "What changed?".into(),
        );
        assert_eq!(
            result,
            Err(RecallOrchestrationError::Rejected(
                RejectionReason::ProviderCapabilitiesInsufficient
            ))
        );
    }

    #[test]
    fn frontend_wire_rejects_unrepresentable_authority_without_mutation() {
        assert!(matches!(
            RecallOrchestration::new(
                LiveAssistanceSessionId::new("recall-session-a"),
                "process-epoch-a".into(),
                "source-binding-a".into(),
                RecallSource::Live {
                    context_session_id: "context session with spaces".into(),
                    mode: CaptureMode::Live,
                },
                JS_MAX_SAFE_INTEGER + 1,
            ),
            Err(RecallOrchestrationError::InvalidGeneration)
        ));

        let mut session = live_session();
        assert_eq!(
            session.bind_provider(binding(
                JS_MAX_SAFE_INTEGER + 1,
                ProviderIsolationProfile::AgentControlledText,
            )),
            Err(RecallOrchestrationError::InvalidGeneration)
        );
        assert!(session.session().provider_binding.is_none());

        session
            .bind_provider(binding(1, ProviderIsolationProfile::AgentControlledText))
            .unwrap();
        let before = session.session().clone();
        assert_eq!(
            session.begin_foreground(
                "invalid request id",
                ForegroundTurnId::new("turn-a"),
                EvidenceId::new("user-a"),
                "Question".into(),
            ),
            Err(RecallOrchestrationError::InvalidSource)
        );
        assert_eq!(session.session(), &before);
    }

    #[test]
    fn invocation_wire_shape_is_camel_case_at_the_frontend_boundary() {
        let mut session = live_session();
        let token = start_turn(&mut session);
        let wire = serde_json::to_value(token).unwrap();

        assert_eq!(wire["schemaVersion"], 2);
        assert_eq!(wire["invocation"]["sequence"], 1);
        assert_eq!(wire["invocation"]["sourcePolicyGeneration"], 1);
        assert_eq!(wire["invocation"]["userGeneration"], 1);
        assert!(wire["invocation"].get("source_policy_generation").is_none());
        assert!(wire["invocation"].get("user_generation").is_none());
    }

    #[test]
    fn rust_serialization_matches_the_node_v2_envelope_golden() {
        let mut session = live_session();
        let _ = start_turn(&mut session);
        let transition = session.retire_for_focus_change().unwrap();
        let terminal = terminal_effect(&transition.effects).unwrap();
        let actual = serde_json::to_value(terminal).unwrap();
        let expected: serde_json::Value = serde_json::from_str(include_str!(
            "../../src/scripts/fixtures/recall-envelope-v2.json"
        ))
        .unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn accepted_completion_is_the_only_publish_authority() {
        let mut session = live_session();
        let token = start_turn(&mut session);
        assert_eq!(token.schema_version, 2);
        assert_eq!(token.focus_generation, 7);
        assert_eq!(token.provider.generation, 1);
        let CompletionDisposition::Publish(transition) = session.complete_foreground(&token) else {
            panic!("accepted completion must publish");
        };
        let terminal = terminal_effect(&transition.effects).expect("completion must close");
        assert_eq!(terminal.event_kind, RecallStreamEventKind::Done);
        assert_eq!(terminal.event_sequence, 1);
        assert_eq!(terminal.payload.reason, RecallTerminalReason::Completed);
        assert_eq!(
            session.complete_foreground(&token),
            CompletionDisposition::Retract(CompletionRejection::StreamClosed)
        );
    }

    #[test]
    fn focus_mismatch_retracts_without_mutating_current_turn() {
        let mut session = live_session();
        let mut stale = start_turn(&mut session);
        stale.focus_generation -= 1;
        assert_eq!(
            session.complete_foreground(&stale),
            CompletionDisposition::Retract(CompletionRejection::WrongFocus)
        );
        stale.focus_generation = session.focus_generation();
        assert!(matches!(
            session.complete_foreground(&stale),
            CompletionDisposition::Publish(_)
        ));
    }

    #[test]
    fn provider_change_retracts_old_completion() {
        let mut session = live_session();
        let token = start_turn(&mut session);
        let transition = session
            .bind_provider(binding(2, ProviderIsolationProfile::Unavailable))
            .unwrap();
        let terminal = terminal_effect(&transition.effects)
            .expect("provider change must retract the active turn");
        assert_eq!(terminal.event_kind, RecallStreamEventKind::Retracted);
        assert_eq!(
            terminal.payload.reason,
            RecallTerminalReason::ProviderChanged
        );
        assert!(matches!(
            transition.effects.as_slice(),
            [
                RecallEffect::ReducerAction(AssistanceAction::CancelForeground { .. }),
                RecallEffect::EmitTerminal(_),
                RecallEffect::ReducerAction(AssistanceAction::PolicyBoundStateCleared { .. }),
                RecallEffect::ReducerAction(AssistanceAction::ProviderBindingUpdated { .. })
            ]
        ));
        assert_eq!(
            session.complete_foreground(&token),
            CompletionDisposition::Retract(CompletionRejection::StreamClosed)
        );
    }

    #[test]
    fn finalized_focus_uses_exact_meeting_reference() {
        let session = RecallOrchestration::new(
            LiveAssistanceSessionId::new("recall-session-final"),
            "process-epoch-a".into(),
            "source-binding-final-a".into(),
            RecallSource::Finalized {
                meeting_ref: "/private/meetings/final.md".into(),
            },
            9,
        )
        .unwrap();
        assert_eq!(
            session.source(),
            &RecallSource::Finalized {
                meeting_ref: "/private/meetings/final.md".into()
            }
        );
        assert_eq!(
            session
                .session()
                .finalized_meeting_ref
                .as_ref()
                .map(MeetingRef::as_str),
            Some("/private/meetings/final.md")
        );
    }

    #[test]
    fn provider_is_rechecked_immediately_before_dispatch_and_every_chunk() {
        let mut session = live_session();
        let token = start_turn(&mut session);
        assert_eq!(session.authorize_dispatch(&token), Ok(()));
        let chunk = RecallStreamEnvelope {
            authority: token.clone(),
            event_sequence: 1,
            event_kind: RecallStreamEventKind::Text,
            payload: "first chunk",
        };
        assert_eq!(
            session
                .next_stream_event(&token, RecallStreamEventKind::Text, "first chunk")
                .unwrap(),
            chunk
        );
        let transition = session
            .bind_provider(binding(2, ProviderIsolationProfile::Unavailable))
            .unwrap();
        assert_eq!(
            terminal_effect(&transition.effects).map(|event| event.event_sequence),
            Some(2)
        );
        assert_eq!(
            session.authorize_dispatch(&token),
            Err(CompletionRejection::StreamClosed)
        );
        assert_eq!(
            session.next_stream_event(&token, RecallStreamEventKind::Text, "late chunk"),
            Err(CompletionRejection::StreamClosed)
        );
    }

    #[test]
    fn backend_mints_contiguous_stream_sequence_and_cancel_retires_authority() {
        let mut session = live_session();
        let token = start_turn(&mut session);
        let first = session
            .next_stream_event(&token, RecallStreamEventKind::Status, "reading")
            .unwrap();
        let second = session
            .next_stream_event(&token, RecallStreamEventKind::Text, "answer")
            .unwrap();
        assert_eq!((first.event_sequence, second.event_sequence), (1, 2));
        let transition = session.cancel_foreground(&token).unwrap();
        assert!(reducer_actions(&transition.effects)
            .iter()
            .any(|action| matches!(
                **action,
                AssistanceAction::CancelForeground {
                    reason: minutes_core::live_sidekick::InvalidationReason::UserCancelled,
                    ..
                }
            )));
        let terminal = terminal_effect(&transition.effects).expect("cancel must close");
        assert_eq!(terminal.event_sequence, 3);
        assert_eq!(terminal.event_kind, RecallStreamEventKind::Cancelled);
        assert_eq!(terminal.payload.reason, RecallTerminalReason::UserCancelled);
        assert_eq!(
            session.next_stream_event(&token, RecallStreamEventKind::Text, "late"),
            Err(CompletionRejection::StreamClosed)
        );
    }

    #[test]
    fn provider_failure_mints_error_then_permanently_closes() {
        let mut session = live_session();
        let token = start_turn(&mut session);
        let text = session
            .next_stream_event(&token, RecallStreamEventKind::Text, "partial")
            .unwrap();
        let transition = session.fail_foreground(&token).unwrap();
        let terminal = terminal_effect(&transition.effects).expect("failure must close");
        assert_eq!((text.event_sequence, terminal.event_sequence), (1, 2));
        assert_eq!(terminal.event_kind, RecallStreamEventKind::Error);
        assert_eq!(
            terminal.payload.reason,
            RecallTerminalReason::ProviderFailed
        );
        assert_eq!(
            session.next_stream_event(&token, RecallStreamEventKind::Text, "late"),
            Err(CompletionRejection::StreamClosed)
        );
    }

    #[test]
    fn focus_and_source_policy_invalidation_each_mint_one_retraction() {
        let mut focus_session = live_session();
        let focus_token = start_turn(&mut focus_session);
        focus_session
            .next_stream_event(&focus_token, RecallStreamEventKind::Text, "partial")
            .unwrap();
        let focus_transition = focus_session.retire_for_focus_change().unwrap();
        let focus_terminal = terminal_effect(&focus_transition.effects).unwrap();
        assert_eq!(focus_terminal.event_sequence, 2);
        assert_eq!(focus_terminal.event_kind, RecallStreamEventKind::Retracted);
        assert_eq!(
            focus_terminal.payload.reason,
            RecallTerminalReason::FocusChanged
        );

        let mut policy_session = live_session();
        let _ = start_turn(&mut policy_session);
        let next_generation = policy_session.session().source_policy_generation + 1;
        let policy_transition = policy_session
            .invalidate_source_policy(next_generation)
            .unwrap();
        let policy_terminal = terminal_effect(&policy_transition.effects).unwrap();
        assert_eq!(policy_terminal.event_sequence, 1);
        assert_eq!(
            policy_terminal.payload.reason,
            RecallTerminalReason::SourcePolicyChanged
        );
        assert!(matches!(
            policy_transition.effects.as_slice(),
            [
                RecallEffect::ReducerAction(AssistanceAction::CancelForeground { .. }),
                RecallEffect::EmitTerminal(_),
                RecallEffect::ReducerAction(AssistanceAction::PolicyBoundStateCleared { .. })
            ]
        ));
    }

    #[test]
    fn terminal_kinds_cannot_be_minted_before_their_reducer_transition() {
        let mut session = live_session();
        let token = start_turn(&mut session);
        for kind in [
            RecallStreamEventKind::Done,
            RecallStreamEventKind::Cancelled,
            RecallStreamEventKind::Retracted,
            RecallStreamEventKind::Error,
        ] {
            assert_eq!(
                session.next_stream_event(&token, kind, "too early"),
                Err(CompletionRejection::TerminalEventRequiresTransition)
            );
        }
        assert_eq!(
            session
                .next_stream_event(&token, RecallStreamEventKind::Text, "allowed")
                .unwrap()
                .event_sequence,
            1
        );
    }

    #[test]
    fn live_chunks_reserve_the_last_sequence_for_a_terminal_tombstone() {
        let mut session = live_session();
        let token = start_turn(&mut session);
        session.stream_event_sequence = JS_MAX_SAFE_INTEGER - 1;

        assert_eq!(
            session.next_stream_event(&token, RecallStreamEventKind::Text, "too late"),
            Err(CompletionRejection::InvalidEventSequence)
        );

        let transition = session.cancel_foreground(&token).unwrap();
        let terminal = terminal_effect(&transition.effects).expect("reserved terminal slot");
        assert_eq!(terminal.event_sequence, JS_MAX_SAFE_INTEGER);
        assert_eq!(terminal.event_kind, RecallStreamEventKind::Cancelled);
    }

    #[test]
    fn a_new_user_turn_retracts_the_prior_stream_before_replacement() {
        let mut session = live_session();
        let first = start_turn(&mut session);
        session
            .next_stream_event(&first, RecallStreamEventKind::Text, "partial")
            .unwrap();
        let begin = session
            .begin_foreground(
                "client-b",
                ForegroundTurnId::new("turn-b"),
                EvidenceId::new("user-b"),
                "New question".into(),
            )
            .unwrap();
        assert!(reducer_actions(&begin.effects)
            .iter()
            .any(|action| matches!(**action, AssistanceAction::CancelForeground { .. })));
        let second = begin.invocation.clone();
        let prior_terminal =
            terminal_effect(&begin.effects).expect("replacement must retract the prior stream");
        assert_eq!(prior_terminal.event_sequence, 2);
        assert_eq!(prior_terminal.event_kind, RecallStreamEventKind::Retracted);
        assert_eq!(
            prior_terminal.payload.reason,
            RecallTerminalReason::Superseded
        );
        assert!(matches!(
            begin.effects.as_slice(),
            [
                RecallEffect::ReducerAction(AssistanceAction::CancelForeground { .. }),
                RecallEffect::EmitTerminal(_),
                RecallEffect::ReducerAction(AssistanceAction::AcknowledgeForeground { .. }),
                RecallEffect::ReducerAction(
                    AssistanceAction::RequestReadOnlyForegroundInference { .. }
                )
            ]
        ));
        assert_eq!(
            session
                .next_stream_event(&second, RecallStreamEventKind::Text, "fresh")
                .unwrap()
                .event_sequence,
            1
        );
    }

    #[test]
    fn stream_event_kinds_are_typed_for_every_visible_terminal_state() {
        let kinds = [
            RecallStreamEventKind::Status,
            RecallStreamEventKind::Text,
            RecallStreamEventKind::Error,
            RecallStreamEventKind::Done,
            RecallStreamEventKind::Cancelled,
            RecallStreamEventKind::Retracted,
        ];
        assert_eq!(kinds.len(), 6);
    }

    #[test]
    fn wrong_process_epoch_cannot_replay_after_restart() {
        let mut session = live_session();
        let mut token = start_turn(&mut session);
        token.process_epoch = "prior-process".into();
        assert_eq!(
            session.complete_foreground(&token),
            CompletionDisposition::Retract(CompletionRejection::WrongProcess)
        );
    }

    #[test]
    fn finalization_rotates_source_and_focus_authority() {
        let mut session = live_session();
        let live_token = start_turn(&mut session);
        let stopped = session.capture_stopped().unwrap();
        assert_eq!(
            terminal_effect(&stopped.effects).map(|terminal| terminal.payload.reason),
            Some(RecallTerminalReason::MeetingEnded)
        );
        assert_eq!(
            session.complete_foreground(&live_token),
            CompletionDisposition::Retract(CompletionRejection::StreamClosed)
        );
        session.processing_started().unwrap();
        let processing_token = session
            .begin_foreground(
                "client-processing",
                ForegroundTurnId::new("turn-processing"),
                EvidenceId::new("user-processing"),
                "Summarize before attachment".into(),
            )
            .unwrap()
            .invocation;
        session
            .next_stream_event(
                &processing_token,
                RecallStreamEventKind::Text,
                "partial processing answer",
            )
            .unwrap();
        let transition = session
            .finalize(
                "/private/meetings/final.md".into(),
                "source-binding-final-a".into(),
                8,
            )
            .unwrap();
        assert!(matches!(
            transition.effects.as_slice(),
            [
                RecallEffect::ReducerAction(AssistanceAction::CancelForeground { .. }),
                RecallEffect::EmitTerminal(_),
                RecallEffect::ReducerAction(AssistanceAction::PolicyBoundStateCleared {
                    new_generation: 2
                }),
                RecallEffect::ReducerAction(AssistanceAction::FinalizedMeetingAttached { .. })
            ]
        ));
        let terminal = terminal_effect(&transition.effects).expect("finalization retracts stream");
        assert_eq!(terminal.event_sequence, 2);
        assert_eq!(terminal.event_kind, RecallStreamEventKind::Retracted);
        assert_eq!(
            terminal.payload.reason,
            RecallTerminalReason::LifecycleChanged
        );
        assert_eq!(session.focus_generation(), 8);
        assert_eq!(
            session.complete_foreground(&processing_token),
            CompletionDisposition::Retract(CompletionRejection::StreamClosed)
        );
        assert!(session.session().evidence.is_empty());
    }
}
