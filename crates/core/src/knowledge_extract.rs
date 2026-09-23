//! Fact extraction from meeting transcripts for knowledge base updates.
//!
//! Two-phase extraction with safety guardrails:
//! 1. **Structured-first** (no LLM): extract from YAML frontmatter — decisions, action_items, entities.
//!    Zero hallucination risk since these are already LLM-validated during summarization.
//! 2. **LLM extraction** (optional, engine != "none"): mine transcript body for richer facts.
//!    Conservative prompt: "only extract explicitly stated facts, never infer."

use crate::knowledge::{Confidence, Fact, PersonFacts};
use crate::markdown::Frontmatter;
use crate::person_identity::{PersonCanonicalizer, PersonIdentity};
use std::collections::HashMap;

/// Extract facts from structured frontmatter only (phase 1 — zero hallucination risk).
/// This is the default mode when `[knowledge] engine = "none"`.
pub fn extract_from_frontmatter(fm: &Frontmatter, meeting_path: &str) -> Vec<PersonFacts> {
    let date = fm.date.format("%Y-%m-%d").to_string();
    let meeting_slug = meeting_path
        .rsplit('/')
        .next()
        .unwrap_or(meeting_path)
        .trim_end_matches(".md")
        .to_string();

    let canonical_people = build_canonical_person_index(fm);
    let mut person_map: HashMap<String, PersonFacts> = HashMap::new();

    // Extract from action_items (high-value: explicit assignee + task)
    for item in &fm.action_items {
        if item.status == "done" {
            continue;
        }
        let Some(identity) = resolve_person_identity(&item.assignee, &canonical_people) else {
            continue;
        };
        let entry = person_map
            .entry(identity.slug.clone())
            .or_insert_with(|| PersonFacts {
                slug: identity.slug.clone(),
                name: identity.name.clone(),
                facts: vec![],
            });
        entry.facts.push(Fact {
            text: format!(
                "Committed to: {} (due: {})",
                item.task,
                item.due.as_deref().unwrap_or("unset")
            ),
            category: "commitment".into(),
            confidence: Confidence::Explicit,
            source_meeting: meeting_slug.clone(),
            source_date: date.clone(),
        });
    }

    // Extract from decisions (linked to topic, attributed to all attendees)
    for decision in &fm.decisions {
        // Decisions are attributed to the meeting, not a specific person.
        // We file them under each attendee present.
        for attendee in fm.normalized_attendees() {
            let Some(identity) = resolve_person_identity(&attendee, &canonical_people) else {
                continue;
            };
            let entry = person_map
                .entry(identity.slug.clone())
                .or_insert_with(|| PersonFacts {
                    slug: identity.slug.clone(),
                    name: identity.name.clone(),
                    facts: vec![],
                });
            let topic_str = decision
                .topic
                .as_deref()
                .map(|t| format!(" [{}]", t))
                .unwrap_or_default();
            entry.facts.push(Fact {
                text: format!("Decision{}: {}", topic_str, decision.text),
                category: "decision".into(),
                confidence: Confidence::Strong,
                source_meeting: meeting_slug.clone(),
                source_date: date.clone(),
            });
        }
    }

    // Extract from entities.people.
    //
    // Issue #245: this used to assert "Attended meeting" for every person
    // entity, at Strong confidence. But `entities.people` is built from a
    // deliberately broad list that includes people the summarizer only heard
    // named, so a person discussed on a call they were not on was recorded as
    // having attended it. Narrowing the `attendees:` field alone did not fix
    // that, because this path never reads `attendees:`.
    //
    // Presence is a claim about the world and needs a source that can support
    // it, so it now comes from the attendee list. Everyone else keeps a fact,
    // because being named in a meeting is real and worth knowing, but it says
    // what actually happened and carries the weaker confidence that matches.
    let attending: std::collections::HashSet<String> = fm
        .normalized_attendees()
        .iter()
        .filter_map(|attendee| resolve_person_identity(attendee, &canonical_people))
        .map(|identity| identity.slug)
        .collect();

    for entity in &fm.entities.people {
        let Some(identity) = canonical_people.resolve_entity(entity) else {
            continue;
        };
        // Only create the entry if they don't already have facts from above.
        // Avoids cluttering with "was in meeting" for people we already have richer data on.
        if !person_map.contains_key(&identity.slug) {
            let attended = attending.contains(&identity.slug);
            person_map.insert(
                identity.slug.clone(),
                PersonFacts {
                    slug: identity.slug.clone(),
                    name: identity.name.clone(),
                    facts: vec![Fact {
                        text: if attended {
                            format!("Attended meeting: {}", fm.title)
                        } else {
                            format!("Mentioned in meeting: {}", fm.title)
                        },
                        category: "context".into(),
                        confidence: if attended {
                            Confidence::Strong
                        } else {
                            Confidence::Inferred
                        },
                        source_meeting: meeting_slug.clone(),
                        source_date: date.clone(),
                    }],
                },
            );
        }
    }

    // Extract from intents
    for intent in &fm.intents {
        if let Some(ref who) = intent.who {
            let Some(identity) = resolve_person_identity(who, &canonical_people) else {
                continue;
            };
            let entry = person_map
                .entry(identity.slug.clone())
                .or_insert_with(|| PersonFacts {
                    slug: identity.slug.clone(),
                    name: identity.name.clone(),
                    facts: vec![],
                });
            let kind_label = format!("{:?}", intent.kind)
                .to_lowercase()
                .replace("_", " ");
            entry.facts.push(Fact {
                text: format!("{}: {}", capitalize_first(&kind_label), intent.what),
                category: match intent.kind {
                    crate::markdown::IntentKind::ActionItem
                    | crate::markdown::IntentKind::Commitment => "commitment".into(),
                    crate::markdown::IntentKind::Decision => "decision".into(),
                    crate::markdown::IntentKind::OpenQuestion => "context".into(),
                },
                confidence: Confidence::Strong,
                source_meeting: meeting_slug.clone(),
                source_date: date.clone(),
            });
        }
    }

    person_map.into_values().collect()
}

fn build_canonical_person_index(fm: &Frontmatter) -> PersonCanonicalizer {
    let attendees = fm.normalized_attendees();
    let context_names: Vec<&str> = attendees
        .iter()
        .map(String::as_str)
        .chain(fm.people.iter().map(String::as_str))
        .chain(fm.action_items.iter().map(|item| item.assignee.as_str()))
        .chain(fm.intents.iter().filter_map(|intent| intent.who.as_deref()))
        .collect();

    PersonCanonicalizer::new(&fm.entities.people, context_names)
}

fn resolve_person_identity(
    raw: &str,
    canonical_people: &PersonCanonicalizer,
) -> Option<PersonIdentity> {
    canonical_people.resolve(raw)
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::{ActionItem, ContentType, Decision, EntityLinks, EntityRef, Frontmatter};
    use chrono::{Local, TimeZone};

    fn test_frontmatter() -> Frontmatter {
        Frontmatter {
            title: "Q2 Strategy Call".into(),
            r#type: ContentType::Meeting,
            date: Local.with_ymd_and_hms(2026, 4, 3, 14, 0, 0).unwrap(),
            duration: "30m".into(),
            source: None,
            status: None,
            tags: vec![],
            attendees: vec!["Mat".into(), "Dan".into()],
            attendees_raw: None,
            calendar_event: None,
            people: vec![],
            entities: EntityLinks {
                people: vec![
                    EntityRef {
                        slug: "mat".into(),
                        label: "Mat".into(),
                        aliases: vec![],
                    },
                    EntityRef {
                        slug: "dan-benamoz".into(),
                        label: "Dan Benamoz".into(),
                        aliases: vec!["Dan".into(), "dan".into()],
                    },
                ],
                projects: vec![],
            },
            device: None,
            captured_at: None,
            context: None,
            action_items: vec![
                ActionItem {
                    assignee: "Mat".into(),
                    task: "Send pricing doc to Dan".into(),
                    due: Some("2026-04-05".into()),
                    status: "open".into(),
                },
                ActionItem {
                    assignee: "Dan".into(),
                    task: "Review privacy requirements".into(),
                    due: None,
                    status: "open".into(),
                },
            ],
            decisions: vec![Decision {
                text: "Switch to monthly billing for pharmacy consultations".into(),
                topic: Some("pricing".into()),
                authority: None,
                supersedes: None,
            }],
            intents: vec![],
            recorded_by: None,
            capture: None,
            sensitivity: None,
            debrief: None,
            consent: None,
            consent_notice: None,
            visibility: None,
            speaker_map: vec![],
            name_corrections: Vec::new(),
            recording_health: None,
            speaker_mapping: None,
            summarization: None,
            processing_warnings: Vec::new(),
            template: None,
            filter_diagnosis: None,
        }
    }

    #[test]
    fn a_person_only_mentioned_is_not_recorded_as_having_attended() {
        // Issue #245, and the reason narrowing `attendees:` alone was not the
        // whole fix. `entities.people` is built from a deliberately broad list,
        // and this path never reads `attendees:`, so a person who was merely
        // discussed still collected an "Attended meeting" fact at Strong
        // confidence. The reporter's two person call named six absent people.
        let mut fm = test_frontmatter();
        // Marcus is discussed on the call. He is not on it, so he is not an
        // attendee, and he has no action item or decision of his own.
        fm.entities.people.push(EntityRef {
            slug: "marcus".into(),
            label: "Marcus".into(),
            aliases: vec![],
        });

        let results = extract_from_frontmatter(&fm, "2026-04-03-strategy.md");

        let marcus = results
            .iter()
            .find(|pf| pf.slug == "marcus")
            .expect("a mentioned person should still be recorded, just not as present");
        assert_eq!(marcus.facts.len(), 1);
        assert!(
            marcus.facts[0].text.starts_with("Mentioned in meeting"),
            "expected a mention, got {:?}",
            marcus.facts[0].text
        );
        assert_eq!(marcus.facts[0].confidence, Confidence::Inferred);

        // And nothing about the call can be filed under him.
        assert!(
            !marcus
                .facts
                .iter()
                .any(|fact| fact.category == "decision" || fact.category == "commitment"),
            "a mentioned person must not be a party to the meeting's decisions"
        );
    }

    #[test]
    fn an_attendee_with_no_other_facts_still_reads_as_present() {
        // The other half of the same rule: narrowing the claim must not erase
        // it for people the attendee list does vouch for.
        //
        // The decisions pass files a fact under every attendee, and the
        // entities pass only fills in people who picked up nothing there, so
        // the fixture's decisions and action items are cleared to reach the
        // branch under test.
        let mut fm = test_frontmatter();
        fm.decisions.clear();
        fm.action_items.clear();
        fm.attendees.push("Priya".into());
        fm.entities.people.push(EntityRef {
            slug: "priya".into(),
            label: "Priya".into(),
            aliases: vec![],
        });

        let results = extract_from_frontmatter(&fm, "2026-04-03-strategy.md");

        let priya = results
            .iter()
            .find(|pf| pf.slug == "priya")
            .expect("an attendee should be recorded");
        assert!(
            priya
                .facts
                .iter()
                .any(|fact| fact.text.starts_with("Attended meeting")
                    && fact.confidence == Confidence::Strong),
            "expected an attendance fact, got {:?}",
            priya.facts
        );
    }

    #[test]
    fn extracts_action_items_as_commitments() {
        let fm = test_frontmatter();
        let results = extract_from_frontmatter(&fm, "2026-04-03-strategy.md");

        let mat_facts: Vec<&PersonFacts> = results.iter().filter(|pf| pf.slug == "mat").collect();
        assert_eq!(mat_facts.len(), 1);

        let commitment = mat_facts[0]
            .facts
            .iter()
            .find(|f| f.category == "commitment")
            .expect("should have commitment fact");
        assert!(commitment.text.contains("Send pricing doc"));
        assert_eq!(commitment.confidence, Confidence::Explicit);
        assert_eq!(commitment.source_meeting, "2026-04-03-strategy");
    }

    #[test]
    fn extracts_decisions_for_each_attendee() {
        let fm = test_frontmatter();
        let results = extract_from_frontmatter(&fm, "2026-04-03-strategy.md");

        // Both Mat and Dan should get the pricing decision
        for name in &["mat", "dan-benamoz"] {
            let pf: Vec<&PersonFacts> = results.iter().filter(|pf| pf.slug == *name).collect();
            assert!(!pf.is_empty(), "{} should have facts", name);
            let has_decision = pf[0].facts.iter().any(|f| f.category == "decision");
            assert!(has_decision, "{} should have decision fact", name);
        }
    }

    #[test]
    fn skips_done_action_items() {
        let mut fm = test_frontmatter();
        fm.action_items = vec![ActionItem {
            assignee: "Alice".into(),
            task: "Already completed".into(),
            due: None,
            status: "done".into(),
        }];
        fm.decisions = vec![];
        fm.entities.people = vec![];
        fm.attendees = vec![];

        let results = extract_from_frontmatter(&fm, "test.md");
        assert!(results.is_empty(), "done items should not produce facts");
    }

    #[test]
    fn entity_presence_only_when_no_richer_facts() {
        let mut fm = test_frontmatter();
        // Dan has action items + decisions (richer facts)
        // Add a third entity who has no action items or decisions
        fm.entities.people.push(EntityRef {
            slug: "jex-musa".into(),
            label: "Jex Musa".into(),
            aliases: vec![],
        });

        let results = extract_from_frontmatter(&fm, "test.md");
        let jex: Vec<&PersonFacts> = results.iter().filter(|pf| pf.slug == "jex-musa").collect();
        assert_eq!(jex.len(), 1);
        assert_eq!(jex[0].facts.len(), 1);
        assert!(jex[0].facts[0].text.contains("Attended meeting"));
    }

    #[test]
    fn merges_short_names_into_unique_entity_slug() {
        let fm = test_frontmatter();
        let results = extract_from_frontmatter(&fm, "2026-04-03-strategy.md");

        assert!(results.iter().all(|pf| pf.slug != "dan"));

        let dan = results
            .iter()
            .find(|pf| pf.slug == "dan-benamoz")
            .expect("canonical Dan profile should exist");
        assert_eq!(dan.name, "Dan Benamoz");
        assert!(dan.facts.iter().any(|fact| fact.category == "commitment"));
        assert!(dan.facts.iter().any(|fact| fact.category == "decision"));
    }

    #[test]
    fn first_name_matching_stays_disabled_when_entities_are_ambiguous() {
        let mut fm = test_frontmatter();
        fm.entities.people = vec![
            EntityRef {
                slug: "dan-benamoz".into(),
                label: "Dan Benamoz".into(),
                aliases: vec![],
            },
            EntityRef {
                slug: "dan-smith".into(),
                label: "Dan Smith".into(),
                aliases: vec![],
            },
        ];
        let identities = build_canonical_person_index(&fm);

        let fallback = resolve_person_identity("Dan", &identities).expect("fallback identity");
        assert_eq!(fallback.slug, "dan");
        assert_eq!(fallback.name, "Dan");
    }
}
