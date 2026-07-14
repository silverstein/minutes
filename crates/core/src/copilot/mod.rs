//! Portable real-time meeting copilot.
//!
//! This module consumes revisioned transcript evidence and emits short-lived,
//! versioned nudges. It never owns capture, mutates transcript history, or
//! exposes arbitrary tools to the fast model lane.

mod battle_card;
mod clock;
mod control;
pub mod eval;
mod latency;
mod ollama_provider;
mod policy;
mod provider;
mod runner;
mod types;

pub use crate::ollama::CancelToken;
pub use battle_card::{BattleCard, BattleCardError};
pub use clock::{CopilotClock, SystemCopilotClock};
pub use control::{
    clear_session_controls, copilot_pause_path, copilot_pid_path, copilot_status_path,
    copilot_stop_path, create_session_guard, read_session_status, request_pause, request_resume,
    request_stop, write_session_status, CopilotEvidenceMode, CopilotSessionStatus,
};
pub use latency::{LatencyRecord, PartialLatencySeed};
pub use ollama_provider::OllamaCopilotModel;
pub use policy::NudgePolicy;
pub use provider::{
    AppleFoundationCopilotModel, CloudCopilotModel, CopilotModel, ModelError, ModelErrorKind,
    ModelEventSink, ModelHealth, ModelHealthStatus, ModelStreamEvent,
};
pub use runner::{CopilotRunner, RunnerEvent, SubmitOutcome};
pub use types::{
    CopilotHealth, CopilotRequest, CopilotState, CopilotUtterance, Nudge, NudgeDraft, NudgeKind,
    TranscriptUpdateKind, COPILOT_CONTRACT_VERSION,
};
