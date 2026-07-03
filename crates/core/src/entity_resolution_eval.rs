//! Entity-resolution evaluation harness — issue #385 / #371, "Slice 0".
//!
//! Cluster-level measurement for the suggestion-only name-variant clustering in
//! [`crate::entity_cluster`]. It is the entity twin of [`crate::name_eval`]:
//! synthetic names only (public repo), test-gated, with a hard gating invariant
//! that mirrors `name_eval`'s `false_corrections == 0`.
//!
//! Gating invariants:
//! - **`wrong_merges == 0`** — no suggested cluster may contain names belonging
//!   to two different ground-truth people. A wrong merge is worse than a split,
//!   and a suggested cluster is the input to a future confirm-merge, so an impure
//!   cluster is the cardinal error even though nothing is written yet.
//! - **full drift recall** — every reported same-person drift group is fully
//!   co-clustered. The feature is worthless if it cannot surface the motivating
//!   fragmentation.
//!
//! Scored but NOT gated: over-suggestion in the ambiguous short-name band
//! (`sam`/`sami`, `an`/`ann`) — an inherent precision cost of a recall-oriented
//! suggester, resolved later by a human confirm-merge. B-cubed precision/recall
//! are reported for visibility.
//!
//! All names below are SYNTHETIC and shaped to mirror the real gtheys vault
//! classes without reproducing anyone's actual name.

#![cfg(test)]

use crate::entity_cluster::{cluster_names, names_plausibly_same_person};
use std::collections::HashMap;

/// Groups of name fragments that each denote ONE synthetic person and therefore
/// MUST fully co-cluster (recall targets). Shapes mirror the real vault:
/// separator + medial-consonant drift, initial c/k phonetic drift, doubled-letter
/// drift.
const DRIFT_GROUPS: &[&[&str]] = &[
    // separator variant + medial consonant drift (the junrei/junlei/junwei/jun-rei
    // shape). 6+ chars so the budget is 2 and every pair (incl. the separator
    // variant vs the drift variants) links -> a true clique.
    &["tan-vir", "tanvir", "tanmir", "tanzir"],
    // same-first-letter medial vowel drift
    &["nadia", "nadya"],
    // doubled-letter drift
    &["aaron", "arron"],
];

/// Names that each denote a DISTINCT synthetic person; no two of them (and none
/// of them with any drift-group name) may share a suggested cluster. Includes
/// mutually dissimilar Latin names AND a non-ASCII near-miss pair (single
/// codepoint apart) that the ASCII-only edit tier must keep separate.
const DISTINCT_DISSIMILAR: &[&str] = &[
    "deepak", "sarah", "bright", "liam", "monica", "keith", "priya", "tomas", "李雷", "李蕾",
];

/// Short similar pairs where the predicate is expected to over-suggest. Scored,
/// NOT gated — a human resolves these via confirm-merge (a follow-up).
const AMBIGUOUS_PAIRS: &[(&str, &str)] = &[("sam", "sami"), ("an", "ann"), ("rena", "rana")];

#[derive(Debug, Default)]
struct ClusterReport {
    drift_groups_recovered: usize,
    drift_groups_total: usize,
    /// Clusters mixing two different ground-truth people. HARD GATE: must be 0.
    wrong_merges: usize,
    ambiguous_suggested: usize,
    ambiguous_total: usize,
    bcubed_precision: f64,
    bcubed_recall: f64,
}

/// Build the evaluation pool and ground-truth person id per name.
/// Person ids: drift group `g` -> id `g`; each dissimilar name -> its own id.
fn build_pool() -> (Vec<String>, HashMap<String, usize>) {
    let mut pool: Vec<String> = Vec::new();
    let mut truth: HashMap<String, usize> = HashMap::new();
    let mut next_id = 0usize;
    for group in DRIFT_GROUPS {
        let id = next_id;
        next_id += 1;
        for &name in *group {
            pool.push(name.to_string());
            truth.insert(name.to_string(), id);
        }
    }
    for &name in DISTINCT_DISSIMILAR {
        let id = next_id;
        next_id += 1;
        pool.push(name.to_string());
        truth.insert(name.to_string(), id);
    }
    (pool, truth)
}

fn run_eval() -> ClusterReport {
    let (pool, truth) = build_pool();
    let clusters = cluster_names(&pool);

    // Map each name to its predicted cluster id (singletons get a unique id).
    let mut predicted: HashMap<String, usize> = HashMap::new();
    for (cid, cluster) in clusters.iter().enumerate() {
        for &idx in cluster {
            predicted.insert(pool[idx].clone(), cid);
        }
    }
    let mut next_singleton = clusters.len();
    for name in &pool {
        predicted.entry(name.clone()).or_insert_with(|| {
            let id = next_singleton;
            next_singleton += 1;
            id
        });
    }

    let mut report = ClusterReport {
        drift_groups_total: DRIFT_GROUPS.len(),
        ambiguous_total: AMBIGUOUS_PAIRS.len(),
        ..Default::default()
    };

    // wrong_merges: predicted clusters mixing >1 ground-truth person.
    for cluster in &clusters {
        let mut persons: Vec<usize> = cluster.iter().map(|&i| truth[&pool[i]]).collect();
        persons.sort_unstable();
        persons.dedup();
        if persons.len() > 1 {
            report.wrong_merges += 1;
        }
    }

    // drift recall: all members of a group share one predicted cluster.
    for group in DRIFT_GROUPS {
        let first = predicted[&group[0].to_string()];
        if group.iter().all(|&n| predicted[&n.to_string()] == first) {
            report.drift_groups_recovered += 1;
        }
    }

    // ambiguous over-suggestion (reported only).
    for (a, b) in AMBIGUOUS_PAIRS {
        if names_plausibly_same_person(a, b).is_some() {
            report.ambiguous_suggested += 1;
        }
    }

    // B-cubed precision/recall over the pool.
    let (mut prec_sum, mut rec_sum) = (0.0f64, 0.0f64);
    for name in &pool {
        let pc = predicted[name];
        let gc = truth[name];
        let pred_members: Vec<&String> = pool.iter().filter(|n| predicted[*n] == pc).collect();
        let truth_members: Vec<&String> = pool.iter().filter(|n| truth[*n] == gc).collect();
        let correct = pred_members.iter().filter(|n| truth[**n] == gc).count() as f64;
        prec_sum += correct / pred_members.len() as f64;
        rec_sum += correct / truth_members.len() as f64;
    }
    report.bcubed_precision = prec_sum / pool.len() as f64;
    report.bcubed_recall = rec_sum / pool.len() as f64;

    report
}

#[test]
fn entity_clustering_meets_gates() {
    let report = run_eval();

    // HARD GATE 1: zero wrong merges (mirrors name_eval false_corrections == 0).
    assert_eq!(
        report.wrong_merges, 0,
        "a suggested cluster mixed two distinct ground-truth people: {report:?}"
    );

    // HARD GATE 2: every drift group is fully recovered.
    assert_eq!(
        report.drift_groups_recovered, report.drift_groups_total,
        "not all drift groups were co-clustered: {report:?}"
    );

    // B-cubed precision should stay high: dissimilar names must not accrete.
    assert!(
        report.bcubed_precision >= 0.99,
        "B-cubed precision regressed: {report:?}"
    );
    assert!(
        report.bcubed_recall >= 0.99,
        "B-cubed recall regressed: {report:?}"
    );

    // The ambiguous short-name band is expected to be over-suggested (the
    // predicate is recall-oriented; a human resolves these via confirm-merge).
    // Asserting all are suggested documents that behavior and would flag if the
    // predicate silently stopped proposing them. This is NOT the safety gate —
    // wrong_merges == 0 above is.
    assert_eq!(
        report.ambiguous_suggested, report.ambiguous_total,
        "ambiguous short-name pairs should all be suggested (recall-oriented): {report:?}"
    );
}

/// Structural gate for the transitive-drift-chain class: every suggested cluster
/// must be a clique under the predicate. A fuzzy-match chain (`jon`~`jan`~`jana`)
/// must never bridge non-matching endpoints into one cluster, even though it is a
/// single connected component. Guards against a regression back to
/// connected-component clustering.
#[test]
fn every_suggested_cluster_is_a_clique() {
    let mut pool: Vec<String> = build_pool().0;
    // Add a real drift chain so decomposition is actually exercised.
    pool.extend(["jon", "jan", "jana"].iter().map(|s| s.to_string()));
    let clusters = cluster_names(&pool);
    for c in &clusters {
        for i in 0..c.len() {
            for j in (i + 1)..c.len() {
                assert!(
                    names_plausibly_same_person(&pool[c[i]], &pool[c[j]]).is_some(),
                    "cluster is not a clique (transitive bridge): {:?}",
                    c.iter().map(|&k| pool[k].as_str()).collect::<Vec<_>>()
                );
            }
        }
    }
}
