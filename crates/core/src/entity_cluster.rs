//! Person entity-resolution clustering — issue #385, class 3 (name-variant
//! fragmentation, e.g. `junrei` / `jun-rei` / `junlei` / `junwei`).
//!
//! This is **suggestion-only**. It groups people whose names are plausibly the
//! same person so the graph can surface them as candidates for a human to
//! confirm. It NEVER merges or writes anything. Per the entity-resolution plan
//! (`docs/plans/person-entity-resolution-2026-06-26.md`) a wrong merge is worse
//! than a split, so the merge action is a confirm-gated follow-up. Because
//! nothing is written, an over-eager suggestion costs precision, not data.
//!
//! Two link tiers, both conservative:
//! - **Separator variant** — identical characters modulo separators and case
//!   (`Mo-Han` ~ `mohan`, `jun-rei` ~ `junrei`). Guarded by a minimum length.
//! - **Spelling edit** — single-token, ASCII names that share a first letter and
//!   are within a length-scaled edit budget (`geert` ~ `gert`, `junrei` ~
//!   `junlei`). ASCII-only because same-first + bounded edit is a Latin
//!   heuristic; a single edit between distinct non-ASCII names (李雷/李蕾) is not
//!   a same-person signal. Reuses the exact edit-distance matcher from
//!   `name_correction`.

use crate::name_correction::{levenshtein, normalize_name};

/// Why two names were linked. Kept for tests/scoring; the graph display collapses
/// clusters and does not surface the per-edge reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatchReason {
    /// Same characters modulo separators/case (`jun-rei` ~ `junrei`).
    SeparatorVariant,
    /// Same first letter, within edit budget (`geert` ~ `gert`).
    PhoneticEdit,
}

/// Minimum compact length for a separator-variant match, so short initials
/// (`d-p`, `c-s`) don't collapse to one entity.
const MIN_COMPACT_LEN: usize = 3;

/// Compact key: normalized (lowercase + accent-folded) with every non-alphanumeric
/// character removed, so pure separator/case variants share a key
/// (`Mo-Han` -> `mohan`, `jun-rei` -> `junrei`, `José` -> `jose`).
pub(crate) fn compact_key(name: &str) -> String {
    normalize_name(name)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Edit budget for a normalized token: 1 edit for short names, 2 for longer.
/// Mirrors `name_correction::distance_budget`.
fn distance_budget(len: usize) -> usize {
    if len >= 6 {
        2
    } else {
        1
    }
}

fn is_single_token(name: &str) -> bool {
    normalize_name(name).split_whitespace().count() == 1
}

/// Are these two names plausibly the same person? Returns the reason when yes.
///
/// SUGGESTION strength only — this decides whether to *propose* a link, never to
/// merge. It is deliberately recall-oriented in the ambiguous short-name band
/// (`sam`/`sami`, `an`/`ann`): those become suggestions a human resolves. It
/// stays confidently negative for dissimilar names (different first letter,
/// different phonetics, out of edit budget).
pub(crate) fn names_plausibly_same_person(a: &str, b: &str) -> Option<MatchReason> {
    let ca = compact_key(a);
    let cb = compact_key(b);
    if ca.is_empty() || cb.is_empty() {
        return None;
    }

    // Tier 1: identical compact key => separator/case variant.
    if ca == cb {
        if ca.chars().count() >= MIN_COMPACT_LEN {
            return Some(MatchReason::SeparatorVariant);
        }
        return None;
    }

    // Tier 2: single-edit spelling drift, single-token names only. Multi-token
    // names are the existing `names_likely_same` (prefix/last-name) territory,
    // and edit distance over full multi-word strings is too noisy to trust.
    if !is_single_token(a) || !is_single_token(b) {
        return None;
    }
    let na = normalize_name(a);
    let nb = normalize_name(b);
    // ASCII-only. Same-first-letter + bounded edit is an English/Latin heuristic;
    // for non-ASCII scripts (e.g. CJK) a single edit between DISTINCT names
    // (李雷 vs 李蕾) is common and must not be suggested as the same person.
    // Accent-folded Latin names (José -> jose) are ASCII here, so they still match.
    if !na.is_ascii() || !nb.is_ascii() {
        return None;
    }
    // Require the same first letter: a coincidental single edit across different
    // initials is too weak a signal to even suggest.
    if na.as_bytes().first() != nb.as_bytes().first() {
        return None;
    }
    let budget = distance_budget(na.chars().count().min(nb.chars().count()));
    if levenshtein(&na, &nb) <= budget {
        Some(MatchReason::PhoneticEdit)
    } else {
        None
    }
}

/// Cluster an explicit edge list into CLIQUES (groups where every pair is a
/// direct edge), not mere connected components. This is what prevents fuzzy-match
/// drift CHAINS from bridging distinct people: `jon`~`jan`~`jana` form one
/// connected component, but `jon`/`jana` are not directly linked, so they must
/// never share a cluster. Real drift groups (`junrei`/`junlei`/`junwei`/`jun-rei`)
/// are fully connected and survive as one cluster.
///
/// A connected component that is already a clique is emitted whole. A non-clique
/// component is decomposed into its direct-edge pairs (2-member clusters), so
/// every real pairwise link is still surfaced without the spurious transitive
/// pair. Output is deterministic: members sorted, clusters ordered lexically.
pub(crate) fn cluster_indices(n: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
    use std::collections::{BTreeMap, BTreeSet};

    // Normalize edges to (min, max) and dedup.
    let mut edge_set: BTreeSet<(usize, usize)> = BTreeSet::new();
    for &(a, b) in edges {
        if a >= n || b >= n || a == b {
            continue;
        }
        edge_set.insert((a.min(b), a.max(b)));
    }

    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut root = x;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = x;
        while parent[cur] != root {
            let next = parent[cur];
            parent[cur] = root;
            cur = next;
        }
        root
    }
    for &(a, b) in &edge_set {
        let ra = find(&mut parent, a);
        let rb = find(&mut parent, b);
        if ra != rb {
            if ra < rb {
                parent[rb] = ra;
            } else {
                parent[ra] = rb;
            }
        }
    }

    let mut components: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        components.entry(root).or_default().push(i);
    }

    let is_edge = |a: usize, b: usize| edge_set.contains(&(a.min(b), a.max(b)));
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for members in components.into_values() {
        if members.len() < 2 {
            continue;
        }
        // Clique? every pair within the component must be a direct edge.
        let is_clique = members
            .iter()
            .enumerate()
            .all(|(i, &a)| members[i + 1..].iter().all(|&b| is_edge(a, b)));
        if is_clique {
            clusters.push(members); // already ascending (built from 0..n)
        } else {
            // Decompose into direct-edge pairs so distinct chain endpoints
            // (jon/jana) never share a cluster.
            for i in 0..members.len() {
                for j in (i + 1)..members.len() {
                    if is_edge(members[i], members[j]) {
                        clusters.push(vec![members[i], members[j]]);
                    }
                }
            }
        }
    }
    clusters.sort();
    clusters
}

/// Convenience: cluster a flat list of names using [`names_plausibly_same_person`]
/// as the edge source, then [`cluster_indices`]. This is the same predicate the
/// graph layer uses for `alias_clusters`, so the eval mirrors production.
#[cfg(test)]
pub(crate) fn cluster_names(names: &[String]) -> Vec<Vec<usize>> {
    let mut edges = Vec::new();
    for i in 0..names.len() {
        for j in (i + 1)..names.len() {
            if names_plausibly_same_person(&names[i], &names[j]).is_some() {
                edges.push((i, j));
            }
        }
    }
    cluster_indices(names.len(), &edges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_key_folds_separators_case_and_accents() {
        assert_eq!(compact_key("Mo-Han"), "mohan");
        assert_eq!(compact_key("jun-rei"), "junrei");
        assert_eq!(compact_key("José"), "jose");
    }

    #[test]
    fn separator_variants_link() {
        assert_eq!(
            names_plausibly_same_person("jun-rei", "junrei"),
            Some(MatchReason::SeparatorVariant)
        );
    }

    #[test]
    fn short_separator_keys_do_not_link() {
        // "d-p" / "dp" compact to "dp" (len 2) < MIN_COMPACT_LEN.
        assert_eq!(names_plausibly_same_person("d-p", "dp"), None);
    }

    #[test]
    fn spelling_edit_drift_links() {
        assert_eq!(
            names_plausibly_same_person("geert", "gert"),
            Some(MatchReason::PhoneticEdit)
        );
        // r/l and r/w drift, same first letter, within budget.
        assert!(names_plausibly_same_person("junrei", "junlei").is_some());
        assert!(names_plausibly_same_person("junrei", "junwei").is_some());
    }

    #[test]
    fn dissimilar_names_do_not_link() {
        // Different first letter is not a signal, even for a single edit (c/k).
        assert_eq!(names_plausibly_same_person("carl", "karl"), None);
        assert_eq!(names_plausibly_same_person("carl", "deepak"), None);
        assert_eq!(names_plausibly_same_person("sarah", "sam"), None);
        assert_eq!(names_plausibly_same_person("bright", "liam"), None);
    }

    #[test]
    fn non_ascii_near_miss_does_not_link() {
        // Distinct CJK names one codepoint apart must NOT be suggested (no ASCII
        // phonetic/edit corroboration applies).
        assert_eq!(names_plausibly_same_person("李雷", "李蕾"), None);
    }

    #[test]
    fn multi_token_names_use_separator_tier_only() {
        // "Alex Chen" vs "Alex Kim": different compact, multi-token -> no phonetic tier.
        assert_eq!(names_plausibly_same_person("Alex Chen", "Alex Kim"), None);
    }

    #[test]
    fn clique_clustering_groups_a_drift_set() {
        // The jun* fragments are mutually linked (a clique), so they collapse to
        // one cluster; `bright` is unrelated and excluded.
        let names: Vec<String> = ["jun-rei", "junrei", "junlei", "junwei", "bright"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let clusters = cluster_names(&names);
        assert_eq!(clusters.len(), 1, "one cluster, bright excluded");
        // Indices 0..=3 are the jun* set.
        assert_eq!(clusters[0], vec![0, 1, 2, 3]);
    }

    #[test]
    fn drift_chain_does_not_bridge_endpoints() {
        // `jon`~`jan` and `jan`~`jana` are edges, but `jon`~`jana` is not (dist 2).
        // Clique emission must NOT put the chain endpoints in one cluster.
        let names: Vec<String> = ["jon", "jan", "jana"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let clusters = cluster_names(&names);
        for c in &clusters {
            assert!(
                !(c.contains(&0) && c.contains(&2)),
                "drift-chain endpoints jon/jana were bridged: {c:?}"
            );
        }
    }

    #[test]
    fn clustering_is_deterministic_and_excludes_singletons() {
        let names: Vec<String> = ["zeta", "gert", "geert"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let clusters = cluster_names(&names);
        // gert(1) ~ geert(2); zeta(0) is a singleton and excluded.
        assert_eq!(clusters, vec![vec![1, 2]]);
    }
}
