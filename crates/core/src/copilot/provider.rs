use super::{CancelToken, CopilotRequest, NudgeDraft, StrategyRequest, StrategyState};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelHealthStatus {
    Available,
    Degraded,
    Unavailable,
    NotImplemented,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPrivacyClass {
    OnDevice,
    LocalService,
    Cloud,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelHealth {
    pub provider: String,
    pub model: String,
    pub status: ModelHealthStatus,
    pub detail: String,
    pub checked_ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelErrorKind {
    Cancelled,
    Timeout,
    Unavailable,
    InvalidResponse,
    NotImplemented,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("{message}")]
pub struct ModelError {
    pub kind: ModelErrorKind,
    pub message: String,
}

impl ModelError {
    pub fn new(kind: ModelErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn cancelled() -> Self {
        Self::new(ModelErrorKind::Cancelled, "copilot model request cancelled")
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ModelErrorKind::Timeout, message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelStreamEvent {
    TextDelta(String),
    Structured(NudgeDraft),
}

pub trait ModelEventSink: Send + Sync {
    fn on_event(&self, event: ModelStreamEvent);
}

impl<F> ModelEventSink for F
where
    F: Fn(ModelStreamEvent) + Send + Sync,
{
    fn on_event(&self, event: ModelStreamEvent) {
        self(event);
    }
}

/// Fast-lane provider contract. It has no tool API by design.
pub trait CopilotModel: Send + Sync + 'static {
    fn provider_name(&self) -> &str;
    fn model_name(&self) -> &str;
    fn prewarm(&self) -> Result<(), ModelError>;
    fn stream_structured(
        &self,
        request: &CopilotRequest,
        cancel: &CancelToken,
        sink: &dyn ModelEventSink,
    ) -> Result<NudgeDraft, ModelError>;
    /// Slow-lane strategy refresh. Providers may override this with a compact
    /// model call; the deterministic fallback keeps custom providers source
    /// compatible and never emits a second user-facing stream.
    fn refresh_strategy(
        &self,
        request: &StrategyRequest,
        cancel: &CancelToken,
    ) -> Result<StrategyState, ModelError> {
        if cancel.is_cancelled() {
            return Err(ModelError::cancelled());
        }
        Ok(StrategyState::from_draft(
            request.heuristic_draft(),
            request.evidence_revision,
        ))
    }
    fn health(&self) -> ModelHealth;

    /// Privacy boundary used by automatic routing. Providers fail closed to a
    /// local service unless they explicitly declare another boundary.
    fn privacy_class(&self) -> ModelPrivacyClass {
        ModelPrivacyClass::LocalService
    }

    /// Usable context capacity for the configured model. The fast lane keeps
    /// prompts compact, but routing must still reject a model that cannot hold
    /// the current policy/evidence budget.
    fn context_window_tokens(&self) -> usize {
        4_096
    }
}

/// Trait-shaped placeholder for an explicitly configured future cloud lane.
/// It cannot send data in this PR.
#[derive(Debug, Clone)]
pub struct CloudCopilotModel {
    model: String,
}

impl CloudCopilotModel {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

impl CopilotModel for CloudCopilotModel {
    fn provider_name(&self) -> &str {
        "cloud"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn prewarm(&self) -> Result<(), ModelError> {
        Err(not_implemented("cloud"))
    }

    fn stream_structured(
        &self,
        _request: &CopilotRequest,
        _cancel: &CancelToken,
        _sink: &dyn ModelEventSink,
    ) -> Result<NudgeDraft, ModelError> {
        Err(not_implemented("cloud"))
    }

    fn health(&self) -> ModelHealth {
        stub_health("cloud", &self.model)
    }

    fn privacy_class(&self) -> ModelPrivacyClass {
        ModelPrivacyClass::Cloud
    }

    fn context_window_tokens(&self) -> usize {
        128_000
    }
}

/// Trait-shaped placeholder for a future macOS acceleration. This type is
/// available cross-platform so callers never make Apple FM the baseline.
#[derive(Debug, Clone)]
pub struct AppleFoundationCopilotModel {
    model: String,
}

impl AppleFoundationCopilotModel {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

impl CopilotModel for AppleFoundationCopilotModel {
    fn provider_name(&self) -> &str {
        "apple-fm"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn prewarm(&self) -> Result<(), ModelError> {
        Err(not_implemented("Apple Foundation Models"))
    }

    fn stream_structured(
        &self,
        _request: &CopilotRequest,
        _cancel: &CancelToken,
        _sink: &dyn ModelEventSink,
    ) -> Result<NudgeDraft, ModelError> {
        Err(not_implemented("Apple Foundation Models"))
    }

    fn health(&self) -> ModelHealth {
        stub_health("apple-fm", &self.model)
    }

    fn privacy_class(&self) -> ModelPrivacyClass {
        ModelPrivacyClass::OnDevice
    }
}

fn not_implemented(provider: &str) -> ModelError {
    ModelError::new(
        ModelErrorKind::NotImplemented,
        format!("{provider} copilot provider is not implemented in contract v1"),
    )
}

fn stub_health(provider: &str, model: &str) -> ModelHealth {
    ModelHealth {
        provider: provider.into(),
        model: model.into(),
        status: ModelHealthStatus::NotImplemented,
        detail: "provider stub only; no data will be sent".into(),
        checked_ts: Utc::now(),
    }
}
