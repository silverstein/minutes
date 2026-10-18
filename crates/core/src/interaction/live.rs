//! Provider capabilities and independent speaking/working state.
//!
//! Gemini contract verified 2026-09-15:
//! https://ai.google.dev/gemini-api/docs/models/gemini-3.8-live-extended-thinking
//! https://ai.google.dev/gemini-api/docs/live-api/tools

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::{text, Result, MAX_ITEMS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveProfile {
    Standard,
    AsyncReasoning,
}

impl LiveProfile {
    /// Unknown models require an explicit adapter, rather than silently
    /// inheriting wire assumptions from a name substring.
    pub fn gemini(model: &str) -> Result<Self> {
        match model.strip_prefix("models/").unwrap_or(model) {
            "gemini-3.8-live" => Ok(Self::Standard),
            "gemini-3.8-live-extended-thinking" => Ok(Self::AsyncReasoning),
            _ => Err("unknown Live model capability profile".into()),
        }
    }

    pub fn function_declaration(self, mut declaration: Value) -> Result<Value> {
        let object = declaration.as_object_mut().ok_or("invalid declaration")?;
        if self == Self::AsyncReasoning {
            object.insert("behavior".into(), json!("NON_BLOCKING"));
        }
        Ok(declaration)
    }

    pub fn function_response(
        self,
        id: &str,
        name: &str,
        result: Value,
        scheduling: &str,
    ) -> Result<Value> {
        text(id)?;
        text(name)?;
        let mut response = json!({ "result": result });
        if self == Self::Standard {
            if !matches!(scheduling, "WHEN_IDLE" | "INTERRUPT" | "SILENT") {
                return Err("invalid tool scheduling".into());
            }
            response["scheduling"] = json!(scheduling);
        }
        Ok(json!({ "id": id, "name": name, "response": response }))
    }

    pub fn proactive_audio(self, requested: bool) -> bool {
        self == Self::AsyncReasoning || requested
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderActivity {
    Unknown,
    InProgress,
    Idle,
}

/// Accept both SDK snake_case and JSON camelCase. Unknown values remain unknown.
pub fn interaction_status(frame: &Value) -> Option<ProviderActivity> {
    let value = frame
        .get("interactionStatus")
        .or_else(|| frame.get("interaction_status"))
        .or_else(|| frame.pointer("/serverContent/interactionStatus"))
        .or_else(|| frame.pointer("/server_content/interaction_status"))?;
    Some(match value.as_str() {
        Some("IN_PROGRESS") => ProviderActivity::InProgress,
        Some("IDLE") => ProviderActivity::Idle,
        _ => ProviderActivity::Unknown,
    })
}

#[derive(Debug)]
pub struct LiveActivity {
    profile: LiveProfile,
    provider: ProviderActivity,
    playing: bool,
    listening: bool,
    calls: BTreeSet<String>,
}

impl LiveActivity {
    pub fn new(profile: LiveProfile) -> Self {
        Self {
            profile,
            provider: ProviderActivity::Unknown,
            playing: false,
            listening: false,
            calls: BTreeSet::new(),
        }
    }

    pub fn provider_status(&mut self, status: ProviderActivity) {
        self.provider = status;
    }

    pub fn user_input_started(&mut self) {
        self.listening = true;
        self.provider = ProviderActivity::InProgress;
    }

    pub fn user_input_finished(&mut self) {
        self.listening = false;
    }

    pub fn playback_started(&mut self) {
        self.playing = true;
    }

    /// Flush/mute does not cancel work and does not make the provider idle.
    pub fn playback_stopped(&mut self) {
        self.playing = false;
    }

    pub fn tool_started(&mut self, id: &str) -> Result<()> {
        text(id)?;
        if self.calls.len() >= MAX_ITEMS || !self.calls.insert(id.to_string()) {
            return Err("duplicate call or tool budget exceeded".into());
        }
        self.provider = ProviderActivity::InProgress;
        Ok(())
    }

    pub fn tool_finished(&mut self, id: &str) -> Result<()> {
        if !self.calls.remove(id) {
            return Err("unknown tool completion".into());
        }
        Ok(())
    }

    pub fn turn_complete(&mut self) {
        if self.profile == LiveProfile::Standard {
            self.provider = ProviderActivity::Idle;
        }
    }

    /// Call on reconnect before replaying any status. Work is not reset here.
    pub fn disconnected(&mut self) {
        self.provider = ProviderActivity::Unknown;
        self.playing = false;
        self.listening = false;
    }

    pub fn ready(&self) -> bool {
        self.provider == ProviderActivity::Idle
            && !self.playing
            && !self.listening
            && self.calls.is_empty()
    }

    pub fn pending_tools(&self) -> usize {
        self.calls.len()
    }
}
