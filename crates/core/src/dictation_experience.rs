//! Local, bounded dictation preferences. No window scraping or implicit editing.
use crate::dictation_context::DictationTextMode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DictationExperience {
    pub context_enabled: bool,
    pub writing_style: String,
    pub target_rules: Vec<TargetRule>,
    /// Explicit user-authorized spelling substitutions, never inferred from edits.
    pub dictionary: BTreeMap<String, String>,
    pub microphone_mode: String,
    pub microphones: Vec<String>,
    pub include_virtual_microphones: bool,
    pub clipboard_restore_delay_ms: u64,
    pub paste_last_enabled: bool,
    pub paste_last_shortcut: String,
    pub history_shortcut_enabled: bool,
    pub history_shortcut: String,
}
impl Default for DictationExperience {
    fn default() -> Self {
        Self {
            context_enabled: false,
            writing_style: "natural".into(),
            target_rules: Vec::new(),
            dictionary: BTreeMap::new(),
            microphone_mode: "shared".into(),
            microphones: Vec::new(),
            include_virtual_microphones: false,
            clipboard_restore_delay_ms: 500,
            paste_last_enabled: false,
            paste_last_shortcut: "CmdOrCtrl+Alt+V".into(),
            history_shortcut_enabled: false,
            history_shortcut: "CmdOrCtrl+Alt+D".into(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetRule {
    /// `app:<bundle ID or exact app name>` or `site:<exact hostname>`.
    pub target: String,
    pub mode: String,
    pub style: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CursorContext {
    pub before: String,
    pub selected: String,
    pub after: String,
}
impl DictationExperience {
    pub fn validate(&self) -> Result<(), String> {
        if !["natural", "casual", "literal"].contains(&self.writing_style.as_str()) {
            return Err("Choose Natural, Casual, or Literal writing.".into());
        }
        if !["shared", "system", "preferred"].contains(&self.microphone_mode.as_str()) {
            return Err("Choose a recording, system, or preferred microphone.".into());
        }
        if self.target_rules.len() > 64
            || self.dictionary.len() > 128
            || self.microphones.len() > 16
        {
            return Err(
                "Use at most 64 app preferences, 128 dictionary entries, and 16 microphones."
                    .into(),
            );
        }
        if !(250..=5000).contains(&self.clipboard_restore_delay_ms) {
            return Err(
                "Clipboard restoration must wait between 250 and 5,000 milliseconds.".into(),
            );
        }
        let mut targets = BTreeSet::new();
        for rule in &self.target_rules {
            let (kind, value) = rule
                .target
                .split_once(':')
                .ok_or("Use app: followed by an app name, or site: followed by a hostname.")?;
            if !["app", "site"].contains(&kind)
                || value.is_empty()
                || value.len() > 160
                || value.contains(['\n', '\r', '\0'])
                || value.trim() != value
                || (kind == "site"
                    && (!value.contains('.')
                        || !value
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')))
            {
                return Err("An app preference needs an exact app name or a website hostname, without a URL path.".into());
            }
            if !targets.insert(rule.target.to_lowercase()) {
                return Err("Each app or website can have only one writing preference.".into());
            }
            if mode_from_str(&rule.mode).is_none()
                || !["natural", "casual", "literal"].contains(&rule.style.as_str())
            {
                return Err("Choose a supported writing mode and style.".into());
            }
        }
        let mut aliases = BTreeSet::new();
        for (spoken, spelling) in &self.dictionary {
            for value in [spoken, spelling] {
                if value.trim().is_empty()
                    || value.trim() != value
                    || value.chars().count() > 80
                    || value.contains(['\n', '\r', '\0'])
                    || value.split_whitespace().count() > 8
                {
                    return Err(
                        "Dictionary entries must be short words or phrases (up to 80 characters)."
                            .into(),
                    );
                }
            }
            if !aliases.insert(spoken.to_lowercase()) {
                return Err("Each spoken phrase can have only one dictionary spelling.".into());
            }
        }
        if self.microphones.iter().any(|name| {
            name.trim().is_empty() || name.len() > 256 || name.contains(['\0', '\n', '\r'])
        }) {
            return Err("Choose a microphone from the device list.".into());
        }
        Ok(())
    }
    /// Site preferences take priority over app preferences; no partial app matches.
    pub fn resolve(
        &self,
        app: Option<&str>,
        bundle: Option<&str>,
        site: Option<&str>,
        fallback: DictationTextMode,
    ) -> (DictationTextMode, &str) {
        let matches = |rule: &&TargetRule, kind: &str, values: &[Option<&str>]| {
            rule.target.split_once(':').is_some_and(|(prefix, value)| {
                prefix == kind
                    && values
                        .iter()
                        .flatten()
                        .any(|candidate| value.eq_ignore_ascii_case(candidate))
            })
        };
        let rule = self
            .target_rules
            .iter()
            .find(|r| matches(r, "site", &[site]))
            .or_else(|| {
                self.target_rules
                    .iter()
                    .find(|r| matches(r, "app", &[bundle, app]))
            });
        match rule {
            Some(rule) => (
                mode_from_str(&rule.mode).unwrap_or(fallback),
                rule.style.as_str(),
            ),
            None => (fallback, &self.writing_style),
        }
    }
    /// A bounded hint list for engines that accept contextual vocabulary.
    pub fn recognition_hint(&self) -> String {
        let mut values = BTreeSet::new();
        let mut out = String::new();
        for value in self.dictionary.values() {
            if values.insert(value.to_lowercase()) {
                if out.len() + value.len() + 2 > 768 {
                    break;
                }
                if !out.is_empty() {
                    out.push_str(", ");
                }
                out.push_str(value);
            }
        }
        out
    }
}
pub fn mode_from_str(mode: &str) -> Option<DictationTextMode> {
    match mode {
        "agent_prompt" => Some(DictationTextMode::AgentPrompt),
        "terminal_code" => Some(DictationTextMode::TerminalCode),
        "chat" => Some(DictationTextMode::Chat),
        "email_document" => Some(DictationTextMode::EmailDocument),
        "unknown" => Some(DictationTextMode::Unknown),
        _ => None,
    }
}
/// Cursor-boundary formatting only. It cannot rewrite the user's sentence.
pub fn format_for_cursor(
    text: &str,
    context: Option<&CursorContext>,
    style: &str,
    mode: DictationTextMode,
    dictionary: &BTreeMap<String, String>,
) -> String {
    if style == "literal" || mode == DictationTextMode::TerminalCode {
        return text.to_string();
    }
    let mut out = text.to_string();
    if style == "casual"
        && out.chars().count() <= 200
        && out.ends_with('.')
        && !out.ends_with("..")
        && !out[..out.len() - 1].contains(['.', '!', '?', '\n'])
    {
        out.pop();
    }
    let Some(context) = context else {
        return out;
    };
    if context.before.chars().count() > 256
        || context.after.chars().count() > 256
        || context.selected.chars().count() > 256
    {
        return out;
    }
    let before = context.before.chars().last();
    let previous = context.before.trim_end().chars().last();
    let first_word = out
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric());
    let protected = first_word == "I"
        || first_word.chars().filter(|c| c.is_uppercase()).count() > 1
        || dictionary.values().any(|v| {
            v.split_whitespace()
                .next()
                .is_some_and(|w| w.eq_ignore_ascii_case(first_word))
        });
    if [
        "A", "An", "The", "This", "That", "These", "Those", "Can", "Could", "Would", "Please",
        "Do", "Does", "Did", "Send", "Make", "Keep", "Add", "Remove", "Let", "We", "You", "It",
        "They",
    ]
    .contains(&first_word)
        && previous.is_some_and(|c| !matches!(c, '.' | '!' | '?' | '\n' | ':'))
        && !context.before.ends_with('\n')
        && !protected
        && out.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
    {
        out.replace_range(..1, &out[..1].to_ascii_lowercase());
    }
    // Suppress only identical punctuation at the exact insertion boundary.
    if let (Some(last), Some(next)) = (out.chars().last(), context.after.chars().next()) {
        if last == next && matches!(last, '.' | ',' | '!' | '?' | ';' | ':') {
            out.pop();
        }
    }
    if !out.is_empty()
        && before.is_some_and(|c| !c.is_whitespace() && !matches!(c, '(' | '[' | '{' | '“'))
        && out.chars().next().is_some_and(|c| {
            !c.is_whitespace() && !matches!(c, '.' | ',' | '!' | '?' | ';' | ':' | ')' | ']')
        })
    {
        out.insert(0, ' ');
    }
    if !out.is_empty()
        && context
            .after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric())
        && !out.ends_with(char::is_whitespace)
    {
        out.push(' ');
    }
    out
}
pub fn is_virtual_microphone(name: &str) -> bool {
    let name = name.to_lowercase();
    [
        "blackhole",
        "loopback",
        "soundflower",
        "aggregate",
        "virtual",
        "vb-cable",
        "multi-output",
    ]
    .iter()
    .any(|s| name.contains(s))
}
pub fn microphone_candidates(
    prefs: &DictationExperience,
    available: &[String],
    shared: Option<&str>,
    lid_closed: bool,
) -> Vec<Option<String>> {
    if prefs.microphone_mode == "shared" {
        return vec![shared.map(str::to_string)];
    }
    let allowed = |name: &str| {
        (prefs.include_virtual_microphones || !is_virtual_microphone(name))
            && !(lid_closed
                && (name.to_lowercase().contains("built-in")
                    || name.to_lowercase().contains("macbook")))
    };
    let mut candidates = Vec::new();
    if prefs.microphone_mode == "preferred" {
        for name in &prefs.microphones {
            if let Some(found) = available
                .iter()
                .find(|v| v.eq_ignore_ascii_case(name) && allowed(v))
            {
                if !candidates.contains(&Some(found.clone())) {
                    candidates.push(Some(found.clone()));
                }
            }
        }
    }
    if prefs.microphone_mode == "preferred" || lid_closed {
        for name in available.iter().filter(|v| allowed(v)) {
            if !candidates.contains(&Some(name.clone())) {
                candidates.push(Some(name.clone()));
            }
        }
    }
    // Explicit system selection follows the OS. Preferred mode never falls back
    // to an excluded virtual/built-in device through the system default.
    if prefs.microphone_mode == "system" && !lid_closed {
        candidates.push(None);
    }
    candidates
}
/// Proposes a spelling candidate only from an explicit edit inside Minutes.
/// The caller must still show both phrases and require confirmation before saving.
pub fn correction_candidate(original: &str, corrected: &str) -> Option<(String, String)> {
    if original.len() > 8192 || corrected.len() > 8192 {
        return None;
    }
    let before: Vec<_> = original.split_whitespace().collect();
    let after: Vec<_> = corrected.split_whitespace().collect();
    if before.len() != after.len() {
        return None;
    }
    let differences: Vec<_> = before.iter().zip(&after).filter(|(a, b)| a != b).collect();
    if differences.len() != 1 {
        return None;
    }
    let (before, after) = differences[0];
    let clean = |value: &str| {
        value
            .trim_matches(|c: char| {
                matches!(c, '.' | ',' | '!' | '?' | ';' | ':' | '"' | '“' | '”')
            })
            .to_string()
    };
    let spoken = clean(before);
    let written = clean(after);
    if spoken.is_empty()
        || written.is_empty()
        || spoken == written
        || spoken.len() > 80
        || written.len() > 80
        || !spoken.chars().all(|c| c.is_alphabetic() || c == '-')
        || !written.chars().all(|c| c.is_alphabetic() || c == '-')
    {
        return None;
    }
    let prohibited = [
        "not", "no", "never", "yes", "do", "dont", "don't", "send", "delete", "remove", "keep",
    ];
    if prohibited.contains(&spoken.to_lowercase().as_str())
        || prohibited.contains(&written.to_lowercase().as_str())
    {
        return None;
    }
    // Small spelling changes only. Semantic rewrites and multiword edits are not candidates.
    let a: Vec<_> = spoken.to_lowercase().chars().collect();
    let b: Vec<_> = written.to_lowercase().chars().collect();
    let mut distance: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut diagonal = distance[0];
        distance[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let above = distance[j + 1];
            distance[j + 1] = (distance[j] + 1)
                .min(above + 1)
                .min(diagonal + usize::from(x != y));
            diagonal = above;
        }
    }
    let edits = distance[b.len()];
    if edits > 2 || (edits > 0 && edits * 3 > a.len().max(b.len())) {
        return None;
    }
    Some((spoken, written))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corrections_propose_spelling_but_abstain_from_meaning_changes() {
        for (before, after, expected) in [
            (
                "Use minutes today.",
                "Use Minutes today.",
                Some(("minutes", "Minutes")),
            ),
            (
                "Ask Elodie first.",
                "Ask Élodie first.",
                Some(("Elodie", "Élodie")),
            ),
            (
                "Open Cleanshot now",
                "Open CleanShot now",
                Some(("Cleanshot", "CleanShot")),
            ),
            ("Send it now", "Delete it now", None),
            ("I do not agree", "I do agree", None),
            ("The cost is 120", "The cost is 1200", None),
            ("Use src/main.rs", "Use src/lib.rs", None),
            ("Mat wrote this", "Mat rewrote this yesterday", None),
            ("Use minutes today.", "Use minutes today!", None),
        ] {
            assert_eq!(
                correction_candidate(before, after),
                expected.map(|(a, b)| (a.into(), b.into())),
                "{before} => {after}"
            );
        }
    }
    #[test]
    fn joining_abstains_from_unknown_proper_names_and_unbounded_context() {
        let context = CursorContext {
            before: "ask".into(),
            after: " tomorrow".into(),
            ..Default::default()
        };
        for name in ["Mat", "Élodie", "Wispr", "OpenAI", "X1", "September"] {
            let output = format_for_cursor(
                name,
                Some(&context),
                "natural",
                DictationTextMode::EmailDocument,
                &BTreeMap::new(),
            );
            assert_eq!(output, format!(" {name}"));
        }
        let context = CursorContext {
            before: "x".repeat(257),
            ..Default::default()
        };
        assert_eq!(
            format_for_cursor(
                "Do not send it.",
                Some(&context),
                "natural",
                DictationTextMode::Chat,
                &BTreeMap::new()
            ),
            "Do not send it."
        );
    }
    #[test]
    fn literal_preserves_adversarial_text_in_every_target() {
        let inputs = [
            "Do not send $120.50.",
            "No, keep the original.",
            "git push --force",
            "src/main.rs",
            "Actually, leave it alone.",
            "Élodie’s number is +33 1 23 45 67 89.",
            "rm -rf /tmp/example",
            "hello\nworld",
            "120.00 != 12.00",
            "I am not consenting.",
        ];
        let c = CursorContext {
            before: "prefix".into(),
            after: "suffix".into(),
            selected: "selection".into(),
        };
        for mode in [
            DictationTextMode::Chat,
            DictationTextMode::AgentPrompt,
            DictationTextMode::EmailDocument,
            DictationTextMode::TerminalCode,
            DictationTextMode::Unknown,
        ] {
            for input in inputs {
                assert_eq!(
                    format_for_cursor(input, Some(&c), "literal", mode, &BTreeMap::new()),
                    input
                );
            }
        }
    }
    #[test]
    fn new_preferences_read_old_configs_and_bound_delivery_wait() {
        let mut prefs: DictationExperience = serde_json::from_str("{}").unwrap();
        assert!(
            !prefs.context_enabled && !prefs.paste_last_enabled && !prefs.history_shortcut_enabled
        );
        assert_eq!(prefs.clipboard_restore_delay_ms, 500);
        for delay in [0, 249, 5001, u64::MAX] {
            prefs.clipboard_restore_delay_ms = delay;
            assert!(prefs.validate().is_err());
        }
        for delay in [250, 500, 1000, 5000] {
            prefs.clipboard_restore_delay_ms = delay;
            assert!(prefs.validate().is_ok());
        }
    }
    #[test]
    fn joins_sentence_preserves_names_and_replaces_selection() {
        let c = CursorContext {
            before: "can you".into(),
            after: " tomorrow".into(),
            ..Default::default()
        };
        let names = BTreeMap::from([("minutes".into(), "Minutes".into())]);
        assert_eq!(
            format_for_cursor(
                "Will can join.",
                Some(&c),
                "natural",
                DictationTextMode::Chat,
                &BTreeMap::new()
            ),
            " Will can join."
        );
        assert_eq!(
            format_for_cursor(
                "Send the file.",
                Some(&c),
                "natural",
                DictationTextMode::AgentPrompt,
                &names
            ),
            " send the file."
        );
        assert_eq!(
            format_for_cursor(
                "Minutes",
                Some(&c),
                "natural",
                DictationTextMode::AgentPrompt,
                &names
            ),
            " Minutes"
        );
        let c = CursorContext {
            before: "Hello ".into(),
            selected: "world".into(),
            after: ".".into(),
        };
        assert_eq!(
            format_for_cursor(
                "Mat.",
                Some(&c),
                "natural",
                DictationTextMode::Chat,
                &BTreeMap::from([("mat".into(), "Mat".into())])
            ),
            "Mat"
        );
    }
    #[test]
    fn preserves_literal_numbers_negation_and_unicode() {
        let c = CursorContext {
            before: "run".into(),
            after: "next".into(),
            ..Default::default()
        };
        assert_eq!(
            format_for_cursor(
                "git status",
                Some(&c),
                "natural",
                DictationTextMode::TerminalCode,
                &BTreeMap::new()
            ),
            "git status"
        );
        assert_eq!(
            format_for_cursor(
                "Do not send 120.50!",
                Some(&c),
                "natural",
                DictationTextMode::Chat,
                &BTreeMap::new()
            ),
            " do not send 120.50! "
        );
        assert_eq!(
            format_for_cursor(
                "Élodie",
                None,
                "natural",
                DictationTextMode::Chat,
                &BTreeMap::new()
            ),
            "Élodie"
        );
        assert_eq!(
            format_for_cursor(
                "Is that right?",
                None,
                "casual",
                DictationTextMode::Chat,
                &BTreeMap::new()
            ),
            "Is that right?"
        );
        assert_eq!(
            format_for_cursor(
                "index.ts",
                None,
                "casual",
                DictationTextMode::Chat,
                &BTreeMap::new()
            ),
            "index.ts"
        );
    }
    #[test]
    fn site_override_beats_app_and_terminal_prose_is_explicit() {
        let mut p = DictationExperience::default();
        p.target_rules = vec![
            TargetRule {
                target: "app:Ghostty".into(),
                mode: "agent_prompt".into(),
                style: "natural".into(),
            },
            TargetRule {
                target: "site:mail.google.com".into(),
                mode: "email_document".into(),
                style: "natural".into(),
            },
        ];
        assert!(p.validate().is_ok());
        assert_eq!(
            p.resolve(Some("Ghostty"), None, None, DictationTextMode::TerminalCode)
                .0,
            DictationTextMode::AgentPrompt
        );
        assert_eq!(
            p.resolve(
                Some("Safari"),
                None,
                Some("mail.google.com"),
                DictationTextMode::Unknown
            )
            .0,
            DictationTextMode::EmailDocument
        );
        assert_eq!(
            p.resolve(
                Some("Ghostty clone"),
                None,
                None,
                DictationTextMode::Unknown
            )
            .0,
            DictationTextMode::Unknown
        );
    }
    #[test]
    fn invalid_rules_and_conflicting_spelling_are_rejected() {
        let mut p = DictationExperience::default();
        p.dictionary =
            BTreeMap::from([("mat".into(), "Mat".into()), ("MAT".into(), "Matt".into())]);
        assert!(p.validate().is_err());
        p.dictionary.clear();
        p.target_rules.push(TargetRule {
            target: "site:https://gmail.com/private".into(),
            mode: "chat".into(),
            style: "natural".into(),
        });
        assert!(p.validate().is_err());
    }
    #[test]
    fn missing_preferred_mic_avoids_virtual_and_closed_lid_inputs() {
        let mut p = DictationExperience::default();
        p.microphone_mode = "preferred".into();
        p.microphones = vec!["Disconnected USB".into()];
        let available = vec![
            "BlackHole 2ch".into(),
            "MacBook Pro Microphone".into(),
            "USB Mic".into(),
        ];
        assert_eq!(
            microphone_candidates(&p, &available, None, true),
            vec![Some("USB Mic".into())]
        );
        assert!(microphone_candidates(&p, &available[..2], None, true).is_empty());
        p.microphone_mode = "shared".into();
        assert_eq!(
            microphone_candidates(&p, &available, Some("BlackHole 2ch"), true),
            vec![Some("BlackHole 2ch".into())]
        );
    }
}

/// No transcript, URL, or window contents are read by this hardware query.
pub fn laptop_lid_closed() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::engine_process::command("ioreg")
            .args(["-r", "-k", "AppleClamshellState", "-d", "4"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .is_some_and(|out| {
                String::from_utf8_lossy(&out.stdout).lines().any(|line| {
                    line.contains("AppleClamshellState") && line.trim_end().ends_with("Yes")
                })
            })
    }
    #[cfg(not(target_os = "macos"))]
    false
}
