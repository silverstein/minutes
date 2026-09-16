//! Wire-policy helpers, independent of macOS audio and the voice socket loop.
//! Official contract checked 2026-09-16:
//! https://ai.google.dev/gemini-api/docs/models/gemini-3.8-live-extended-thinking

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveModel {
    Standard,
    ExtendedThinking,
}

impl LiveModel {
    /// Unknown models fail closed instead of inheriting a guessed protocol.
    pub fn from_id(id: &str) -> Option<Self> {
        match id.strip_prefix("models/").unwrap_or(id) {
            "gemini-3.8-live" => Some(Self::Standard),
            "gemini-3.8-live-extended-thinking" => Some(Self::ExtendedThinking),
            _ => None,
        }
    }

    pub fn tool_response(self, id: &str, name: &str, result: Value, delivery: Delivery) -> Value {
        let body = if result.is_object() {
            result
        } else {
            json!({"result":result})
        };
        let mut response = json!({"id": id, "name": name, "response": body});
        // Extended Thinking explicitly disallows scheduling configuration.
        if self == Self::Standard {
            response["response"]["scheduling"] = json!(delivery.scheduling());
        }
        json!({"toolResponse": {"functionResponses": [response]}})
    }

    pub fn function_declaration(self, name: &str, description: &str, parameters: Value) -> Value {
        // Use NON_BLOCKING for both; required for Extended Thinking.
        json!({"name": name, "description": description, "parameters": parameters, "behavior": "NON_BLOCKING"})
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    NeedsAttention,
    Routine,
    Silent,
}

impl Delivery {
    fn scheduling(self) -> &'static str {
        match self {
            Self::NeedsAttention => "INTERRUPT",
            Self::Routine => "WHEN_IDLE",
            Self::Silent => "SILENT",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelActivity {
    Unknown,
    InProgress,
    Idle,
}

#[derive(Debug)]
pub struct LiveActivity {
    model: LiveModel,
    pub model_activity: ModelActivity,
    pub audio_playing: bool,
    pub outstanding_tasks: usize,
    pub awaiting_approval: bool,
}

impl LiveActivity {
    pub fn new(model: LiveModel) -> Self {
        Self {
            model,
            model_activity: ModelActivity::Unknown,
            audio_playing: false,
            outstanding_tasks: 0,
            awaiting_approval: false,
        }
    }

    pub fn user_turn_started(&mut self) {
        self.model_activity = ModelActivity::InProgress;
    }

    pub fn disconnected(&mut self) {
        self.model_activity = ModelActivity::Unknown;
        // No assertion that app-owned tasks have stopped or been cancelled.
    }

    pub fn observe(&mut self, message: &Value) {
        let content = message.get("serverContent");
        let values: Vec<&Value> = [
            message.get("interactionStatus"),
            message.get("interaction_status"),
            content.and_then(|c| c.get("interactionStatus")),
            content.and_then(|c| c.get("interaction_status")),
        ]
        .into_iter()
        .flatten()
        .collect();
        if let Some(first) = values.first() {
            self.model_activity = if values.iter().any(|value| value != first) {
                ModelActivity::Unknown
            } else {
                match first.as_str() {
                    Some("IN_PROGRESS") => ModelActivity::InProgress,
                    Some("IDLE") => ModelActivity::Idle,
                    _ => ModelActivity::Unknown,
                }
            };
        } else if content
            .and_then(|c| c.get("turnComplete"))
            .and_then(Value::as_bool)
            == Some(true)
            && self.model == LiveModel::Standard
        {
            self.model_activity = ModelActivity::Idle;
        }
    }

    pub fn ready(&self) -> bool {
        self.model_activity == ModelActivity::Idle
            && !self.audio_playing
            && self.outstanding_tasks == 0
            && !self.awaiting_approval
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_utterance_completion_does_not_mean_idle() {
        let mut activity = LiveActivity::new(LiveModel::ExtendedThinking);
        activity.user_turn_started();
        activity.observe(&json!({"serverContent": {"turnComplete": true}}));
        assert!(!activity.ready());
        activity.observe(&json!({"interactionStatus": "IDLE"}));
        assert!(activity.ready());
    }

    #[test]
    fn nested_status_only_and_top_level_status_work() {
        let mut activity = LiveActivity::new(LiveModel::ExtendedThinking);
        activity.observe(
            &json!({"serverContent": {"interactionStatus": "IN_PROGRESS", "turnComplete": true}}),
        );
        assert!(!activity.ready());
        activity.observe(&json!({"interaction_status": "IDLE"}));
        assert!(activity.ready());
    }

    #[test]
    fn unknown_or_conflicting_status_is_not_idle() {
        let mut activity = LiveActivity::new(LiveModel::ExtendedThinking);
        activity.observe(&json!({"interactionStatus": "FUTURE_STATE"}));
        assert!(!activity.ready());
        activity.observe(&json!({"interactionStatus": "IDLE", "serverContent": {"interactionStatus": "IN_PROGRESS"}}));
        assert!(!activity.ready());
    }

    #[test]
    fn standard_turn_completion_is_supported() {
        let mut activity = LiveActivity::new(LiveModel::Standard);
        activity.observe(&json!({"serverContent": {"turnComplete": true}}));
        assert!(activity.ready());
    }

    #[test]
    fn audio_tasks_and_approval_are_independent_of_model_idle() {
        let mut activity = LiveActivity::new(LiveModel::Standard);
        activity.observe(&json!({"serverContent": {"turnComplete": true}}));
        activity.audio_playing = true;
        assert!(!activity.ready());
        activity.audio_playing = false;
        activity.outstanding_tasks = 1;
        assert!(!activity.ready());
        activity.outstanding_tasks = 0;
        activity.awaiting_approval = true;
        assert!(!activity.ready());
        activity.awaiting_approval = false;
        assert!(activity.ready());
    }

    #[test]
    fn disconnection_does_not_complete_app_work() {
        let mut activity = LiveActivity::new(LiveModel::Standard);
        activity.outstanding_tasks = 1;
        activity.disconnected();
        assert!(!activity.ready());
        assert_eq!(activity.outstanding_tasks, 1);
    }

    #[test]
    fn extended_tools_omit_unsupported_scheduling() {
        let response = LiveModel::ExtendedThinking.tool_response(
            "id",
            "tool",
            json!({"ok": true}),
            Delivery::Routine,
        );
        assert!(response["toolResponse"]["functionResponses"][0]
            .get("scheduling")
            .is_none());
        let declaration = LiveModel::ExtendedThinking.function_declaration(
            "tool",
            "Test",
            json!({"type": "OBJECT"}),
        );
        assert_eq!(declaration["behavior"], "NON_BLOCKING");
    }

    #[test]
    fn standard_delivery_and_unknown_model_are_explicit() {
        for (delivery, expected) in [
            (Delivery::NeedsAttention, "INTERRUPT"),
            (Delivery::Routine, "WHEN_IDLE"),
            (Delivery::Silent, "SILENT"),
        ] {
            let response = LiveModel::Standard.tool_response("id", "tool", json!({}), delivery);
            assert_eq!(
                response["toolResponse"]["functionResponses"][0]["response"]["scheduling"],
                expected
            );
        }
        assert!(LiveModel::from_id("unrecognized-future-model").is_none());
        assert_eq!(
            LiveModel::from_id("models/gemini-3.8-live"),
            Some(LiveModel::Standard)
        );
    }
}
