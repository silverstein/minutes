//! Known names: spelling bias for the speech model and a resolver for near misses.
//!
//! Speech recognition mishears names. The prototype's dominant failure was a
//! literal lookup of a misheard surname followed by "not found". Two fixes, both
//! here: inject the people Mat actually talks to (and the vocabulary terms) into
//! the system prompt, and give the model a `resolve_person` tool that matches a
//! heard name against that list with a phonetic key plus edit distance.

use serde::Serialize;

use crate::config::Config;
use crate::graph::{PolicyProjectionRequest, PolicyProjectionResponse};

/// One known person from the relationship projection.
#[derive(Debug, Clone, Serialize)]
pub struct KnownPerson {
    pub name: String,
    pub meetings: i64,
    pub last_seen: String,
}

/// A resolver candidate.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub name: String,
    pub meetings: i64,
    pub last_seen: String,
    pub score: f32,
}

/// People plus vocabulary terms, loaded once per session.
#[derive(Debug, Clone, Default)]
pub struct NameIndex {
    pub people: Vec<KnownPerson>,
    pub terms: Vec<String>,
}

impl NameIndex {
    /// Load the top `limit` people from the policy graph projection and every
    /// vocabulary canonical plus alias. Failures degrade to an empty list; a
    /// missing graph must not stop a voice session.
    pub fn load(config: &Config, limit: usize) -> Self {
        let people = match crate::graph_worker::run_policy_projection_worker(
            config,
            PolicyProjectionRequest::People {
                limit,
                include_commitments: false,
                include_stats: false,
            },
        ) {
            Ok(PolicyProjectionResponse::People(p)) => p
                .people
                .into_iter()
                .filter(|p| !p.name.trim().is_empty())
                .map(|p| KnownPerson {
                    name: p.name,
                    meetings: p.meeting_count,
                    last_seen: p.last_seen,
                })
                .collect(),
            Ok(_) => Vec::new(),
            Err(e) => {
                tracing::warn!(error = %e, "voice live: people projection unavailable");
                Vec::new()
            }
        };
        let terms = match crate::vocabulary::load() {
            Ok(store) => {
                let mut t: Vec<String> = Vec::new();
                for e in store.entries {
                    t.push(e.canonical);
                    t.extend(e.aliases);
                }
                t.sort();
                t.dedup();
                t
            }
            Err(_) => Vec::new(),
        };
        Self { people, terms }
    }

    /// Build from explicit values (tests, or hosts with their own source).
    pub fn from_parts(people: Vec<KnownPerson>, terms: Vec<String>) -> Self {
        Self { people, terms }
    }

    /// Comma-separated names for the prompt, most frequent first.
    pub fn prompt_names(&self, max: usize) -> String {
        self.people
            .iter()
            .take(max)
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Comma-separated vocabulary terms for the prompt.
    pub fn prompt_terms(&self) -> String {
        self.terms.join(", ")
    }

    /// Best `k` candidates for a heard name.
    pub fn resolve(&self, heard: &str, k: usize) -> Vec<Candidate> {
        let q = normalize(heard);
        let qt: Vec<&str> = q.split(' ').filter(|s| !s.is_empty()).collect();
        if qt.is_empty() {
            return Vec::new();
        }
        let q_first = qt[0];
        let q_last = qt[qt.len() - 1];
        let mut scored: Vec<Candidate> = self
            .people
            .iter()
            .map(|p| {
                let n = normalize(&p.name);
                let nt: Vec<&str> = n.split(' ').filter(|s| !s.is_empty()).collect();
                let (n_first, n_last) = match nt.as_slice() {
                    [] => ("", ""),
                    [one] => (*one, *one),
                    [first, .., last] => (*first, *last),
                };
                let mut s = similarity(&q, &n).max(similarity(&phonetic(&q), &phonetic(&n)));
                if qt.len() > 1 && nt.len() > 1 {
                    let first = similarity(q_first, n_first)
                        .max(similarity(&phonetic(q_first), &phonetic(n_first)));
                    let last = similarity(q_last, n_last)
                        .max(similarity(&phonetic(q_last), &phonetic(n_last)));
                    s = s.max(0.45 * first + 0.55 * last);
                } else if qt.len() == 1 {
                    let first_exact = if q_first == n_first { 0.8 } else { 0.0 };
                    let last_exact = if q_first == n_last { 0.75 } else { 0.0 };
                    let first_phon = similarity(&phonetic(q_first), &phonetic(n_first)) * 0.7;
                    s = s.max(first_exact).max(last_exact).max(first_phon);
                }
                Candidate {
                    name: p.name.clone(),
                    meetings: p.meetings,
                    last_seen: p.last_seen.clone(),
                    score: (s * 100.0).round() / 100.0,
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.meetings.cmp(&a.meetings))
        });
        scored.truncate(k);
        scored
    }
}

/// Lowercase ASCII letters and single spaces; diacritics folded where trivial.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for ch in s.chars() {
        let mapped: Option<char> = match ch.to_ascii_lowercase() {
            c if c.is_ascii_alphabetic() => Some(c),
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => Some('a'),
            'è' | 'é' | 'ê' | 'ë' => Some('e'),
            'ì' | 'í' | 'î' | 'ï' => Some('i'),
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' => Some('o'),
            'ù' | 'ú' | 'û' | 'ü' => Some('u'),
            'ç' => Some('c'),
            'ñ' => Some('n'),
            _ => None,
        };
        match mapped {
            Some(c) => {
                out.push(c);
                last_space = false;
            }
            None => {
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            }
        }
    }
    out.trim().to_string()
}

/// A crude phonetic key: fold common confusions, drop interior vowels, collapse doubles.
/// "bennimos" and "benamoz" both become "bnms".
pub fn phonetic(s: &str) -> String {
    let t = normalize(s)
        .replace("ph", "f")
        .replace("ck", "k")
        .replace("th", "t")
        .replace("sh", "s")
        .replace("gh", "g")
        .replace('q', "k")
        .replace('x', "ks")
        .replace('z', "s")
        .replace('w', "v")
        .replace('y', "i")
        .replace('j', "g");
    let t = soft_c(&t);
    t.split(' ')
        .map(|w| {
            let mut chars = w.chars();
            let mut key = String::new();
            if let Some(first) = chars.next() {
                key.push(first);
            }
            let mut prev: Option<char> = key.chars().next();
            for c in chars {
                if matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'h') {
                    continue;
                }
                if prev == Some(c) {
                    continue;
                }
                key.push(c);
                prev = Some(c);
            }
            key
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn soft_c(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == 'c' {
            let next = chars.get(i + 1).copied();
            out.push(if matches!(next, Some('e') | Some('i') | Some('y')) {
                's'
            } else {
                'k'
            });
        } else {
            out.push(c);
        }
    }
    out
}

/// 1.0 identical, 0.0 nothing in common, based on Levenshtein distance.
pub fn similarity(a: &str, b: &str) -> f32 {
    let max = a.chars().count().max(b.chars().count()).max(1);
    1.0 - crate::name_correction::levenshtein(a, b) as f32 / max as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> NameIndex {
        NameIndex::from_parts(
            vec![
                KnownPerson {
                    name: "Mathieu Silverstein".into(),
                    meetings: 500,
                    last_seen: "2026-09-01".into(),
                },
                KnownPerson {
                    name: "Dan Benamoz".into(),
                    meetings: 12,
                    last_seen: "2026-08-25".into(),
                },
                KnownPerson {
                    name: "Daniel Brooks".into(),
                    meetings: 3,
                    last_seen: "2026-05-01".into(),
                },
                KnownPerson {
                    name: "Gunnar Holm".into(),
                    meetings: 4,
                    last_seen: "2026-09-09".into(),
                },
            ],
            vec!["RxVIP".into()],
        )
    }

    #[test]
    fn phonetic_key_folds_the_prototype_miss() {
        assert_eq!(phonetic("Bennimos"), phonetic("Benamoz"));
    }

    #[test]
    fn misheard_surname_resolves_to_the_right_person() {
        let c = index().resolve("Dan Bennimos", 3);
        assert_eq!(c[0].name, "Dan Benamoz");
        assert!(c[0].score >= 0.8, "score {}", c[0].score);
        assert!(c[0].score > c[1].score);
    }

    #[test]
    fn first_name_only_prefers_exact_first_name_matches() {
        let c = index().resolve("Dan", 3);
        assert_eq!(c[0].name, "Dan Benamoz");
    }

    #[test]
    fn unrelated_names_score_low() {
        let c = index().resolve("Priya Natarajan", 1);
        assert!(c[0].score < 0.6, "score {}", c[0].score);
    }

    #[test]
    fn prompt_lists_are_frequency_ordered_and_capped() {
        let idx = index();
        assert_eq!(idx.prompt_names(2), "Mathieu Silverstein, Dan Benamoz");
        assert_eq!(idx.prompt_terms(), "RxVIP");
    }

    #[test]
    fn normalize_folds_case_punctuation_and_accents() {
        assert_eq!(normalize("  Zoë  O'Brien-Smith "), "zoe o brien smith");
    }
}
