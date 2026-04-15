use crate::knowledge::slugify;
use crate::markdown::EntityRef;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersonIdentity {
    pub slug: String,
    pub name: String,
    pub aliases: Vec<String>,
}

#[derive(Clone, Debug)]
struct PersonCandidate {
    identity: PersonIdentity,
    alias_score: usize,
    support_score: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PersonCanonicalizer {
    exact_matches: HashMap<String, Vec<usize>>,
    slug_matches: HashMap<String, Vec<usize>>,
    candidates: Vec<PersonCandidate>,
}

impl PersonCanonicalizer {
    pub(crate) fn new<I, S>(entities: &[EntityRef], context_names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut canonicalizer = Self::default();

        for entity in entities {
            let Some(identity) = normalize_entity_identity(entity) else {
                continue;
            };

            let exact_keys = exact_keys_for_entity(entity);
            let slug_keys = slug_keys_for_entity(entity);

            let alias_score = exact_keys.len().max(slug_keys.len());
            let idx = canonicalizer.candidates.len();
            canonicalizer.candidates.push(PersonCandidate {
                identity,
                alias_score,
                support_score: 1,
            });

            for key in exact_keys {
                canonicalizer
                    .exact_matches
                    .entry(key)
                    .or_default()
                    .push(idx);
            }
            for key in slug_keys {
                canonicalizer.slug_matches.entry(key).or_default().push(idx);
            }
        }

        let context_values: Vec<String> = context_names
            .into_iter()
            .filter_map(|raw| normalize_raw_name(raw.as_ref()).map(|(_, name)| name.to_string()))
            .collect();

        for raw in context_values {
            let exact = canonicalizer.lookup_exact(&raw);
            if let Some(idx) = canonicalizer.pick_best_index(exact) {
                canonicalizer.candidates[idx].support_score += 1;
                continue;
            }

            let slug = slugify(&raw);
            if slug.is_empty() {
                continue;
            }

            if let Some(idx) = canonicalizer.pick_best_index(canonicalizer.lookup_slug(&slug)) {
                canonicalizer.candidates[idx].support_score += 1;
            }
        }

        canonicalizer
    }

    pub(crate) fn resolve(&self, raw: &str) -> Option<PersonIdentity> {
        let (_, trimmed) = normalize_raw_name(raw)?;

        if let Some(idx) = self.pick_best_index(self.lookup_exact(trimmed)) {
            return Some(self.candidates[idx].identity.clone());
        }

        let slug = slugify(trimmed);
        if slug.is_empty() {
            return None;
        }

        if let Some(idx) = self.pick_best_index(self.lookup_slug(&slug)) {
            return Some(self.candidates[idx].identity.clone());
        }

        Some(PersonIdentity {
            slug,
            name: trimmed.to_string(),
            aliases: vec![],
        })
    }

    pub(crate) fn resolve_entity(&self, entity: &EntityRef) -> Option<PersonIdentity> {
        if let Some(identity) = self.resolve(&entity.label) {
            return Some(identity);
        }
        if let Some(identity) = self.resolve(&entity.slug) {
            return Some(identity);
        }
        normalize_entity_identity(entity)
    }

    fn lookup_exact<'a>(&'a self, raw: &str) -> &'a [usize] {
        self.exact_matches
            .get(&raw.to_ascii_lowercase())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn lookup_slug<'a>(&'a self, slug: &str) -> &'a [usize] {
        self.slug_matches
            .get(slug)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn pick_best_index(&self, indices: &[usize]) -> Option<usize> {
        let mut best_idx: Option<usize> = None;
        let mut best_support = 0usize;
        let mut best_alias = 0usize;
        let mut ambiguous = false;

        for &idx in indices {
            let candidate = &self.candidates[idx];
            let support = candidate.support_score;
            let alias = candidate.alias_score;

            match best_idx {
                None => {
                    best_idx = Some(idx);
                    best_support = support;
                    best_alias = alias;
                }
                Some(_) if ambiguous => {
                    if support > best_support && alias > best_alias {
                        best_idx = Some(idx);
                        best_support = support;
                        best_alias = alias;
                        ambiguous = false;
                    }
                }
                Some(_) => {
                    if support > best_support || (support == best_support && alias > best_alias) {
                        best_idx = Some(idx);
                        best_support = support;
                        best_alias = alias;
                    } else if support == best_support && alias == best_alias {
                        ambiguous = true;
                    }
                }
            }
        }

        if ambiguous {
            None
        } else {
            best_idx
        }
    }
}

fn normalize_raw_name(raw: &str) -> Option<(&str, &str)> {
    let trimmed = raw.trim().trim_start_matches('@').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some((raw, trimmed))
    }
}

fn normalize_entity_identity(entity: &EntityRef) -> Option<PersonIdentity> {
    let slug = slugify(&entity.slug);
    if slug.is_empty() {
        return None;
    }

    let name = if entity.label.trim().is_empty() {
        entity.slug.trim().to_string()
    } else {
        entity.label.trim().to_string()
    };

    Some(PersonIdentity {
        slug,
        name,
        aliases: unique_aliases(entity.aliases.iter().cloned()),
    })
}

fn exact_keys_for_entity(entity: &EntityRef) -> HashSet<String> {
    let mut keys = HashSet::new();

    if !entity.slug.trim().is_empty() {
        keys.insert(entity.slug.trim().to_ascii_lowercase());
    }
    if !entity.label.trim().is_empty() {
        keys.insert(entity.label.trim().to_ascii_lowercase());
    }
    for alias in &entity.aliases {
        let trimmed = alias.trim();
        if !trimmed.is_empty() {
            keys.insert(trimmed.to_ascii_lowercase());
        }
    }

    keys
}

fn slug_keys_for_entity(entity: &EntityRef) -> HashSet<String> {
    let mut keys = HashSet::new();

    for value in std::iter::once(entity.slug.as_str())
        .chain(std::iter::once(entity.label.as_str()))
        .chain(entity.aliases.iter().map(String::as_str))
    {
        let slug = slugify(value);
        if !slug.is_empty() {
            keys.insert(slug);
        }
    }

    keys
}

fn unique_aliases<I>(aliases: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for alias in aliases {
        let trimmed = alias.trim();
        if trimmed.is_empty() {
            continue;
        }

        let key = trimmed.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(trimmed.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dan_entities() -> Vec<EntityRef> {
        vec![EntityRef {
            slug: "dan-benamoz".into(),
            label: "Dan Benamoz".into(),
            aliases: vec!["Dan".into(), "dan".into()],
        }]
    }

    #[test]
    fn resolves_raw_name_through_alias_table() {
        let resolver = PersonCanonicalizer::new(&dan_entities(), ["Dan"]);
        let identity = resolver.resolve("Dan").expect("resolved identity");
        assert_eq!(identity.slug, "dan-benamoz");
        assert_eq!(identity.name, "Dan Benamoz");
    }

    #[test]
    fn falls_back_to_raw_slug_when_no_entity_matches() {
        let resolver = PersonCanonicalizer::new(&[], ["Dan"]);
        let identity = resolver.resolve("Dan").expect("fallback identity");
        assert_eq!(identity.slug, "dan");
        assert_eq!(identity.name, "Dan");
    }

    #[test]
    fn chooses_stronger_context_when_aliases_collide() {
        let resolver = PersonCanonicalizer::new(
            &[
                EntityRef {
                    slug: "dan-benamoz".into(),
                    label: "Dan Benamoz".into(),
                    aliases: vec!["Dan".into(), "DB".into(), "Daniel".into()],
                },
                EntityRef {
                    slug: "dan-smith".into(),
                    label: "Dan Smith".into(),
                    aliases: vec!["Dan".into()],
                },
            ],
            ["Dan", "Dan Benamoz", "DB"],
        );

        let identity = resolver.resolve("Dan").expect("collision resolution");
        assert_eq!(identity.slug, "dan-benamoz");
    }

    #[test]
    fn case_insensitive_matching_works() {
        let resolver = PersonCanonicalizer::new(&dan_entities(), ["DAN"]);
        let identity = resolver.resolve("DAN").expect("case-insensitive identity");
        assert_eq!(identity.slug, "dan-benamoz");
    }

    #[test]
    fn ambiguous_collision_without_stronger_signal_falls_back() {
        let resolver = PersonCanonicalizer::new(
            &[
                EntityRef {
                    slug: "dan-benamoz".into(),
                    label: "Dan Benamoz".into(),
                    aliases: vec!["Dan".into()],
                },
                EntityRef {
                    slug: "dan-smith".into(),
                    label: "Dan Smith".into(),
                    aliases: vec!["Dan".into()],
                },
            ],
            ["Dan"],
        );

        let identity = resolver.resolve("Dan").expect("ambiguous fallback");
        assert_eq!(identity.slug, "dan");
        assert_eq!(identity.name, "Dan");
    }

    fn candidate(alias_score: usize, support_score: usize) -> PersonCandidate {
        PersonCandidate {
            identity: PersonIdentity {
                slug: format!("candidate-{alias_score}-{support_score}"),
                name: format!("Candidate {alias_score}/{support_score}"),
                aliases: vec![],
            },
            alias_score,
            support_score,
        }
    }

    #[test]
    fn pick_best_index_keeps_ambiguity_latched_after_equal_top_tie() {
        let canonicalizer = PersonCanonicalizer {
            candidates: vec![candidate(2, 1), candidate(2, 1), candidate(3, 1)],
            ..Default::default()
        };

        assert_eq!(canonicalizer.pick_best_index(&[0, 1, 2]), None);
    }

    #[test]
    fn pick_best_index_returns_strictly_higher_scoring_candidate() {
        let canonicalizer = PersonCanonicalizer {
            candidates: vec![candidate(2, 1), candidate(2, 1), candidate(3, 2)],
            ..Default::default()
        };

        assert_eq!(canonicalizer.pick_best_index(&[0, 1, 2]), Some(2));
    }

    #[test]
    fn pick_best_index_returns_none_when_all_top_candidates_tie() {
        let canonicalizer = PersonCanonicalizer {
            candidates: vec![candidate(2, 1), candidate(2, 1), candidate(2, 1)],
            ..Default::default()
        };

        assert_eq!(canonicalizer.pick_best_index(&[0, 1, 2]), None);
    }
}
