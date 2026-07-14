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
mod mode;
mod ollama_provider;
mod policy;
mod provider;
mod runner;
mod strategy;
mod topic;
mod types;

pub use crate::ollama::CancelToken;
pub use battle_card::{BattleCard, BattleCardError, GroundingSource, RepositoryGrounding};
pub use clock::{CopilotClock, SystemCopilotClock};
pub use control::{
    clear_session_controls, copilot_feedback_path, copilot_pause_path, copilot_pid_path,
    copilot_status_path, copilot_stop_path, create_session_guard, read_session_status,
    request_feedback, request_pause, request_resume, request_stop, take_feedback_request,
    write_session_status, CopilotEvidenceMode, CopilotFeedbackRequest, CopilotSessionStatus,
};
pub use latency::{LatencyRecord, PartialLatencySeed};
pub use mode::{MeetingMode, MeetingModePolicy, OpportunityKind};
pub use ollama_provider::OllamaCopilotModel;
pub use policy::{CopilotFeedback, NudgePolicy, PolicySnapshot};
pub use provider::{
    AppleFoundationCopilotModel, CloudCopilotModel, CopilotModel, ModelError, ModelErrorKind,
    ModelEventSink, ModelHealth, ModelHealthStatus, ModelStreamEvent,
};
pub use runner::{
    CopilotRunner, DepthLaneConfig, DepthLaneSnapshot, FeedbackOutcome, RunnerEvent, SubmitOutcome,
};
pub use strategy::{StrategyRefreshReason, StrategyRequest, StrategyState, StrategyStateDraft};
pub use topic::{is_decisive_final, keywords as topic_keywords, TopicShift, TopicShiftDetector};
pub use types::{
    CopilotHealth, CopilotRequest, CopilotState, CopilotUtterance, Nudge, NudgeDraft, NudgeKind,
    TranscriptUpdateKind, COPILOT_CONTRACT_VERSION,
};
