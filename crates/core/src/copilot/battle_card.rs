use crate::config::Config;
use crate::context_card::{ContextCard, ContextCardRequest};
use serde::{Deserialize, Serialize};

const BATTLE_CARD_CHAR_BUDGET: usize = crate::context_card::DEFAULT_CONTEXT_CARD_CHAR_BUDGET;

/// Bounded historical context refreshed asynchronously for the fast lane.
///
/// Every repository query uses the default restricted-history exclusion. The
/// rendered field is the model-facing representation and is capped to roughly
/// 1,000–2,000 tokens using a conservative character budget.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BattleCard {
    pub people: Vec<String>,
    pub open_commitments: Vec<String>,
    pub decisions: Vec<String>,
    pub intents: Vec<String>,
    pub fts_excerpts: Vec<String>,
    pub rendered: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BattleCardError {
    #[error("battle-card sources were unavailable: {0}")]
    SourcesUnavailable(String),
}

/// Retrieval seam owned by the slow lane. Implementations may touch graph,
/// FTS, or optional QMD, so callers must never invoke this from capture or the
/// fast nudge path.
pub trait GroundingSource: Send + Sync + 'static {
    fn refresh(&self, query: &str) -> Result<BattleCard, BattleCardError>;
}

#[derive(Debug, Clone)]
pub struct RepositoryGrounding {
    config: Config,
}

impl RepositoryGrounding {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl GroundingSource for RepositoryGrounding {
    fn refresh(&self, query: &str) -> Result<BattleCard, BattleCardError> {
        BattleCard::assemble(&self.config, query)
    }
}

impl BattleCard {
    pub fn empty() -> Self {
        Self {
            rendered: "No historical context was loaded.".into(),
            ..Self::default()
        }
    }

    pub fn assemble(config: &Config, query: &str) -> Result<Self, BattleCardError> {
        let mut request = ContextCardRequest::new(query);
        request.max_chars = BATTLE_CARD_CHAR_BUDGET;
        let context = ContextCard::assemble(config, request)
            .map_err(|error| BattleCardError::SourcesUnavailable(error.to_string()))?;
        let mut card = Self::default();
        for evidence in context.evidence() {
            match evidence.context_class.as_str() {
                "relationship" => card.people.push(evidence.text.clone()),
                "commitment" => card.open_commitments.push(evidence.text.clone()),
                "decision" => card.decisions.push(evidence.text.clone()),
                "open_intent" => card.intents.push(evidence.text.clone()),
                "meeting_excerpt" | "related_meeting" | "prior_meeting" => {
                    card.fts_excerpts.push(evidence.text.clone());
                }
                _ => {}
            }
        }
        dedupe(&mut card.people);
        dedupe(&mut card.open_commitments);
        dedupe(&mut card.decisions);
        dedupe(&mut card.intents);
        dedupe(&mut card.fts_excerpts);
        card.people.truncate(8);
        card.open_commitments.truncate(10);
        card.decisions.truncate(8);
        card.intents.truncate(10);
        card.fts_excerpts.truncate(6);
        card.rendered = context.rendered().to_string();
        if card.rendered.is_empty() {
            card.rendered = "No relevant unrestricted historical context was found.".into();
        }
        Ok(card)
    }
}

fn dedupe(values: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    values.retain(|value| seen.insert(value.to_ascii_lowercase()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::{
        CopilotRequest, CopilotUtterance, MeetingMode, StrategyRefreshReason, StrategyRequest,
        StrategyState, TranscriptUpdateKind,
    };
    use std::ffi::OsString;
    use std::path::Path;

    struct HomeOverride {
        previous: Option<OsString>,
    }

    impl HomeOverride {
        fn set(path: &Path) -> Self {
            let previous = std::env::var_os("HOME");
            std::env::set_var("HOME", path);
            Self { previous }
        }
    }

    impl Drop for HomeOverride {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var("HOME", previous);
            } else {
                std::env::remove_var("HOME");
            }
        }
    }

    fn meeting(title: &str, person: &str, secret: &str, restricted: bool) -> String {
        let sensitivity = if restricted {
            "sensitivity: restricted\n"
        } else {
            ""
        };
        format!(
            "---\ntitle: {title}\ntype: meeting\ndate: 2026-06-11T12:00:00+00:00\nduration: 30m\nstatus: complete\n{sensitivity}attendees: [{person}]\npeople: [{person}]\naction_items:\n  - assignee: {person}\n    task: {secret}\n    status: open\ndecisions:\n  - text: {secret}\n    topic: pricing\nintents:\n  - kind: commitment\n    what: {secret}\n    who: {person}\n    status: open\n---\n\n## Transcript\n\n{secret}\n"
        )
    }

    #[test]
    fn unscoped_coach_grounding_fails_closed_instead_of_exporting_archive_identities() {
        let _guard = crate::test_home_env_lock();
        let temp = tempfile::tempdir().unwrap();
        let _home = HomeOverride::set(temp.path());
        let meetings = temp.path().join("meetings");
        std::fs::create_dir_all(&meetings).unwrap();
        std::fs::write(
            meetings.join("normal.md"),
            meeting(
                "Pricing Sync",
                "Sam Lee",
                "Share the public pricing deck",
                false,
            ),
        )
        .unwrap();
        std::fs::write(
            meetings.join("restricted.md"),
            meeting(
                "Board Pricing",
                "Alex Kim",
                "SECRET board pricing floor",
                true,
            ),
        )
        .unwrap();

        let mut config = Config::default();
        config.output_dir = meetings;
        let card = BattleCard::assemble(&config, "pricing").unwrap();

        assert!(!card.rendered.contains("Sam Lee"));
        assert!(!card.rendered.contains("public pricing deck"));
        assert!(!card.rendered.contains("Alex Kim"));
        assert!(!card.rendered.contains("SECRET"));
        assert!(!card.rendered.contains("Board Pricing"));
        assert!(card
            .rendered
            .contains("No explicit or calendar-confirmed participant context"));
        assert!(card.rendered.len() <= BATTLE_CARD_CHAR_BUDGET);

        let serialized_card = serde_json::to_string(&card).unwrap();
        assert!(!serialized_card.contains("Alex Kim"));
        assert!(!serialized_card.contains("SECRET"));
        assert!(!serialized_card.contains("Board Pricing"));

        let utterance = CopilotUtterance {
            utterance_sequence: 1,
            revision: 1,
            update_kind: TranscriptUpdateKind::Final,
            source: "live.utterance.final".into(),
            text: "Can we confirm the public pricing deck owner?".into(),
            speaker: None,
            speaker_verified: false,
            offset_ms: 0,
            duration_ms: 100,
        };
        let fast_request = CopilotRequest {
            goal: "Confirm the public pricing plan".into(),
            mode: MeetingMode::Decision,
            session_epoch: 1,
            evidence_revision: 1,
            evidence_utterance_sequence: 1,
            evidence_utterance_revision: 1,
            update_kind: TranscriptUpdateKind::Final,
            utterances: vec![utterance.clone()],
            battle_card: card.clone(),
            strategy_state: StrategyState::empty(),
        };
        assert!(!fast_request.untrusted_payload().contains("SECRET"));

        let depth_request = StrategyRequest {
            goal: fast_request.goal.clone(),
            mode: fast_request.mode,
            evidence_revision: fast_request.evidence_revision,
            reason: StrategyRefreshReason::Initial,
            utterances: vec![utterance],
            battle_card: card,
            prior_state: StrategyState::empty(),
        };
        let depth_payload = depth_request.untrusted_payload();
        assert!(!depth_payload.contains("Alex Kim"));
        assert!(!depth_payload.contains("SECRET"));
        assert!(!depth_payload.contains("Board Pricing"));

        let strategy = StrategyState::from_draft(
            depth_request.heuristic_draft(),
            depth_request.evidence_revision,
        );
        let serialized_strategy = serde_json::to_string(&strategy).unwrap();
        assert!(!serialized_strategy.contains("Alex Kim"));
        assert!(!serialized_strategy.contains("SECRET"));
        assert!(!serialized_strategy.contains("Board Pricing"));
    }
}
