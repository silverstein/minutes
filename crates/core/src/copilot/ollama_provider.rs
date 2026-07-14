use super::{
    CancelToken, CopilotModel, CopilotRequest, ModelError, ModelErrorKind, ModelEventSink,
    ModelHealth, ModelHealthStatus, ModelStreamEvent, NudgeDraft, StrategyRequest, StrategyState,
    StrategyStateDraft,
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
        self.adapter
            .prewarm()
            .map_err(|error| map_ollama_error(error, "prewarm"))
    }

    fn stream_structured(
        &self,
        request: &CopilotRequest,
        cancel: &CancelToken,
        sink: &dyn ModelEventSink,
    ) -> Result<NudgeDraft, ModelError> {
        let stream_request = OllamaStreamRequest {
            messages: vec![
                OllamaChatMessage::new("system", request.trusted_system_prompt()),
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
            .map_err(|error| map_ollama_error(error, "fast lane"))?;
        let draft = parse_nudge_draft(&response.text)?;
        sink.on_event(ModelStreamEvent::Structured(draft.clone()));
        Ok(draft)
    }

    fn refresh_strategy(
        &self,
        request: &StrategyRequest,
        cancel: &CancelToken,
    ) -> Result<StrategyState, ModelError> {
        let stream_request = OllamaStreamRequest {
            messages: vec![
                OllamaChatMessage::new("system", request.system_prompt()),
                OllamaChatMessage::new("user", request.untrusted_payload()),
            ],
            format: Some(strategy_state_schema()),
            temperature: Some(0.1),
        };
        let response = self
            .adapter
            .stream_chat(&stream_request, cancel, |_| {})
            .map_err(|error| map_ollama_error(error, "depth lane"))?;
        let draft = parse_json::<StrategyStateDraft>(&response.text, "strategy state")?;
        Ok(StrategyState::from_draft(draft, request.evidence_revision))
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

    fn context_window_tokens(&self) -> usize {
        // Ollama models can override num_ctx at generation time, but the
        // portable default is 8k. The copilot's bounded prompt remains below
        // this and routing can compare it with other provider capacities.
        8_192
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
            ,"opportunity": {
                "type": "string",
                "enum": ["pain", "objection", "next_step", "evidence", "decision", "leverage", "rapport", "clarity", "safety", "general"]
            },
            "confidence": { "type": "integer", "minimum": 0, "maximum": 100 }
        },
        "required": ["kind", "text", "source_chip", "opportunity", "confidence"],
        "additionalProperties": false
    })
}

fn strategy_state_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "open_threads": { "type": "array", "items": { "type": "string" }, "maxItems": 6 },
            "unmet_goal_items": { "type": "array", "items": { "type": "string" }, "maxItems": 6 },
            "unresolved_objections": { "type": "array", "items": { "type": "string" }, "maxItems": 6 },
            "steer_toward": { "type": "array", "items": { "type": "string" }, "maxItems": 6 }
        },
        "required": ["open_threads", "unmet_goal_items", "unresolved_objections", "steer_toward"],
        "additionalProperties": false
    })
}

fn parse_nudge_draft(raw: &str) -> Result<NudgeDraft, ModelError> {
    parse_json(raw, "nudge")
}

fn parse_json<T: serde::de::DeserializeOwned>(
    raw: &str,
    response_kind: &str,
) -> Result<T, ModelError> {
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
            format!("Ollama returned invalid {response_kind} JSON: {error}"),
        )
    })
}

fn map_ollama_error(error: OllamaError, lane: &str) -> ModelError {
    match error {
        OllamaError::Cancelled => ModelError::cancelled(),
        OllamaError::Transport(message)
            if message.to_ascii_lowercase().contains("timeout")
                || message.to_ascii_lowercase().contains("timed out") =>
        {
            ModelError::timeout(format!("Ollama {lane} timed out: {message}"))
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
            opportunity: super::super::OpportunityKind::Clarity,
            confidence: 87,
        };
        let json = serde_json::to_string(&expected).unwrap();
        assert_eq!(parse_nudge_draft(&json).unwrap(), expected);
        assert_eq!(
            parse_nudge_draft(&format!("```json\n{json}\n```")).unwrap(),
            expected
        );
    }
}
