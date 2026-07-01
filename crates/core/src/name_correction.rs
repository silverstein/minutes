//! Post-pass name correction (config-gated, off by default).
//!
//! The "big lever" of the name-accuracy epic (bead minutes-25x3.4): after
//! transcription, fuzzy/phonetic-match person-name tokens against the
//! expected-name pool (calendar attendees, identity, vocabulary) and rewrite
//! clear mis-transcriptions to the correct spelling. Measured against the
//! text-level harness in `name_eval` (`docs/plans/every-name-right-2026-06-11.md`).
//!
//! Design philosophy: **wrong corrections are worse than wrong
//! transcriptions.** Every gate below favors leaving a token untouched over a
//! risky rewrite, and corrections are returned with the raw token preserved so
//! the pipeline can record provenance (never a silent rewrite). The pass is
//! config-gated and off by default.
//!
//! Both fuzzy tiers require the target name to be a confirmed meeting
//! participant (an attendee or a High-confidence attributed speaker) and refuse
//! to rewrite a common English word. Correcting only toward this meeting's
//! people, never a cross-vault vocabulary/graph name, plus the common-word gate,
//! is what stops ordinary words from becoming coincidental look-alike names
//! (#388: `work`->`York`, `crate`->`Cate`, `high`->`Hugh`). Accent restoration
//! (same letters, e.g. `monica`->`Mónica`) is the only path open to any pool
//! name.
//!
//! Two tiers of confidence (both participant-gated, both common-word-gated):
//! - **Out of name-position** (no syntactic name cue around the token): a
//!   single-edit same-first-letter misspelling of a participant name at least
//!   `MIN_MISSPELL_LEN` long. The conservative tier that protects common words
//!   like `mark`/`work`/`high`.
//! - **In name-position** (preceded by an address cue like `thanks`/`merci`, or
//!   followed by a name-verb like `will`/`owns`): the first-letter / length
//!   gates relax and a unique participant within 2 edits wins. This safely
//!   recovers the harder different-first-letter (`Geert`<-`bert`) and short-token
//!   (`Thanh`<-`tan`) cases.
//!
//! The edit-distance budget and unique-winner requirement always hold, so a
//! token far from any pool name is never touched, in or out of name-position.

use serde::{Deserialize, Serialize};

/// Minimum token length eligible for a non-accent (misspelling) correction.
/// Short tokens (`tan`, `mark`) collide with common words and real names too
/// easily without speaker-turn context, so v1 does not touch them.
const MIN_MISSPELL_LEN: usize = 4;

/// A single applied correction, surfaced for frontmatter provenance so the
/// rewrite is auditable and reversible. The raw token is always preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NameCorrection {
    /// The token as transcribed.
    pub raw: String,
    /// The pool spelling it was rewritten to.
    pub corrected: String,
}

pub fn build_name_pool(
    attendees: &[String],
    identity: Option<&crate::config::IdentityConfig>,
    vocabulary: Option<&crate::vocabulary::VocabularyStore>,
) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(identity) = identity {
        if let Some(name) = identity.name.as_ref() {
            candidates.push(name.clone());
        }
        candidates.extend(identity.aliases.iter().cloned());
    }
    candidates.extend(attendees.iter().cloned());
    if let Some(vocabulary) = vocabulary {
        candidates.extend(vocabulary.decode_phrases(8));
    }

    let mut names = Vec::new();
    for token in candidates
        .iter()
        .flat_map(|candidate| candidate.split_whitespace())
        .map(str::trim)
        .filter(|token| token.chars().all(|c| c.is_alphabetic()))
        .filter(|token| token.chars().count() >= 2)
        // A pool entry that is itself a common word (e.g. "The"/"Team" from a
        // "The Team" attendee, or a stopword-like vocabulary term) would turn
        // ordinary words into correction targets, so keep them out of the pool.
        .filter(|token| !is_stopword(&normalize(token)))
    {
        if !names.iter().any(|name| name == token) {
            names.push(token.to_string());
        }
    }
    names
}

struct PoolEntry {
    /// Canonical surface form (properly cased/accented), what we rewrite to.
    surface: String,
    /// Lowercased, accent-folded form for distance + accent comparison.
    norm: String,
    /// True when this name is a confirmed meeting participant (an attendee or a
    /// High-confidence attributed speaker). Both fuzzy tiers require a
    /// participant target so a token is never rewritten toward a name that is
    /// merely in the cross-vault vocabulary/graph but not in this meeting (#388).
    /// Accent restoration (same letters) is the only path open to any pool name.
    is_participant: bool,
}

/// Fold common Latin accented characters to ASCII (Mónica -> monica). Covers
/// the Latin-1 / Latin-Extended vowels plus ñ/ç that appear in European names;
/// non-Latin romanizations (e.g. Xiulan) have no accents to fold.
fn fold_char(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

/// Lowercase + accent-fold for comparison.
fn normalize(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(fold_char)
        .collect()
}

/// True when `token` equals `surface` ignoring case but NOT ignoring accents.
/// This is the pure-casing guard: `mark` vs `Mark` is case-only (skip, it is a
/// common word or already fine), whereas `monica` vs `Mónica` differs by accent
/// (a real restoration target).
fn differs_only_by_case(token: &str, surface: &str) -> bool {
    token != surface && token.to_lowercase() == surface.to_lowercase()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn build_pool(
    pool: &[String],
    participant_norms: &std::collections::HashSet<String>,
) -> Vec<PoolEntry> {
    pool.iter()
        .filter_map(|name| {
            let surface = name.trim();
            // Single-word names only in v1: multi-word handling (and matching a
            // surname token against a full name) is its own design.
            if surface.is_empty() || surface.split_whitespace().count() != 1 {
                return None;
            }
            let norm = normalize(surface);
            if norm.is_empty() {
                return None;
            }
            let is_participant = participant_norms.contains(&norm);
            Some(PoolEntry {
                surface: surface.to_string(),
                is_participant,
                norm,
            })
        })
        .collect()
}

/// Words that, immediately before a token, mark it as a person being addressed
/// or referenced (vocative / dative slots). Multilingual to match the
/// multilingual name target (merci/gracias/etc.).
const ADDRESS_CUES: &[&str] = &[
    // Strong vocatives only. High-frequency prepositions (to/for/with/from/over/cc)
    // are deliberately excluded: they dominate ordinary prepositional phrases, so
    // they would turn "to go over" into a name slot. Names after a preposition are
    // still corrected via the normal same-first-letter / accent path.
    "thanks", "thank", "hi", "hey", "hello", "dear", "ping", "merci", "gracias", "hola", "bonjour",
    "ciao",
];

/// Common function words / pronouns / auxiliaries that must never be rewritten
/// to a name and must never enter the pool, even in a name-position. They are
/// the dominant collision risk for short pool names (e.g. `we`->`Wei`,
/// `all`->`Al`, `them`->`Team`, `go`->`Jo`, `well`->`Will`).
const STOPWORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "and",
    "or",
    "but",
    "so",
    "as",
    "at",
    "by",
    "for",
    "from",
    "in",
    "into",
    "of",
    "off",
    "on",
    "onto",
    "to",
    "too",
    "up",
    "down",
    "out",
    "over",
    "under",
    "with",
    "via",
    "per",
    "vs",
    "we",
    "you",
    "your",
    "yours",
    "i",
    "me",
    "my",
    "mine",
    "he",
    "him",
    "his",
    "she",
    "her",
    "hers",
    "it",
    "its",
    "they",
    "them",
    "their",
    "theirs",
    "this",
    "that",
    "these",
    "those",
    "here",
    "there",
    "then",
    "than",
    "is",
    "am",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "do",
    "did",
    "does",
    "done",
    "has",
    "had",
    "have",
    "will",
    "would",
    "can",
    "could",
    "should",
    "may",
    "might",
    "must",
    "shall",
    "go",
    "got",
    "get",
    "well",
    "yes",
    "no",
    "not",
    "now",
    "new",
    "one",
    "two",
    "who",
    "why",
    "how",
    "what",
    "when",
    "where",
    "ok",
    "okay",
    "just",
    "like",
    "also",
    "more",
    "most",
    "some",
    "any",
    "all",
    "each",
    "even",
    "only",
    "very",
    "much",
    "many",
    "few",
    "our",
    "ours",
    "us",
    "if",
    "else",
    "about",
    "after",
    "before",
    "again",
    // Collective / role nouns that arrive via multi-word attendee fields ("The
    // Team", "Product Group") and would otherwise become correction targets.
    "team",
    "group",
    "staff",
    "board",
    "crew",
    "panel",
    "folks",
    "everyone",
    "everybody",
    "guys",
    // Days and months: high-frequency words a short edit from many names.
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "january",
    "february",
    "march",
    "april",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    // Common English words that double as names; too collision-prone to correct
    // without stronger context (Bill<->Will, June<->Jane, Mark<->Marc, etc.).
    "mark",
    "bill",
    "art",
    "grace",
    "hope",
    "min",
    "rose",
    "dawn",
    "sunny",
    "drew",
    "sun",
];

/// True when the normalized token is a common word that must never be corrected.
fn is_stopword(norm: &str) -> bool {
    STOPWORDS.contains(&norm)
}

/// True when `norm` (already lowercase + accent-folded) is a common English
/// word. The conservative correction tier refuses to rewrite these so ordinary
/// words are never "corrected" into coincidental look-alike names (#388). The
/// list is a bundled set of common 4+ character words; shorter tokens are
/// already excluded by `MIN_MISSPELL_LEN`.
fn is_common_word(norm: &str) -> bool {
    use std::collections::HashSet;
    use std::sync::OnceLock;
    static WORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    let set = WORDS.get_or_init(|| {
        include_str!("data/common_words.txt")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect()
    });
    set.contains(norm)
}

/// Words that, immediately after a token, mark it as a person-subject.
const NAME_VERB_CUES: &[&str] = &[
    "will",
    "owns",
    "said",
    "says",
    "asked",
    "mentioned",
    "presented",
    "joined",
    "leads",
    "wants",
    "needs",
    "added",
    "noted",
    "agreed",
    "owns",
    "owned",
    "presents",
];

/// True when the token sits in a grammatical slot strongly associated with a
/// person name: preceded by an address cue, or followed by a name-verb. This is
/// the lightweight "context" signal (the plan's speaker-turn-context goal,
/// realized syntactically with no NLP dependency) that lets us safely correct
/// the harder different-first-letter / short-token cases.
fn in_name_position(prev_word: Option<&str>, next_word: Option<&str>) -> bool {
    let prev_hit = prev_word
        .map(normalize)
        .is_some_and(|w| ADDRESS_CUES.contains(&w.as_str()));
    let next_hit = next_word
        .map(normalize)
        .is_some_and(|w| NAME_VERB_CUES.contains(&w.as_str()));
    prev_hit || next_hit
}

/// Decide the correction for a single word token, or `None` to leave it alone.
/// `name_position` relaxes the misspelling gate (drops the prefix / min-length
/// requirements) because the surrounding syntax already confirms the token is a
/// person name; the edit-distance budget and unique-winner requirement still
/// apply, so a token far from any pool name is never touched.
fn match_token(token: &str, name_position: bool, pool: &[PoolEntry]) -> Option<String> {
    // Only consider alphabetic tokens (skip numbers, IDs, mixed tokens).
    if token.is_empty() || !token.chars().all(|c| c.is_alphabetic()) {
        return None;
    }
    let tok_norm = normalize(token);
    // Never rewrite a common function word / pronoun, in or out of name-position.
    if is_stopword(&tok_norm) {
        return None;
    }

    // Collect DISTINCT candidate pool entries (accent restoration OR fuzzy). A
    // correction fires only when exactly one pool name is a candidate, so an
    // accent match is suppressed when another name is also fuzzy-close (and
    // vice versa) -- ambiguity always means leave it alone.
    let mut candidate: Option<&PoolEntry> = None;
    let mut candidate_count = 0usize;

    for entry in pool {
        // Already the exact surface form, or a pure-casing variant: leave alone.
        if token == entry.surface || differs_only_by_case(token, &entry.surface) {
            return None;
        }
        let is_candidate = if tok_norm == entry.norm {
            // Same letters, differ by accent only -> accent restoration.
            true
        } else {
            let dist = levenshtein(&tok_norm, &entry.norm);
            if dist == 0 {
                false
            } else {
                let ascii = tok_norm.is_ascii() && entry.norm.is_ascii();
                // Both fuzzy tiers require (a) the target be a confirmed meeting
                // PARTICIPANT (attendee or High-confidence attributed speaker),
                // never a name merely in the cross-vault vocabulary/graph, and
                // (b) the raw token NOT be a common English word. (a) removes the
                // class where an ordinary word maps to a look-alike name from
                // another meeting (#388: work->York, crate->Cate, merge->Marge);
                // (b) removes the class where an ordinary word maps to a
                // participant name (#388: high->Hugh, line->Life, moat->Mat).
                // Real misspellings (ashwarya->Aishwarya, jacque->Jacques) are
                // not common words, so they still correct.
                let is_common = is_common_word(&tok_norm);
                // Relaxed (name-position) tier: a participant within 2 edits,
                // even across a different first letter / short length, when
                // surrounding syntax marks the token as a name.
                let relaxed = name_position
                    && entry.is_participant
                    && tok_norm.len() >= 3
                    && dist <= 2
                    && !is_common;
                // Conservative (no-context) tier: a single-edit, same-first-letter
                // misspelling of a participant name at least MIN_MISSPELL_LEN long.
                let same_first = tok_norm.as_bytes().first() == entry.norm.as_bytes().first();
                let conservative = entry.is_participant
                    && tok_norm.len() >= MIN_MISSPELL_LEN
                    && entry.norm.len() >= MIN_MISSPELL_LEN
                    && dist <= 1
                    && same_first
                    && !is_common;
                ascii && (relaxed || conservative)
            }
        };
        if is_candidate {
            candidate = Some(entry);
            candidate_count += 1;
        }
    }

    if candidate_count == 1 {
        candidate.map(|e| e.surface.clone())
    } else {
        None
    }
}

/// Correct person-name tokens in `text` against `pool`. Returns the corrected
/// text and the list of applied corrections (raw preserved). Non-word
/// characters (whitespace, punctuation, the `[SPEAKER m:ss]` prefix) are passed
/// through verbatim; only whole alphabetic word spans are ever rewritten.
/// Correct names treating every pool name as a confirmed participant (no
/// participant-gating). Used by tests and the eval harness; the pipeline uses
/// [`correct_names_with_participants`] with the real participant set.
pub fn correct_names(text: &str, pool: &[String]) -> (String, Vec<NameCorrection>) {
    correct_names_with_participants(text, pool, pool)
}

/// Correct names, gating the aggressive relaxed (name-position) tier to the
/// `participants` set (attendees + High-confidence attributed speakers). The
/// conservative tier (accent / same-first-letter) still uses the full `pool`.
pub fn correct_names_with_participants(
    text: &str,
    pool: &[String],
    participants: &[String],
) -> (String, Vec<NameCorrection>) {
    // Confirmed-participant names (attendees + High-confidence attributed
    // speakers), normalized, gate the aggressive relaxed tier.
    let participant_norms: std::collections::HashSet<String> = participants
        .iter()
        .flat_map(|p| p.split_whitespace())
        .map(normalize)
        .filter(|n| !n.is_empty())
        .collect();
    let entries = build_pool(pool, &participant_norms);
    if entries.is_empty() {
        return (text.to_string(), Vec::new());
    }

    // Tokenize into alternating word / non-word segments, preserving everything
    // (whitespace, punctuation, the `[SPEAKER m:ss]` prefix) so only whole
    // alphabetic word spans are ever rewritten and structure is byte-preserved.
    enum Seg {
        Word(String),
        Other(String),
    }
    let mut segs: Vec<Seg> = Vec::new();
    let mut cur = String::new();
    let mut cur_is_word = false;
    for c in text.chars() {
        let is_word = c.is_alphabetic();
        if !cur.is_empty() && is_word != cur_is_word {
            let taken = std::mem::take(&mut cur);
            segs.push(if cur_is_word {
                Seg::Word(taken)
            } else {
                Seg::Other(taken)
            });
        }
        cur.push(c);
        cur_is_word = is_word;
    }
    if !cur.is_empty() {
        segs.push(if cur_is_word {
            Seg::Word(cur)
        } else {
            Seg::Other(cur)
        });
    }

    // Word segment positions, for prev/next-word lookup.
    let word_positions: Vec<usize> = segs
        .iter()
        .enumerate()
        .filter_map(|(i, s)| matches!(s, Seg::Word(_)).then_some(i))
        .collect();
    let word_at = |idx: Option<&usize>| -> Option<&str> {
        idx.and_then(|&i| match &segs[i] {
            Seg::Word(w) => Some(w.as_str()),
            Seg::Other(_) => None,
        })
    };

    // Mark word segments that sit inside a `[...]` span (the `[SPEAKER_N m:ss]`
    // prefix). Those tokens are never correction candidates -- correcting
    // `SPEAKER` to a pool name would corrupt the speaker label.
    let mut bracketed = vec![false; segs.len()];
    let mut depth: i32 = 0;
    for (i, s) in segs.iter().enumerate() {
        match s {
            Seg::Other(text) => {
                for c in text.chars() {
                    match c {
                        '[' => depth += 1,
                        ']' => depth = (depth - 1).max(0),
                        _ => {}
                    }
                }
            }
            Seg::Word(_) => bracketed[i] = depth > 0,
        }
    }

    let mut corrections = Vec::new();
    let mut replacements: Vec<(usize, String)> = Vec::new();
    for (k, &i) in word_positions.iter().enumerate() {
        let Seg::Word(token) = &segs[i] else {
            continue;
        };
        if bracketed[i] {
            continue;
        }
        let prev = word_at(k.checked_sub(1).and_then(|kp| word_positions.get(kp)));
        let next = word_at(word_positions.get(k + 1));
        if let Some(surface) = match_token(token, in_name_position(prev, next), &entries) {
            corrections.push(NameCorrection {
                raw: token.clone(),
                corrected: surface.clone(),
            });
            replacements.push((i, surface));
        }
    }
    for (i, surface) in replacements {
        segs[i] = Seg::Word(surface);
    }

    let out: String = segs
        .iter()
        .map(|s| match s {
            Seg::Word(w) | Seg::Other(w) => w.as_str(),
        })
        .collect();
    (out, corrections)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn issue_388_does_not_rewrite_common_words_to_names() {
        // Real repro from the gtheys/Mat meeting: cross-vault names in the pool
        // plus participant names (Hugh, Mat) that collide with common words. Not
        // one common word may move. Covers work->York, moat->Mat, line->Life,
        // high->Hugh, ways->Way, multiple->Multiplier, happier->Harper.
        let names = pool(&["York", "Harper", "Life", "Hugh", "Mat", "Way", "Multiplier"]);
        let participants = pool(&["Hugh", "Mat"]);
        let cases = [
            "this is never going to work",
            "he builds a moat around it",
            "on the command line interface",
            "a high level of trust",
            "asked the team multiple times",
            "there are better ways to do it",
            "bobby would be happier",
        ];
        for raw in cases {
            let (out, corrections) = correct_names_with_participants(raw, &names, &participants);
            assert_eq!(
                out, raw,
                "common words must be left intact: {raw:?} -> {out:?}"
            );
            assert!(
                corrections.is_empty(),
                "no corrections expected for {raw:?}, got {corrections:?}"
            );
        }
    }

    #[test]
    fn issue_388_name_position_does_not_rewrite_common_words_to_participants() {
        // Codex review hole: the relaxed (name-position) tier must ALSO refuse
        // common words, even when the target is a participant and syntax marks a
        // name slot (a following name-verb like `will`).
        let names = pool(&["Hugh", "Mat"]);
        let participants = names.clone();
        let cases = [
            "the daily high will move lower", // high -> Hugh (participant) via next-verb `will`
            "the moat will protect it",       // moat -> Mat (participant)
        ];
        for raw in cases {
            let (out, corrections) = correct_names_with_participants(raw, &names, &participants);
            assert_eq!(
                out, raw,
                "name-position common word must be intact: {raw:?} -> {out:?}"
            );
            assert!(
                corrections.is_empty(),
                "no corrections for {raw:?}: {corrections:?}"
            );
        }
    }

    #[test]
    fn issue_388_conservative_only_targets_participants() {
        // Codex review hole: a non-common domain word one edit from a non-participant
        // vocabulary name must NOT be rewritten. `crate`->`Cate`, `merge`->`Marge`.
        let names = pool(&["Cate", "Marge"]); // in pool (vocabulary) but NOT participants
        let participants: Vec<String> = Vec::new();
        for raw in ["the rust crate failed", "merge the pull request"] {
            let (out, corrections) = correct_names_with_participants(raw, &names, &participants);
            assert_eq!(
                out, raw,
                "non-participant target must not fire: {raw:?} -> {out:?}"
            );
            assert!(
                corrections.is_empty(),
                "no corrections for {raw:?}: {corrections:?}"
            );
        }
    }

    #[test]
    fn issue_388_fix_still_corrects_real_misspellings() {
        // The common-word gate must not disable legit corrections: a same-first,
        // single-edit misspelling of a non-common name via the conservative tier
        // (no name-position cue here), out of an edit budget the old code allowed.
        let names = pool(&["Aishwarya", "Jacques"]);
        let participants = names.clone();
        let (out1, c1) =
            correct_names_with_participants("over to ashwarya now", &names, &participants);
        assert!(
            out1.contains("Aishwarya"),
            "ashwarya should still correct: {out1}"
        );
        assert!(!c1.is_empty());
        let (out2, _) =
            correct_names_with_participants("merci jacque for joining", &names, &participants);
        assert!(
            out2.contains("Jacques"),
            "jacque should still correct: {out2}"
        );
    }

    #[test]
    fn build_name_pool_collects_unique_single_name_tokens() {
        let identity = crate::config::IdentityConfig {
            name: Some("Mathieu Silverstein".into()),
            aliases: vec!["Mat".into(), "M S".into(), "J9".into()],
            ..Default::default()
        };
        let attendees = vec![
            "Sarah Chen".into(),
            "Mat".into(),
            "A".into(),
            "D4n".into(),
            "Mónica".into(),
        ];

        let pool = build_name_pool(&attendees, Some(&identity), None);

        assert_eq!(
            pool,
            vec!["Mathieu", "Silverstein", "Mat", "Sarah", "Chen", "Mónica"]
        );
    }

    #[test]
    fn restores_accent_and_records_provenance() {
        let (out, corr) = correct_names("gracias monica for the update", &pool(&["Mónica"]));
        assert_eq!(out, "gracias Mónica for the update");
        assert_eq!(corr.len(), 1);
        assert_eq!(corr[0].raw, "monica");
        assert_eq!(corr[0].corrected, "Mónica");
    }

    #[test]
    fn corrects_same_first_letter_misspelling() {
        let (out, _) = correct_names("merci jacque for joining", &pool(&["Jacques"]));
        assert_eq!(out, "merci Jacques for joining");
    }

    #[test]
    fn leaves_pure_case_common_word_alone() {
        // "mark" the word must not become the name "Mark" (case-only differs).
        let (out, corr) = correct_names("that was a good mark on the exam", &pool(&["Mark"]));
        assert_eq!(out, "that was a good mark on the exam");
        assert!(corr.is_empty());
    }

    #[test]
    fn leaves_already_correct_name_alone() {
        let (out, corr) = correct_names("hi Sarah how are you", &pool(&["Sarah"]));
        assert_eq!(out, "hi Sarah how are you");
        assert!(corr.is_empty());
    }

    #[test]
    fn does_not_touch_short_tokens_outside_name_position() {
        // "tan" with no address cue or name-verb around it stays the word "tan".
        let (out, corr) = correct_names("we got a nice tan today", &pool(&["Thanh"]));
        assert_eq!(out, "we got a nice tan today");
        assert!(corr.is_empty());
    }

    #[test]
    fn corrects_hard_cases_in_name_position() {
        // Different first letter (bert->Geert) and short (tan->Thanh) only
        // become correctable when the surrounding syntax confirms a name.
        let (out, _) = correct_names("thanks bert for the notes", &pool(&["Geert", "Sanne"]));
        assert_eq!(out, "thanks Geert for the notes");
        let (out2, _) = correct_names("tan owns the rollout", &pool(&["Thanh", "Linh"]));
        assert_eq!(out2, "Thanh owns the rollout");
    }

    #[test]
    fn name_position_is_still_distance_gated() {
        // A token in a name slot but far from every pool name is left alone:
        // context relaxes the corroboration gates, not the edit-distance budget.
        let (out, corr) = correct_names("thanks everyone for joining", &pool(&["Geert"]));
        assert_eq!(out, "thanks everyone for joining");
        assert!(corr.is_empty());
    }

    #[test]
    fn ambiguous_match_is_left_alone() {
        // Two equally-close pool names -> no unique winner -> no correction.
        let (out, corr) = correct_names("ping karan", &pool(&["Karen", "Kiran"]));
        assert_eq!(out, "ping karan");
        assert!(corr.is_empty());
    }

    #[test]
    fn preserves_punctuation_and_structure() {
        let (out, _) = correct_names("[SPEAKER_1 0:05] merci, jacque!", &pool(&["Jacques"]));
        assert_eq!(out, "[SPEAKER_1 0:05] merci, Jacques!");
    }

    #[test]
    fn empty_pool_is_a_noop() {
        let (out, corr) = correct_names("merci jacque", &pool(&[]));
        assert_eq!(out, "merci jacque");
        assert!(corr.is_empty());
    }

    // ---- regression guards for adversarial-review findings ----

    #[test]
    fn stopword_in_name_position_is_never_corrected() {
        // "we"/"all" are common words a dist <= 2 from short pool names but must
        // never be rewritten, even though the surrounding syntax is a name slot.
        let (out, corr) = correct_names("we will demo today", &pool(&["Wei", "Aki"]));
        assert_eq!(out, "we will demo today");
        assert!(corr.is_empty());
        let (out2, _) = correct_names("thanks all for joining", &pool(&["Al"]));
        assert_eq!(out2, "thanks all for joining");
    }

    #[test]
    fn speaker_prefix_is_never_corrupted() {
        // SPEAKER sits inside the [..] prefix; "will" follows it, but the bracket
        // guard keeps the label intact.
        let (out, corr) = correct_names("[SPEAKER_1 0:05] will present", &pool(&["Spencer"]));
        assert_eq!(out, "[SPEAKER_1 0:05] will present");
        assert!(corr.is_empty());
    }

    #[test]
    fn pool_keeps_real_names_drops_stopwords() {
        // Real per-token names survive; stopwords ("The") and collective nouns
        // ("Team") are dropped from the pool.
        let names = build_name_pool(
            &["Sarah Chen".to_string(), "The Team".to_string()],
            None,
            None,
        );
        assert!(names.iter().any(|n| n == "Sarah"));
        assert!(names.iter().any(|n| n == "Chen"));
        assert!(!names.iter().any(|n| n.eq_ignore_ascii_case("the")));
        assert!(!names.iter().any(|n| n.eq_ignore_ascii_case("team")));
        let (out, _) = correct_names("we did this for them today", &names);
        assert_eq!(out, "we did this for them today");
    }

    #[test]
    fn non_latin_token_is_not_fuzzy_matched_to_latin_name() {
        let (out, corr) = correct_names("thanks 王 now", &pool(&["Al"]));
        assert_eq!(out, "thanks 王 now");
        assert!(corr.is_empty());
    }

    #[test]
    fn accent_match_suppressed_when_another_name_is_also_close() {
        // "Jose" accent-matches "José" but is also 1 edit from "Jase": ambiguous,
        // so leave it alone rather than guess.
        let (out, corr) = correct_names("thanks Jose now", &pool(&["José", "Jase"]));
        assert_eq!(out, "thanks Jose now");
        assert!(corr.is_empty());
    }

    #[test]
    fn dropped_preposition_cue_does_not_open_a_name_slot() {
        // "to" is no longer an address cue, so a different-first-letter token
        // after it is not relaxed-corrected.
        let (out, corr) = correct_names("send this to bob", &pool(&["Rob"]));
        assert_eq!(out, "send this to bob");
        assert!(corr.is_empty());
    }

    #[test]
    fn collective_noun_attendee_does_not_pollute_pool() {
        // "The Team" must contribute no pool names, so "term will change" stays.
        let names = build_name_pool(&["The Team".to_string()], None, None);
        assert!(!names.iter().any(|n| n.eq_ignore_ascii_case("team")));
        let (out, corr) = correct_names("the term will change", &names);
        assert_eq!(out, "the term will change");
        assert!(corr.is_empty());
    }

    #[test]
    fn common_word_names_are_left_alone_in_name_position() {
        // Words that double as names must not be rewritten across the gap.
        let (out, _) = correct_names("Bill noted the issue", &pool(&["Will"]));
        assert_eq!(out, "Bill noted the issue");
        let (out2, _) = correct_names("June said yes", &pool(&["Jane"]));
        assert_eq!(out2, "June said yes");
    }

    #[test]
    fn two_char_token_not_corrected_in_name_position() {
        // Below the relaxed-tier length floor; "Bo" stays even after a cue.
        let (out, corr) = correct_names("thanks Bo now", &pool(&["Jo"]));
        assert_eq!(out, "thanks Bo now");
        assert!(corr.is_empty());
    }

    #[test]
    fn relaxed_tier_requires_a_confirmed_participant() {
        // "bert"->Geert is a relaxed (different-first-letter) correction. It is
        // suppressed when Geert is in the pool but NOT a confirmed participant,
        // and fires once Geert is a participant.
        let names = pool(&["Geert"]);
        let (gated, corr) =
            correct_names_with_participants("thanks bert for the notes", &names, &[]);
        assert_eq!(gated, "thanks bert for the notes");
        assert!(corr.is_empty());
        let (allowed, _) =
            correct_names_with_participants("thanks bert for the notes", &names, &names);
        assert_eq!(allowed, "thanks Geert for the notes");
    }

    #[test]
    fn correction_requires_participant_target() {
        // #388: both fuzzy tiers now require the target be a meeting participant,
        // so a token is never rewritten toward a name that is only in the
        // vocabulary/graph and not in this meeting.
        let names = pool(&["Jacques"]);
        // Not a participant -> left alone.
        let (out_np, c_np) =
            correct_names_with_participants("merci jacque for joining", &names, &[]);
        assert_eq!(out_np, "merci jacque for joining");
        assert!(c_np.is_empty());
        // Participant -> the correction fires.
        let (out_p, _) =
            correct_names_with_participants("merci jacque for joining", &names, &names);
        assert_eq!(out_p, "merci Jacques for joining");
    }
}
