//! Sparse, local target hints for deterministic dictation formatting.
//!
//! The classifier consumes only the focused app identity already captured for
//! insertion. It never reads surrounding text, window contents, or network
//! state. An uncertain target is `Unknown`, which keeps the conservative prose
//! cleanup rather than guessing at a richer mode.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictationTextMode {
    TerminalCode,
    AgentPrompt,
    Chat,
    EmailDocument,
    #[default]
    Unknown,
}

impl DictationTextMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TerminalCode => "terminal_code",
            Self::AgentPrompt => "agent_prompt",
            Self::Chat => "chat",
            Self::EmailDocument => "email_document",
            Self::Unknown => "unknown",
        }
    }
}

pub fn infer_target_text_mode(
    app_name: Option<&str>,
    bundle_id: Option<&str>,
    minutes_bundle_id: Option<&str>,
) -> DictationTextMode {
    let app = app_name.unwrap_or_default().to_ascii_lowercase();
    let bundle = bundle_id.unwrap_or_default().to_ascii_lowercase();
    let identity = format!("{app} {bundle}");

    if minutes_bundle_id.is_some_and(|minutes| bundle_id == Some(minutes)) {
        return DictationTextMode::AgentPrompt;
    }
    if matches!(app.as_str(), "minutes" | "minutes dev") {
        return DictationTextMode::AgentPrompt;
    }
    if contains_any(
        &identity,
        &[
            "ghostty",
            "com.apple.terminal",
            "iterm",
            "warp",
            "alacritty",
            "kitty",
            "wezterm",
            "hyper",
            "herdr",
        ],
    ) {
        return DictationTextMode::TerminalCode;
    }
    if contains_any(
        &identity,
        &[
            "chatgpt",
            "claude",
            "com.openai",
            "com.anthropic",
            "cursor",
            "windsurf",
        ],
    ) {
        return DictationTextMode::AgentPrompt;
    }
    if contains_any(
        &identity,
        &[
            "slack",
            "discord",
            "messages",
            "msteams",
            "microsoft teams",
            "signal",
            "whatsapp",
            "telegram",
        ],
    ) {
        return DictationTextMode::Chat;
    }
    if contains_any(
        &identity,
        &[
            "com.apple.mail",
            "outlook",
            "textedit",
            "pages",
            "microsoft word",
            "com.microsoft.word",
            "notion",
            "obsidian",
            "bear",
            "notes",
        ],
    ) {
        return DictationTextMode::EmailDocument;
    }
    DictationTextMode::Unknown
}

fn contains_any(identity: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| identity.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn herdr_is_terminal_code() {
        assert_eq!(
            infer_target_text_mode(Some("Herdr"), Some("dev.herdr.app"), None),
            DictationTextMode::TerminalCode
        );
    }

    #[test]
    fn ghostty_is_terminal_code() {
        assert_eq!(
            infer_target_text_mode(Some("Ghostty"), Some("com.mitchellh.ghostty"), None),
            DictationTextMode::TerminalCode
        );
    }

    #[test]
    fn chat_and_document_apps_are_sparse_and_explicit() {
        assert_eq!(
            infer_target_text_mode(Some("Slack"), Some("com.tinyspeck.slackmacgap"), None),
            DictationTextMode::Chat
        );
        assert_eq!(
            infer_target_text_mode(Some("TextEdit"), Some("com.apple.TextEdit"), None),
            DictationTextMode::EmailDocument
        );
        assert_eq!(
            infer_target_text_mode(Some("Safari"), Some("com.apple.Safari"), None),
            DictationTextMode::Unknown,
            "a browser alone does not reveal whether the field is chat, email, code, or prose"
        );
    }

    #[test]
    fn minutes_input_is_an_agent_prompt() {
        assert_eq!(
            infer_target_text_mode(
                Some("Minutes Dev"),
                Some("com.useminutes.desktop.dev"),
                Some("com.useminutes.desktop.dev")
            ),
            DictationTextMode::AgentPrompt
        );
        assert_eq!(
            infer_target_text_mode(Some("Minutes Dev"), None, None),
            DictationTextMode::AgentPrompt,
            "persisted recovery records retain the app name but not the bundle id"
        );
    }

    #[test]
    fn missing_or_unrecognized_identity_fails_closed() {
        assert_eq!(
            infer_target_text_mode(None, None, Some("com.useminutes.desktop.dev")),
            DictationTextMode::Unknown
        );
        assert_eq!(
            infer_target_text_mode(Some("Private App"), Some("example.private"), None),
            DictationTextMode::Unknown
        );
    }
}
