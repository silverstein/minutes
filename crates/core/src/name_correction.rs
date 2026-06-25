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
//! Two tiers of confidence:
//! - **Out of name-position** (no syntactic name cue around the token): only
//!   accent restoration and bounded edit-distance with a corroborating signal
//!   (same first letter OR matching Double Metaphone) and a minimum length.
//!   This is the conservative tier that protects common words like `mark`.
//! - **In name-position** (preceded by an address cue like `thanks`/`to`/`merci`
//!   or followed by a name-verb like `will`/`owns`): the surrounding syntax
//!   confirms the token is a person name, so the first-letter / DM / min-length
//!   gates are relaxed and a unique pool name within 2 edits wins. This is what
//!   safely recovers the harder different-first-letter (`Geert`<-`bert`) and
//!   short-token (`Thanh`<-`tan`) cases. The edit-distance budget and
//!   unique-winner requirement always hold, so a token far from any pool name
//!   is never touched, in or out of name-position.

use rphonetic::{DoubleMetaphone, Encoder};

/// Minimum token length eligible for a non-accent (misspelling) correction.
/// Short tokens (`tan`, `mark`) collide with common words and real names too
/// easily without speaker-turn context, so v1 does not touch them.
const MIN_MISSPELL_LEN: usize = 4;

/// A single applied correction, surfaced for frontmatter provenance so the
/// rewrite is auditable and reversible. The raw token is always preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameCorrection {
    /// The token as transcribed.
    pub raw: String,
    /// The pool spelling it was rewritten to.
    pub corrected: String,
}

struct PoolEntry {
    /// Canonical surface form (properly cased/accented), what we rewrite to.
    surface: String,
    /// Lowercased, accent-folded form for distance + accent comparison.
    norm: String,
    /// Double Metaphone primary code of the surface form.
    dm: String,
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

/// Distance budget for a normalized token of the given length. Strict: 1 edit
/// for short names, 2 for longer ones.
fn distance_budget(len: usize) -> usize {
    if len >= 6 {
        2
    } else {
        1
    }
}

fn build_pool(pool: &[String]) -> Vec<PoolEntry> {
    let dm = DoubleMetaphone::default();
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
            Some(PoolEntry {
                surface: surface.to_string(),
                dm: dm.encode(surface),
                norm,
            })
        })
        .collect()
}

/// Words that, immediately before a token, mark it as a person being addressed
/// or referenced (vocative / dative slots). Multilingual to match the
/// multilingual name target (merci/gracias/etc.).
const ADDRESS_CUES: &[&str] = &[
    "thanks", "thank", "hi", "hey", "hello", "to", "from", "with", "cc", "ping", "dear", "for",
    "merci", "gracias", "hola", "bonjour", "ciao", "over",
];

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
/// `name_position` relaxes the misspelling gate (drops the first-letter / DM /
/// min-length requirements) because the surrounding syntax already confirms the
/// token is a person name; the edit-distance budget and unique-winner
/// requirement still apply, so a token far from any pool name is never touched.
fn match_token(
    token: &str,
    name_position: bool,
    dm: &DoubleMetaphone,
    pool: &[PoolEntry],
) -> Option<String> {
    // Only consider alphabetic tokens (skip numbers, IDs, mixed tokens).
    if token.is_empty() || !token.chars().all(|c| c.is_alphabetic()) {
        return None;
    }
    let tok_norm = normalize(token);
    let tok_dm = dm.encode(token);

    let mut accent_hit: Option<&PoolEntry> = None;
    let mut accent_count = 0usize;
    let mut fuzzy_hit: Option<&PoolEntry> = None;
    let mut fuzzy_count = 0usize;

    for entry in pool {
        // Already the exact surface form, or a pure-casing variant: leave alone.
        if token == entry.surface || differs_only_by_case(token, &entry.surface) {
            return None;
        }
        if tok_norm == entry.norm {
            // Same letters, differ by accent only -> accent restoration.
            accent_hit = Some(entry);
            accent_count += 1;
            continue;
        }
        let dist = levenshtein(&tok_norm, &entry.norm);
        if dist == 0 {
            continue;
        }
        if name_position {
            // Context confirms a name: a unique pool name within 2 edits wins,
            // even across a different first letter or short length.
            if dist <= 2 {
                fuzzy_hit = Some(entry);
                fuzzy_count += 1;
            }
            continue;
        }
        // No context: require a corroborating signal (same first normalized
        // letter OR matching Double Metaphone) and a minimum length.
        if tok_norm.len() < MIN_MISSPELL_LEN
            || dist > distance_budget(tok_norm.len().max(entry.norm.len()))
        {
            continue;
        }
        let same_first = tok_norm.as_bytes().first() == entry.norm.as_bytes().first();
        let dm_match = !tok_dm.is_empty() && tok_dm == entry.dm;
        if same_first || dm_match {
            fuzzy_hit = Some(entry);
            fuzzy_count += 1;
        }
    }

    // Accent restoration is the highest-confidence path and wins when unique.
    if accent_count == 1 {
        return accent_hit.map(|e| e.surface.clone());
    }
    // Otherwise require a unique fuzzy winner; ambiguity means leave it alone.
    if accent_count == 0 && fuzzy_count == 1 {
        return fuzzy_hit.map(|e| e.surface.clone());
    }
    None
}

/// Correct person-name tokens in `text` against `pool`. Returns the corrected
/// text and the list of applied corrections (raw preserved). Non-word
/// characters (whitespace, punctuation, the `[SPEAKER m:ss]` prefix) are passed
/// through verbatim; only whole alphabetic word spans are ever rewritten.
pub fn correct_names(text: &str, pool: &[String]) -> (String, Vec<NameCorrection>) {
    let entries = build_pool(pool);
    if entries.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let dm = DoubleMetaphone::default();

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

    let mut corrections = Vec::new();
    let mut replacements: Vec<(usize, String)> = Vec::new();
    for (k, &i) in word_positions.iter().enumerate() {
        let Seg::Word(token) = &segs[i] else {
            continue;
        };
        let prev = word_at(k.checked_sub(1).and_then(|kp| word_positions.get(kp)));
        let next = word_at(word_positions.get(k + 1));
        if let Some(surface) = match_token(token, in_name_position(prev, next), &dm, &entries) {
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
}
