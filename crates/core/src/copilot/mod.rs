//! Portable real-time meeting copilot.
//!
//! This module consumes revisioned transcript evidence and emits short-lived,
//! versioned nudges. It never owns capture, mutates transcript history, or
//! exposes arbitrary tools to the fast model lane.

mod apple_fm_provider;
mod battle_card;
mod control;
mod ollama_provider;
mod policy;
mod provider;
mod runner;
mod types;

pub use crate::ollama::CancelToken;
pub use apple_fm_provider::{
    replay_gate_key as apple_fm_replay_gate_key, AppleFoundationCopilotModel,
    APPLE_FM_COPILOT_MODEL, APPLE_FM_COPILOT_PROMPT_VERSION,
};
pub use battle_card::{BattleCard, BattleCardError};
pub use control::{
    clear_session_controls, copilot_pause_path, copilot_pid_path, copilot_status_path,
    copilot_stop_path, create_session_guard, read_session_status, request_pause, request_resume,
    request_stop, write_session_status, CopilotSessionStatus,
};
pub use ollama_provider::OllamaCopilotModel;
pub use policy::NudgePolicy;
pub use provider::{
    CloudCopilotModel, CopilotModel, ModelError, ModelErrorKind, ModelEventSink, ModelHealth,
    ModelHealthStatus, ModelStreamEvent,
};
pub use runner::{CopilotRunner, RunnerEvent, SubmitOutcome};
pub use types::{
    CopilotHealth, CopilotRequest, CopilotState, CopilotUtterance, Nudge, NudgeDraft, NudgeKind,
    TranscriptUpdateKind, COPILOT_CONTRACT_VERSION,
};

/// Whether the Apple Foundation Models copilot lane is genuinely usable in
/// this build and on this machine.
///
/// Going through the provider contract (rather than assuming every macOS host
/// has a usable implementation) makes contract stubs fail closed: their
/// health is `NotImplemented`, while unsupported machines report
/// `Unavailable`. Only a constructed provider reporting `Available` may win
/// `auto-local` selection.
pub fn apple_fm_is_available() -> bool {
    let model = AppleFoundationCopilotModel::new(APPLE_FM_COPILOT_MODEL);
    model_health_is_available(&model.health())
}

fn model_health_is_available(health: &ModelHealth) -> bool {
    health.status == ModelHealthStatus::Available
}

#[cfg(test)]
mod availability_tests {
    use super::*;
    use chrono::Utc;

    fn health(status: ModelHealthStatus) -> ModelHealth {
        ModelHealth {
            provider: "apple-fm".into(),
            model: APPLE_FM_COPILOT_MODEL.into(),
            status,
            detail: "test capability".into(),
            checked_ts: Utc::now(),
        }
    }

    #[test]
    fn apple_fm_availability_fails_closed_for_stub_or_unhealthy_provider() {
        assert!(model_health_is_available(&health(
            ModelHealthStatus::Available
        )));
        assert!(!model_health_is_available(&health(
            ModelHealthStatus::Degraded
        )));
        assert!(!model_health_is_available(&health(
            ModelHealthStatus::Unavailable
        )));
        assert!(!model_health_is_available(&health(
            ModelHealthStatus::NotImplemented
        )));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn apple_fm_availability_is_false_off_macos() {
        assert!(!apple_fm_is_available());
    }
}
