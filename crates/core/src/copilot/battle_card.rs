use crate::config::Config;
use crate::{graph, search};
use serde::{Deserialize, Serialize};

const BATTLE_CARD_CHAR_BUDGET: usize = 7_000;

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
        let mut card = Self::default();
        let mut source_errors = Vec::new();

        // Rebuild before reading graph-derived context so a cache created by
        // older code cannot retain a meeting that is restricted today. If the
        // rebuild fails, omit graph context instead of consulting stale data.
        let graph_ready = match graph::rebuild_index(config) {
            Ok(_) => true,
            Err(error) => {
                source_errors.push(format!("graph rebuild: {error}"));
                false
            }
        };

        if graph_ready {
            match graph::relationship_map(config) {
                Ok(people) => {
                    card.people = people
                        .into_iter()
                        .take(8)
                        .map(|person| {
                            let topics = if person.top_topics.is_empty() {
                                "no recurring topics".to_string()
                            } else {
                                person.top_topics.join(", ")
                            };
                            format!(
                                "{} — {} prior meetings; topics: {}",
                                person.name, person.meeting_count, topics
                            )
                        })
                        .collect();
                }
                Err(error) => source_errors.push(format!("graph people: {error}")),
            }
        }

        if graph_ready {
            match graph::query_commitments(config, None) {
                Ok(commitments) => {
                    card.open_commitments = commitments
                        .into_iter()
                        .take(10)
                        .map(|commitment| {
                            let owner = commitment
                                .person_name
                                .as_deref()
                                .unwrap_or("owner not verified");
                            let due = commitment
                                .due_date
                                .as_deref()
                                .map(|date| format!("; due {date}"))
                                .unwrap_or_default();
                            format!(
                                "{}: {}{} (from {})",
                                owner, commitment.text, due, commitment.meeting_title
                            )
                        })
                        .collect();
                }
                Err(error) => source_errors.push(format!("graph commitments: {error}")),
            }
        }

        let filters = search::SearchFilters::default();
        match search::search_intents("", config, &filters) {
            Ok(intents) => {
                card.intents = intents
                    .into_iter()
                    .filter(|intent| intent.status == "open")
                    .take(10)
                    .map(|intent| {
                        let owner = intent.who.as_deref().unwrap_or("owner not verified");
                        format!(
                            "{:?}: {} — {} ({})",
                            intent.kind, intent.what, owner, intent.title
                        )
                    })
                    .collect();
            }
            Err(error) => source_errors.push(format!("structured intents: {error}")),
        }

        match search::cross_meeting_research(query, config, &filters) {
            Ok(research) => {
                card.decisions = research
                    .related_decisions
                    .into_iter()
                    .take(8)
                    .map(|decision| format!("{} ({})", decision.what, decision.title))
                    .collect();
                // `cross_meeting_research` can find goal-specific intents that
                // are especially valuable; add them after the global open list
                // and dedupe below.
                card.intents
                    .extend(
                        research
                            .related_open_intents
                            .into_iter()
                            .take(5)
                            .map(|intent| {
                                let owner = intent.who.as_deref().unwrap_or("owner not verified");
                                format!(
                                    "{:?}: {} — {} ({})",
                                    intent.kind, intent.what, owner, intent.title
                                )
                            }),
                    );
                dedupe(&mut card.intents);
                card.intents.truncate(10);
            }
            Err(error) => source_errors.push(format!("decision research: {error}")),
        }

        if !query.trim().is_empty() {
            match search::search(query, config, &filters) {
                Ok(results) => {
                    card.fts_excerpts = results
                        .into_iter()
                        .take(6)
                        .map(|result| {
                            format!("{} ({}): {}", result.title, result.date, result.snippet)
                        })
                        .collect();
                }
                Err(error) => source_errors.push(format!("FTS: {error}")),
            }
        }

        if card.people.is_empty()
            && card.open_commitments.is_empty()
            && card.decisions.is_empty()
            && card.intents.is_empty()
            && card.fts_excerpts.is_empty()
            && !source_errors.is_empty()
        {
            return Err(BattleCardError::SourcesUnavailable(
                source_errors.join("; "),
            ));
        }

        card.rendered = render_bounded(&card);
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

fn render_bounded(card: &BattleCard) -> String {
    let mut output = String::new();
    append_section(&mut output, "People", &card.people);
    append_section(&mut output, "Open commitments", &card.open_commitments);
    append_section(&mut output, "Recent decisions", &card.decisions);
    append_section(&mut output, "Open intents", &card.intents);
    append_section(&mut output, "Relevant excerpts", &card.fts_excerpts);
    output
}

fn append_section(output: &mut String, heading: &str, values: &[String]) {
    if values.is_empty() || output.len() >= BATTLE_CARD_CHAR_BUDGET {
        return;
    }
    let heading_line = format!("## {heading}\n");
    if output.len() + heading_line.len() <= BATTLE_CARD_CHAR_BUDGET {
        output.push_str(&heading_line);
    }
    for value in values {
        let line = format!("- {value}\n");
        if output.len() + line.len() > BATTLE_CARD_CHAR_BUDGET {
            break;
        }
        output.push_str(&line);
    }
    if output.len() < BATTLE_CARD_CHAR_BUDGET {
        output.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn assembly_excludes_restricted_history_from_graph_structured_and_fts_sources() {
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

        assert!(card.rendered.contains("Sam Lee"));
        assert!(card.rendered.contains("public pricing deck"));
        assert!(!card.rendered.contains("Alex Kim"));
        assert!(!card.rendered.contains("SECRET"));
        assert!(!card.rendered.contains("Board Pricing"));
        assert!(card.rendered.len() <= BATTLE_CARD_CHAR_BUDGET);
    }
}
