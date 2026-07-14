use super::{CancelToken, CopilotRequest, NudgeDraft};
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
    fn health(&self) -> ModelHealth;
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
