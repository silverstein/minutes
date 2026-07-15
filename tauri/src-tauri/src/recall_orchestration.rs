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
use serde::Serialize;

const RECALL_ENVELOPE_SCHEMA_VERSION: u8 = 2;

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
    pub invocation: InvocationIdentity,
    pub focus_generation: u64,
    pub provider: RecallProviderDispatch,
    pub client_request_id: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionDisposition {
    Publish,
    Retract(CompletionRejection),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionRejection {
    WrongProcess,
    WrongSource,
    WrongFocus,
    WrongProviderBinding,
    InvalidEventSequence,
    Reducer(RejectionReason),
    MissingPublishAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecallOrchestrationError {
    InvalidSource,
    InvalidGeneration,
    Rejected(RejectionReason),
    MissingInvocationAction,
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
    stream_event_sequence: u64,
}

impl RecallOrchestration {
    pub fn new(
        assistance_session_id: LiveAssistanceSessionId,
        process_epoch: String,
        source_binding_id: String,
        source: RecallSource,
        focus_generation: u64,
    ) -> Result<Self, RecallOrchestrationError> {
        if process_epoch.trim().is_empty()
            || source_binding_id.trim().is_empty()
            || source.key().trim().is_empty()
        {
            return Err(RecallOrchestrationError::InvalidSource);
        }
        if focus_generation == 0 {
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
            stream_event_sequence: 0,
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
    ) -> Result<Vec<AssistanceAction>, RecallOrchestrationError> {
        let result = self
            .session
            .reduce(AssistanceEvent::ProviderBindingChanged {
                session_id: self.session.id.clone(),
                binding,
            });
        if result.accepted {
            Ok(result.actions)
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
    ) -> Result<(RecallInvocation, Vec<AssistanceAction>), RecallOrchestrationError> {
        if client_request_id.trim().is_empty() {
            return Err(RecallOrchestrationError::InvalidSource);
        }
        let result = self.session.reduce(AssistanceEvent::UserMessage {
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

        let provider = self
            .session
            .provider_binding
            .as_ref()
            .map(RecallProviderDispatch::from)
            .ok_or(RecallOrchestrationError::MissingInvocationAction)?;
        self.stream_event_sequence = 0;

        Ok((
            RecallInvocation {
                schema_version: RECALL_ENVELOPE_SCHEMA_VERSION,
                process_epoch: self.process_epoch.clone(),
                source_binding_id: self.source_binding_id.clone(),
                assistance_session_id: self.session.id.clone(),
                foreground_turn_id: turn_id,
                invocation,
                focus_generation: self.focus_generation,
                provider,
                client_request_id: client_request_id.to_owned(),
            },
            result.actions,
        ))
    }

    pub fn complete_foreground(&mut self, token: &RecallInvocation) -> CompletionDisposition {
        if let Err(rejection) = self.authorize_event(token) {
            return CompletionDisposition::Retract(rejection);
        }

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
            CompletionDisposition::Publish
        } else {
            CompletionDisposition::Retract(CompletionRejection::MissingPublishAction)
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
        self.authorize_event(token)?;
        let next = self
            .stream_event_sequence
            .checked_add(1)
            .ok_or(CompletionRejection::InvalidEventSequence)?;
        self.stream_event_sequence = next;
        Ok(RecallStreamEnvelope {
            authority: token.clone(),
            event_sequence: next,
            event_kind,
            payload,
        })
    }

    fn authorize_event(&self, token: &RecallInvocation) -> Result<(), CompletionRejection> {
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
        if self.session.foreground_turn.as_ref().is_none_or(|turn| {
            turn.id != token.foreground_turn_id || turn.invocation != token.invocation
        }) {
            return Err(CompletionRejection::Reducer(
                RejectionReason::StaleForegroundResult,
            ));
        }
        Ok(())
    }

    pub fn cancel_foreground(
        &mut self,
        token: &RecallInvocation,
    ) -> Result<Vec<AssistanceAction>, CompletionRejection> {
        self.authorize_event(token)?;
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
        self.stream_event_sequence = 0;
        Ok(result.actions)
    }

    pub fn capture_stopped(&mut self) -> Result<Vec<AssistanceAction>, RecallOrchestrationError> {
        let RecallSource::Live {
            context_session_id, ..
        } = &self.source
        else {
            return Err(RecallOrchestrationError::InvalidSource);
        };
        let result = self.session.reduce(AssistanceEvent::CaptureStopped {
            session_id: self.session.id.clone(),
            capture_session_id: CaptureSessionId::new(context_session_id),
        });
        accepted_actions(result)
    }

    pub fn processing_started(
        &mut self,
    ) -> Result<Vec<AssistanceAction>, RecallOrchestrationError> {
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
        accepted_actions(result)
    }

    /// Atomically rebind live capture to a finalized artifact inside the host
    /// lock. Core rotates source policy and clears source-bound state first;
    /// only an accepted transition updates the host focus/source epoch.
    pub fn finalize(
        &mut self,
        meeting_ref: String,
        new_source_binding_id: String,
        new_focus_generation: u64,
    ) -> Result<Vec<AssistanceAction>, RecallOrchestrationError> {
        if meeting_ref.trim().is_empty()
            || new_source_binding_id.trim().is_empty()
            || new_focus_generation <= self.focus_generation
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
        let result = self.session.reduce(AssistanceEvent::MeetingFinalized {
            session_id: self.session.id.clone(),
            capture_session_id: prior_capture_session_id.clone(),
            meeting_ref: MeetingRef::new(&meeting_ref),
        });
        let actions = accepted_actions(result)?;
        let has_atomic_handoff = actions.iter().any(|action| {
            matches!(
                action,
                AssistanceAction::FinalizedMeetingAttached {
                    prior_capture_session_id: action_capture,
                    meeting_ref: action_meeting,
                    new_generation,
                } if action_capture == &prior_capture_session_id
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
        Ok(actions)
    }
}

fn accepted_actions(
    result: minutes_core::live_sidekick::Reduction,
) -> Result<Vec<AssistanceAction>, RecallOrchestrationError> {
    if result.accepted {
        Ok(result.actions)
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
            .0
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
    fn accepted_completion_is_the_only_publish_authority() {
        let mut session = live_session();
        let token = start_turn(&mut session);
        assert_eq!(token.schema_version, 2);
        assert_eq!(token.focus_generation, 7);
        assert_eq!(token.provider.generation, 1);
        assert_eq!(
            session.complete_foreground(&token),
            CompletionDisposition::Publish
        );
        assert_eq!(
            session.complete_foreground(&token),
            CompletionDisposition::Retract(CompletionRejection::Reducer(
                RejectionReason::StaleForegroundResult
            ))
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
        assert_eq!(
            session.complete_foreground(&stale),
            CompletionDisposition::Publish
        );
    }

    #[test]
    fn provider_change_retracts_old_completion() {
        let mut session = live_session();
        let token = start_turn(&mut session);
        session
            .bind_provider(binding(2, ProviderIsolationProfile::Unavailable))
            .unwrap();
        assert_eq!(
            session.complete_foreground(&token),
            CompletionDisposition::Retract(CompletionRejection::WrongProviderBinding)
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
        session
            .bind_provider(binding(2, ProviderIsolationProfile::Unavailable))
            .unwrap();
        assert_eq!(
            session.authorize_dispatch(&token),
            Err(CompletionRejection::WrongProviderBinding)
        );
        assert_eq!(
            session.next_stream_event(&token, RecallStreamEventKind::Text, "late chunk"),
            Err(CompletionRejection::WrongProviderBinding)
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
        let actions = session.cancel_foreground(&token).unwrap();
        assert!(actions.iter().any(|action| matches!(
            action,
            AssistanceAction::CancelForeground {
                reason: minutes_core::live_sidekick::InvalidationReason::UserCancelled,
                ..
            }
        )));
        assert_eq!(
            session.next_stream_event(&token, RecallStreamEventKind::Text, "late"),
            Err(CompletionRejection::Reducer(
                RejectionReason::StaleForegroundResult
            ))
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
        let old_token = start_turn(&mut session);
        session.capture_stopped().unwrap();
        session.processing_started().unwrap();
        let actions = session
            .finalize(
                "/private/meetings/final.md".into(),
                "source-binding-final-a".into(),
                8,
            )
            .unwrap();
        assert!(actions.iter().any(|action| matches!(
            action,
            AssistanceAction::PolicyBoundStateCleared { new_generation: 2 }
        )));
        assert_eq!(session.focus_generation(), 8);
        assert_eq!(
            session.complete_foreground(&old_token),
            CompletionDisposition::Retract(CompletionRejection::WrongSource)
        );
        assert!(session.session().evidence.is_empty());
    }
}
