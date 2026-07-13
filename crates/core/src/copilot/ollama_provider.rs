use super::{
    CancelToken, CopilotModel, CopilotRequest, ModelError, ModelErrorKind, ModelEventSink,
    ModelHealth, ModelHealthStatus, ModelStreamEvent, NudgeDraft,
};
use crate::config::CopilotConfig;
use crate::ollama::{OllamaAdapter, OllamaChatMessage, OllamaError, OllamaStreamRequest};
use chrono::Utc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct OllamaCopilotModel {
    adapter: OllamaAdapter,
}

impl OllamaCopilotModel {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>, timeout: Duration) -> Self {
        Self {
            adapter: OllamaAdapter::new(base_url, model, timeout),
        }
    }

    pub fn from_config(config: &CopilotConfig) -> Self {
        let base_url = std::env::var("OLLAMA_HOST")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "http://localhost:11434".into());
        Self::new(
            base_url,
            config.fast_model.clone(),
            Duration::from_millis(config.target_latency_ms.max(250)),
        )
    }
}

impl CopilotModel for OllamaCopilotModel {
    fn provider_name(&self) -> &str {
        "ollama"
    }

    fn model_name(&self) -> &str {
        self.adapter.model()
    }

    fn prewarm(&self) -> Result<(), ModelError> {
        self.adapter.prewarm().map_err(map_ollama_error)
    }

    fn stream_structured(
        &self,
        request: &CopilotRequest,
        cancel: &CancelToken,
        sink: &dyn ModelEventSink,
    ) -> Result<NudgeDraft, ModelError> {
        let stream_request = OllamaStreamRequest {
            messages: vec![
                OllamaChatMessage::new("system", CopilotRequest::system_prompt()),
                OllamaChatMessage::new("user", request.untrusted_payload()),
            ],
            format: Some(nudge_draft_schema()),
            temperature: Some(0.2),
        };
        let response = self
            .adapter
            .stream_chat(&stream_request, cancel, |text| {
                sink.on_event(ModelStreamEvent::TextDelta(text.to_string()));
            })
            .map_err(map_ollama_error)?;
        let draft = parse_nudge_draft(&response.text)?;
        sink.on_event(ModelStreamEvent::Structured(draft.clone()));
        Ok(draft)
    }

    fn health(&self) -> ModelHealth {
        let health = self.adapter.health();
        ModelHealth {
            provider: self.provider_name().into(),
            model: self.model_name().into(),
            status: if health.available {
                ModelHealthStatus::Available
            } else {
                ModelHealthStatus::Unavailable
            },
            detail: health.detail,
            checked_ts: Utc::now(),
        }
    }
}

fn nudge_draft_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "kind": {
                "type": "string",
                "enum": ["Say", "Ask", "Clarify", "Hold", "Watch"]
            },
            "text": { "type": "string" },
            "source_chip": { "type": "string" }
        },
        "required": ["kind", "text", "source_chip"],
        "additionalProperties": false
    })
}

fn parse_nudge_draft(raw: &str) -> Result<NudgeDraft, ModelError> {
    let trimmed = raw.trim();
    let candidate = if trimmed.starts_with("```") {
        trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
    } else if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}')) {
        &trimmed[start..=end]
    } else {
        trimmed
    };
    serde_json::from_str(candidate).map_err(|error| {
        ModelError::new(
            ModelErrorKind::InvalidResponse,
            format!("Ollama returned invalid nudge JSON: {error}"),
        )
    })
}

fn map_ollama_error(error: OllamaError) -> ModelError {
    match error {
        OllamaError::Cancelled => ModelError::cancelled(),
        OllamaError::Transport(message)
            if message.to_ascii_lowercase().contains("timeout")
                || message.to_ascii_lowercase().contains("timed out") =>
        {
            ModelError::timeout(format!("Ollama fast lane timed out: {message}"))
        }
        OllamaError::Transport(message) => ModelError::new(ModelErrorKind::Unavailable, message),
        OllamaError::Http { status, body } => ModelError::new(
            ModelErrorKind::Unavailable,
            format!("Ollama HTTP {status}: {body}"),
        ),
        OllamaError::StreamRead(message) | OllamaError::InvalidFrame(message) => {
            ModelError::new(ModelErrorKind::InvalidResponse, message)
        }
        OllamaError::EmptyResponse => ModelError::new(
            ModelErrorKind::InvalidResponse,
            "Ollama returned no nudge text",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::NudgeKind;

    #[test]
    fn parses_structured_nudge_with_or_without_fence() {
        let expected = NudgeDraft {
            kind: NudgeKind::Clarify,
            text: "Clarify who owns the launch.".into(),
            source_chip: "launch owner".into(),
        };
        let json = serde_json::to_string(&expected).unwrap();
        assert_eq!(parse_nudge_draft(&json).unwrap(), expected);
        assert_eq!(
            parse_nudge_draft(&format!("```json\n{json}\n```")).unwrap(),
            expected
        );
    }
}
