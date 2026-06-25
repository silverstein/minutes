//! Name-accuracy evaluation harness (text-level, CI fast layer).
//!
//! The measurement contract for the post-pass name-correction lever
//! (bead minutes-25x3.4), per `docs/plans/every-name-right-2026-06-11.md`:
//! "measure first; without this, every change is vibes."
//!
//! Each fixture is a transcript snippet where a person name was mis-transcribed,
//! paired with the expected-name pool a meeting would supply (calendar
//! attendees, vocabulary, graph people) and the ground-truth spelling. The
//! scorer runs a correction function over the corpus and reports two numbers:
//! names recovered, and FALSE corrections (a token changed that should not have
//! been). "Wrong corrections are worse than wrong transcriptions," so false
//! corrections are the gating metric and must stay at zero.
//!
//! This is the text-level layer: it runs under `cargo test` with no audio and
//! no models, so it is part of the CI fast path. An audio layer (real
//! TTS/consented clips through whisper/parakeet, scoring raw-engine name WER) is
//! a separate feature-gated follow-up, like the real-whisper tests.
//!
//! Gated `#[cfg(test)]`: it is a test utility, not shipped code. The
//! correction lever (minutes-25x3.4) plugs its function into [`run_harness`].

/// One fixture case: a transcript snippet, the expected-name pool, and ground
/// truth. By construction `raw` and `truth` differ only at name tokens (the
/// correction is a 1:1 token replacement), so a position-aligned scorer cleanly
/// separates "name recovered" from "token corrupted".
pub(crate) struct NameCase {
    /// Language/scenario label, surfaced in failure messages.
    pub label: &'static str,
    /// Expected-name pool the meeting would supply.
    pub pool: &'static [&'static str],
    /// Transcript text as the engine produced it (name possibly mis-spelled).
    pub raw: &'static str,
    /// Ground-truth text with the correct name spelling.
    pub truth: &'static str,
}

/// Aggregate score of a correction function over the corpus.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct HarnessReport {
    /// Name occurrences (positions where `raw != truth`) recovered exactly.
    pub recovered: usize,
    /// Name occurrences still wrong after correction.
    pub missed: usize,
    /// Tokens changed that were already correct (`raw == truth` at that
    /// position but the candidate differs). The gating metric: must be zero.
    pub false_corrections: usize,
    /// Cases where the candidate changed the token count (a correction must be
    /// token-preserving). Counted and surfaced; expected to be zero.
    pub structural_mismatches: usize,
}

/// The fixture corpus: 6 correction targets across non-Anglo name families plus
/// 4 negatives that must remain untouched. Synthetic names only (public repo).
pub(crate) const CORPUS: &[NameCase] = &[
    // ---- correction targets (raw != truth at the name) ----
    NameCase {
        label: "dutch-geert",
        pool: &["Geert", "Sanne"],
        raw: "thanks bert for the notes",
        truth: "thanks Geert for the notes",
    },
    NameCase {
        label: "french-jacques",
        pool: &["Jacques", "Camille"],
        raw: "merci jacque for joining",
        truth: "merci Jacques for joining",
    },
    NameCase {
        label: "spanish-monica-accent",
        pool: &["Mónica", "Diego"],
        raw: "gracias monica for the update",
        truth: "gracias Mónica for the update",
    },
    NameCase {
        label: "indian-aishwarya",
        pool: &["Aishwarya", "Rohan"],
        raw: "over to ashwarya now",
        truth: "over to Aishwarya now",
    },
    NameCase {
        label: "chinese-xiulan",
        pool: &["Xiulan", "Wei"],
        raw: "shulan will present next",
        truth: "Xiulan will present next",
    },
    NameCase {
        label: "vietnamese-thanh",
        pool: &["Thanh", "Linh"],
        raw: "tan owns the rollout",
        truth: "Thanh owns the rollout",
    },
    // ---- negatives (raw == truth; any change is a false correction) ----
    NameCase {
        label: "neg-common-word-mark",
        pool: &["Mark", "Priya"],
        raw: "that was a good mark on the exam",
        truth: "that was a good mark on the exam",
    },
    NameCase {
        label: "neg-already-correct",
        pool: &["Sarah", "Tomas"],
        raw: "hi Sarah how are you",
        truth: "hi Sarah how are you",
    },
    NameCase {
        label: "neg-no-name-present",
        pool: &["Geert", "Mónica"],
        raw: "we shipped the quarterly report",
        truth: "we shipped the quarterly report",
    },
    NameCase {
        label: "neg-distant-token",
        pool: &["Aishwarya", "Jacques"],
        raw: "the feature is ready to demo",
        truth: "the feature is ready to demo",
    },
    // A common word / pronoun in a name slot (after a cue, before a name-verb)
    // that is a short edit from a short pool name. Must never be rewritten.
    NameCase {
        label: "neg-pronoun-in-name-position",
        pool: &["Wei", "Aki"],
        raw: "we will demo today",
        truth: "we will demo today",
    },
    // The [SPEAKER_N m:ss] prefix: SPEAKER is a short edit from a pool name and
    // is followed by a name-verb, but the bracket guard must keep it intact.
    NameCase {
        label: "neg-speaker-prefix",
        pool: &["Spencer"],
        raw: "[SPEAKER_1 0:05] will present",
        truth: "[SPEAKER_1 0:05] will present",
    },
];

/// Score one candidate transcript against ground truth, position-aligned by
/// whitespace token. `raw` is the engine output the candidate was derived from,
/// so a position where `raw == truth` but `candidate != truth` is a corruption
/// (false correction), and a position where `raw != truth` is a name target.
pub(crate) fn score_case(case: &NameCase, candidate: &str, report: &mut HarnessReport) {
    let raw: Vec<&str> = case.raw.split_whitespace().collect();
    let truth: Vec<&str> = case.truth.split_whitespace().collect();
    let cand: Vec<&str> = candidate.split_whitespace().collect();

    if cand.len() != truth.len() {
        // A correction pass must preserve token count. A mismatch is a hard
        // failure: every name target is unrecoverable here and we flag it.
        report.structural_mismatches += 1;
        for i in 0..truth.len() {
            if raw.get(i) != truth.get(i) {
                report.missed += 1;
            }
        }
        return;
    }

    for i in 0..truth.len() {
        let is_target = raw.get(i) != truth.get(i);
        let candidate_matches_truth = cand.get(i) == truth.get(i);
        if is_target {
            if candidate_matches_truth {
                report.recovered += 1;
            } else {
                report.missed += 1;
            }
        } else if cand.get(i) != raw.get(i) {
            // Token was already correct; the candidate changed it anyway.
            report.false_corrections += 1;
        }
    }
}

/// Run a correction function over the whole corpus and return the aggregate.
/// The function receives `(raw_text, expected_name_pool)` and returns the
/// corrected text (annotations, if any, should be stripped to plain text by the
/// caller before scoring; the baseline identity function returns `raw`).
pub(crate) fn run_harness(correct: impl Fn(&str, &[&str]) -> String) -> HarnessReport {
    let mut report = HarnessReport::default();
    for case in CORPUS {
        let candidate = correct(case.raw, case.pool);
        score_case(case, &candidate, &mut report);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The corpus has exactly 6 name targets (raw != truth) and 6 negatives.
    #[test]
    fn corpus_shape_is_six_targets_six_negatives() {
        let targets = CORPUS.iter().filter(|c| c.raw != c.truth).count();
        let negatives = CORPUS.iter().filter(|c| c.raw == c.truth).count();
        assert_eq!(targets, 6, "expected 6 correction-target cases");
        assert_eq!(negatives, 6, "expected 6 negative (no-change) cases");
    }

    /// Baseline: the identity correction (no change) recovers nothing and, by
    /// definition, makes zero false corrections. This records the "before" the
    /// post-pass lever (minutes-25x3.4) must improve: 6 names missed, 0 false.
    #[test]
    fn baseline_identity_misses_all_targets_with_no_false_corrections() {
        let report = run_harness(|raw, _pool| raw.to_string());
        assert_eq!(report.recovered, 0, "identity recovers no names");
        assert_eq!(report.missed, 6, "identity misses all 6 name targets");
        assert_eq!(
            report.false_corrections, 0,
            "identity must never corrupt a token"
        );
        assert_eq!(report.structural_mismatches, 0);
    }

    /// Every negative case must be left byte-identical by the identity pass:
    /// no false corrections, no name targets. Per-case so a regression names
    /// the offending fixture.
    #[test]
    fn negatives_unchanged_under_identity() {
        for case in CORPUS.iter().filter(|c| c.raw == c.truth) {
            let mut report = HarnessReport::default();
            score_case(case, case.raw, &mut report);
            assert_eq!(
                report.false_corrections, 0,
                "negative case {} should report no false corrections under identity",
                case.label
            );
            assert_eq!(
                report.missed, 0,
                "negative case {} has no name targets",
                case.label
            );
        }
    }

    /// The real post-pass correction (minutes-25x3.4) measured against the
    /// corpus. The gating invariant is zero false corrections; `recovered` is
    /// reported and asserted at the level v1 actually achieves.
    #[test]
    fn post_pass_correction_recovers_without_false_corrections() {
        let report = run_harness(|raw, pool| {
            let pool_vec: Vec<String> = pool.iter().map(|s| (*s).to_string()).collect();
            crate::name_correction::correct_names(raw, &pool_vec).0
        });
        eprintln!("NAME-CORRECTION HARNESS REPORT: {report:?}");
        assert_eq!(
            report.false_corrections, 0,
            "false corrections must be zero: {report:?}"
        );
        assert_eq!(report.structural_mismatches, 0, "{report:?}");
        // With name-position context, all 6 targets are recovered: the
        // same-first-letter / accent cases plus the harder different-first-letter
        // (Geert<-Bert, Xiulan<-shulan) and short-token (Thanh<-tan) cases, which
        // the surrounding syntax (address cues / name-verbs) confirms as names.
        assert_eq!(
            report.recovered, 6,
            "expected to recover all 6 corpus targets: {report:?}"
        );
    }

    /// A correction that blindly title-cases and swaps any pool name in would
    /// recover names but corrupt negatives. This asserts the scorer actually
    /// catches false corrections (guards the gating metric itself).
    #[test]
    fn scorer_flags_false_corrections() {
        // Pathological "corrector": replace the first lowercase token with the
        // first pool name, every case. Recovers some targets but corrupts the
        // negatives where the first lowercase token was already correct.
        let report = run_harness(|raw, pool| {
            let Some(name) = pool.first() else {
                return raw.to_string();
            };
            let mut out: Vec<String> = raw.split_whitespace().map(str::to_string).collect();
            if let Some(slot) = out
                .iter_mut()
                .find(|t| t.chars().next().is_some_and(|c| c.is_lowercase()))
            {
                *slot = (*name).to_string();
            }
            out.join(" ")
        });
        assert!(
            report.false_corrections > 0,
            "scorer must detect that the pathological corrector corrupts tokens"
        );
    }
}
