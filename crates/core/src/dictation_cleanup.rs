//! Deterministic, on-device cleanup for dictated text.
//!
//! Raw ASR output reads as unfinished: lowercase sentence starts, scattered
//! filler words, no `I` capitalization. This module applies a fast, dependency-free,
//! idempotent pass that raises perceived quality without a language model, mirroring
//! what first-class dictation tools do automatically as you speak.
//!
//! The transforms are split into a safe default set (whitespace normalization,
//! conservative filler removal, sentence + `I` capitalization, custom-vocabulary
//! replacement) and two opt-in transforms that carry false-positive risk
//! (spoken-punctuation commands like saying "period", and aggressive vocabulary
//! application). An optional LLM tier ("ollama") is recognized for forward
//! compatibility but currently falls back to the deterministic rules.
//!
//! [`clean_dictation_text`] is pure: it takes already-resolved [`CleanupOptions`]
//! (including any vocabulary replacements) so it can be unit-tested without any IO.
//!
//! Known deterministic limits (no language model, so these are intentional):
//! - Single-letter abbreviations ("e.g.", "U.S.") are handled, but multi-letter ones
//!   ("etc.", "approx.") are still treated as sentence ends, so the next word gets
//!   capitalized. A sentence that genuinely ends in a single capital letter+period
//!   ("...an A. She...") will, conversely, not capitalize the next word.
//! - Spoken-punctuation mode (opt-in) consumes the literal words "period",
//!   "comma", etc. as commands, so those words cannot appear as content there.

/// Conservative filler tokens removed by default.
///
/// Deliberately tight: only unambiguous vocalized pauses. Words that double as real
/// content ("like", "so", "right", "well", "ah") are intentionally excluded so the
/// pass never eats meaning.
const FILLERS: &[&str] = &["um", "uh", "erm", "uhm", "umm", "uhh", "uhhh"];

/// Spoken-punctuation commands, longest phrase first so multi-word phrases win.
///
/// Each entry maps a spoken phrase to the literal it becomes. Newline forms are
/// emitted as `\n` / `\n\n`; the rest attach to the preceding word. Only applied
/// when [`CleanupOptions::spoken_punctuation`] is enabled, because these phrases
/// collide with ordinary words (the noun "period", "comma", etc.).
const SPOKEN_PUNCTUATION: &[(&str, &str)] = &[
    ("new paragraph", "\n\n"),
    ("new line", "\n"),
    ("exclamation point", "!"),
    ("exclamation mark", "!"),
    ("question mark", "?"),
    ("full stop", "."),
    ("semicolon", ";"),
    ("period", "."),
    ("comma", ","),
    ("colon", ":"),
];

/// English contraction suffixes after a standalone `i` (as in `i'm` -> `I'm`).
const I_CONTRACTIONS: &[&str] = &["m", "ll", "ve", "d", "re", "s"];

/// Sentence-ending punctuation.
const TERMINATORS: &[char] = &['.', '!', '?'];

/// Which cleanup strategy to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupEngine {
    /// No cleanup: return the input trimmed only.
    None,
    /// Deterministic, on-device rule-based cleanup.
    Rules,
}

impl CleanupEngine {
    /// Parse a `cleanup_engine` config string.
    ///
    /// Empty / unset defaults to [`CleanupEngine::Rules`] so dictation is polished
    /// out of the box. `"none"` / `"off"` disables cleanup. `"ollama"` (and any
    /// future LLM engine) is recognized but falls back to rules for now.
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "raw" | "disabled" => CleanupEngine::None,
            _ => CleanupEngine::Rules,
        }
    }
}

/// Resolved options for a cleanup pass. Built once per session from config plus the
/// user's vocabulary store, then reused for every utterance.
#[derive(Debug, Clone)]
pub struct CleanupOptions {
    /// Strategy to apply.
    pub engine: CleanupEngine,
    /// Remove conservative filler words ([`FILLERS`]). Default on.
    pub remove_fillers: bool,
    /// Convert spoken punctuation commands ("period", "new line"). Opt-in: collides
    /// with ordinary words, so off by default.
    pub spoken_punctuation: bool,
    /// `(surface, canonical)` replacements applied case-insensitively at word
    /// boundaries, e.g. `("gpt", "GPT")`. Sorted longest-surface-first by the
    /// constructor so multi-word phrases win over their sub-words.
    ///
    /// Applied once per cleanup pass. A replacement whose canonical contains the
    /// surface as a separate word (e.g. `("gpt", "Chat GPT")`) therefore expands on
    /// each repeated clean and is not idempotent; prefer canonicals that do not embed
    /// their own surface form.
    pub replacements: Vec<(String, String)>,
}

impl Default for CleanupOptions {
    fn default() -> Self {
        Self {
            engine: CleanupEngine::Rules,
            remove_fillers: true,
            spoken_punctuation: false,
            replacements: Vec::new(),
        }
    }
}

impl CleanupOptions {
    /// Disabled cleanup (passthrough except trimming).
    pub fn disabled() -> Self {
        Self {
            engine: CleanupEngine::None,
            ..Self::default()
        }
    }

    /// Sort `replacements` longest-surface-first and drop empty/ill-formed entries.
    /// Call after populating `replacements` so application order is deterministic.
    pub fn with_sorted_replacements(mut self) -> Self {
        self.replacements
            .retain(|(surface, _)| !surface.trim().is_empty());
        self.replacements
            .sort_by_key(|(surface, _)| std::cmp::Reverse(surface.chars().count()));
        self
    }
}

/// Clean a single dictated utterance.
///
/// Pure and idempotent: `clean(clean(x)) == clean(x)`. With [`CleanupEngine::None`]
/// the input is only trimmed.
pub fn clean_dictation_text(raw: &str, opts: &CleanupOptions) -> String {
    if opts.engine == CleanupEngine::None {
        return raw.trim().to_string();
    }

    let mut text = collapse_whitespace(raw.trim());

    if !opts.replacements.is_empty() {
        text = apply_replacements(&text, &opts.replacements);
    }
    if opts.remove_fillers {
        text = remove_fillers(&text);
    }
    if opts.spoken_punctuation {
        text = apply_spoken_punctuation(&text);
    }

    text = fix_punctuation_spacing(&text);
    if opts.spoken_punctuation {
        // Commands placed next to existing punctuation can produce ".." etc.
        text = collapse_double_punctuation(&text);
    }
    // Cap blank lines again: spoken-punctuation newlines are injected after the
    // initial collapse_whitespace, so adjacent "new paragraph" can stack.
    text = cap_blank_lines(&text);
    text = capitalize_sentences(&text);
    text = capitalize_standalone_i(&text);

    text.trim().to_string()
}

/// Collapse runs of spaces/tabs to a single space, drop line-leading whitespace, and
/// cap consecutive blank lines at one (so at most `\n\n`). Newlines are otherwise kept.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    let mut newline_run = 0usize;
    for c in s.chars() {
        if c == '\n' {
            pending_space = false;
            if newline_run < 2 {
                out.push('\n');
            }
            newline_run += 1;
        } else if c.is_whitespace() {
            if newline_run == 0 && !out.is_empty() {
                pending_space = true;
            }
        } else {
            if pending_space {
                out.push(' ');
            }
            pending_space = false;
            newline_run = 0;
            out.push(c);
        }
    }
    out
}

/// True if `c` is a word character for boundary purposes (alphanumeric or apostrophe).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\''
}

/// Opening quote/bracket characters skipped at a sentence start.
fn is_opener(c: char) -> bool {
    matches!(c, '(' | '[' | '{' | '"' | '\'' | '“' | '‘' | '«')
}

/// Closing quote/bracket characters allowed between a terminator and the next sentence.
fn is_closer(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '"' | '\'' | '”' | '’' | '»')
}

/// Boundary-aware, case-insensitive replacement of `surface` with `replacement`.
///
/// A match only counts when both edges sit on a non-word boundary, so "gpt" does not
/// match inside "gpts". Case folding is done per source character, so a length-changing
/// lowercase (e.g. Turkish `İ`) elsewhere in the string does not disable matching.
fn replace_word_ci(haystack: &str, surface: &str, replacement: &str) -> String {
    let needle: Vec<char> = surface.to_lowercase().chars().collect();
    if needle.is_empty() {
        return haystack.to_string();
    }
    let hay: Vec<char> = haystack.chars().collect();
    let mut out = String::with_capacity(haystack.len());
    let mut i = 0;
    while i < hay.len() {
        if let Some(end) = match_word_at(&hay, i, &needle) {
            let left_ok = i == 0 || !is_word_char(hay[i - 1]);
            let right_ok = end >= hay.len() || !is_word_char(hay[end]);
            if left_ok && right_ok {
                out.push_str(replacement);
                i = end;
                continue;
            }
        }
        out.push(hay[i]);
        i += 1;
    }
    out
}

/// If `needle` (already lowercased) matches `hay[start..]` under per-char lowercasing,
/// return the exclusive end index in `hay`. Returns `None` unless the match aligns
/// exactly on a source-character boundary.
fn match_word_at(hay: &[char], start: usize, needle: &[char]) -> Option<usize> {
    let mut ni = 0;
    let mut hi = start;
    while ni < needle.len() {
        let hc = *hay.get(hi)?;
        let mut produced = 0;
        for lc in hc.to_lowercase() {
            if ni >= needle.len() || lc != needle[ni] {
                return None;
            }
            ni += 1;
            produced += 1;
        }
        if produced == 0 {
            return None;
        }
        hi += 1;
    }
    Some(hi)
}

/// Apply vocabulary replacements in order (caller pre-sorts longest-first).
fn apply_replacements(s: &str, replacements: &[(String, String)]) -> String {
    let mut text = s.to_string();
    for (surface, canonical) in replacements {
        text = replace_word_ci(&text, surface, canonical);
    }
    text
}

/// Remove standalone filler words, preserving surrounding punctuation/structure.
///
/// A token is dropped only when it is a bare filler or a filler with a single trailing
/// comma ("um", "um,"). Bracketed or otherwise punctuated forms ("(um)", "um.") are
/// kept so real structure is never eaten.
fn remove_fillers(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in s.split('\n') {
        let kept: Vec<&str> = line
            .split(' ')
            .filter(|word| !is_removable_filler(word))
            .filter(|word| !word.is_empty())
            .collect();
        lines.push(kept.join(" "));
    }
    lines.join("\n")
}

/// True if `word` is a bare filler or a filler with a single trailing comma.
fn is_removable_filler(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    let core = lower.strip_suffix(',').unwrap_or(lower.as_str());
    FILLERS.contains(&core)
}

/// Replace spoken-punctuation phrases with their literal forms (opt-in). The space
/// before any attached punctuation is squeezed later in [`fix_punctuation_spacing`].
fn apply_spoken_punctuation(s: &str) -> String {
    let mut text = s.to_string();
    for (phrase, literal) in SPOKEN_PUNCTUATION {
        text = replace_word_ci(&text, phrase, literal);
    }
    text
}

/// Tidy punctuation spacing: drop spaces before `,.!?;:`, collapse repeated spaces,
/// and trim each line. (No space is inserted *after* punctuation, to avoid breaking
/// decimals and abbreviations like "1.2" and "e.g.".)
fn fix_punctuation_spacing(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in s.split('\n') {
        let mut out = String::with_capacity(line.len());
        for c in line.chars() {
            if matches!(c, ',' | '.' | '!' | '?' | ';' | ':') {
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push(c);
            } else if c == ' ' {
                if !out.ends_with(' ') && !out.is_empty() {
                    out.push(' ');
                }
            } else {
                out.push(c);
            }
        }
        lines.push(out.trim().to_string());
    }
    lines.join("\n")
}

/// Collapse a doubled terminal/comma punctuation to one, preserving `...` ellipsis.
/// Only used after spoken-punctuation, which can place a command next to existing
/// punctuation ("hello. period" -> "hello.." -> "hello.").
fn collapse_double_punctuation(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if matches!(c, ',' | '.' | '!' | '?' | ';' | ':') {
            let mut run = 1;
            while i + run < chars.len() && chars[i + run] == c {
                run += 1;
            }
            if c == '.' && run >= 3 {
                out.push_str("..."); // keep ellipsis
            } else {
                out.push(c);
            }
            i += run;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Collapse runs of 3+ newlines down to a single blank line (`\n\n`).
fn cap_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run = 0usize;
    for c in s.chars() {
        if c == '\n' {
            run += 1;
            if run <= 2 {
                out.push('\n');
            }
        } else {
            run = 0;
            out.push(c);
        }
    }
    out
}

/// True if the char before `dot_idx` is a lone alphabetic letter (its left side is a
/// non-word boundary), marking an abbreviation dot like the `g` in "e.g." or `S` in
/// "U.S." rather than a sentence end.
fn is_single_letter_before(chars: &[char], dot_idx: usize) -> bool {
    if dot_idx == 0 {
        return false;
    }
    let prev = chars[dot_idx - 1];
    if !prev.is_alphabetic() {
        return false;
    }
    dot_idx < 2 || !is_word_char(chars[dot_idx - 2])
}

/// Capitalize the first letter of the text and of each new sentence.
///
/// A new sentence begins after a terminator (`.`/`!`/`?`) that is followed by
/// whitespace or end-of-line (so "e.g." and "U.S." are not split mid-token), or after
/// a newline. Leading whitespace and opening/closing quotes/brackets are skipped. A
/// first token that already contains an uppercase letter ("iPhone", "iOS", "eBay") is
/// left untouched.
fn capitalize_sentences(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut at_start = true;
    let mut pending_end = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if at_start {
            if c.is_whitespace() || is_opener(c) || is_closer(c) {
                out.push(c);
                i += 1;
                continue; // stay at sentence start
            }
            if c.is_alphabetic() {
                let mut j = i;
                while j < chars.len() && is_word_char(chars[j]) {
                    j += 1;
                }
                let has_upper = chars[i..j].iter().any(|ch| ch.is_uppercase());
                if has_upper {
                    out.extend(&chars[i..j]); // brand-like, leave as-is
                } else {
                    out.extend(chars[i].to_uppercase());
                    out.extend(&chars[i + 1..j]);
                }
                i = j;
                at_start = false;
                continue;
            }
            // digit or other content starts the sentence
            out.push(c);
            at_start = false;
            i += 1;
            continue;
        }

        out.push(c);
        if TERMINATORS.contains(&c) {
            // A '.' after a single standalone letter ("e.g.", "U.S.") is an
            // abbreviation, not a sentence end. Otherwise a terminator ends the
            // sentence when followed by whitespace, EOL, or a closing quote/bracket.
            if c == '.' && is_single_letter_before(&chars, i) {
                pending_end = false;
            } else {
                let next = chars.get(i + 1);
                pending_end = next.is_none_or(|n| n.is_whitespace() || is_closer(*n));
            }
        } else if c == '\n' {
            at_start = true;
            pending_end = false;
        } else if pending_end && is_closer(c) {
            // allow `."` before the boundary; keep pending_end
        } else if pending_end && c.is_whitespace() {
            at_start = true;
            pending_end = false;
        } else {
            pending_end = false;
        }
        i += 1;
    }
    out
}

/// Capitalize the standalone pronoun `i` and its contractions (`i'm` -> `I'm`).
fn capitalize_standalone_i(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let left_boundary = i == 0 || !is_word_char(chars[i - 1]);
        if c == 'i' && left_boundary {
            match chars.get(i + 1).copied() {
                // bare "i"
                None => {
                    out.push('I');
                    i += 1;
                    continue;
                }
                Some(n) if !is_word_char(n) => {
                    out.push('I');
                    i += 1;
                    continue;
                }
                // "i'<suffix>" contraction
                Some('\'') => {
                    let suffix: String = chars[i + 2..]
                        .iter()
                        .take_while(|c| c.is_alphabetic())
                        .collect();
                    let after_idx = i + 2 + suffix.chars().count();
                    let after_ok = chars.get(after_idx).is_none_or(|c| !is_word_char(*c));
                    if I_CONTRACTIONS.contains(&suffix.to_ascii_lowercase().as_str()) && after_ok {
                        out.push('I');
                        i += 1;
                        continue;
                    }
                }
                _ => {}
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> CleanupOptions {
        CleanupOptions::default()
    }

    #[test]
    fn engine_parse_defaults_to_rules() {
        assert_eq!(CleanupEngine::parse(""), CleanupEngine::Rules);
        assert_eq!(CleanupEngine::parse("rules"), CleanupEngine::Rules);
        assert_eq!(CleanupEngine::parse("ollama"), CleanupEngine::Rules);
        assert_eq!(CleanupEngine::parse("none"), CleanupEngine::None);
        assert_eq!(CleanupEngine::parse(" OFF "), CleanupEngine::None);
    }

    #[test]
    fn none_engine_only_trims() {
        let opts = CleanupOptions::disabled();
        assert_eq!(
            clean_dictation_text("  hello   world  ", &opts),
            "hello   world"
        );
    }

    #[test]
    fn capitalizes_first_letter_and_sentences() {
        assert_eq!(clean_dictation_text("hello world", &rules()), "Hello world");
        assert_eq!(
            clean_dictation_text("hello. how are you? fine. great!", &rules()),
            "Hello. How are you? Fine. Great!"
        );
    }

    #[test]
    fn capitalizes_standalone_i_and_contractions() {
        assert_eq!(
            clean_dictation_text("i think i'm right and i'll go", &rules()),
            "I think I'm right and I'll go"
        );
        assert_eq!(
            clean_dictation_text("the api is fine", &rules()),
            "The api is fine"
        );
    }

    #[test]
    fn preserves_mixed_case_brands_at_sentence_start() {
        // the worst corruption class: do not upper-case the first letter of a token
        // that already carries internal capitals.
        assert_eq!(
            clean_dictation_text("iPhone is great", &rules()),
            "iPhone is great"
        );
        assert_eq!(clean_dictation_text("iOS shipped", &rules()), "iOS shipped");
        assert_eq!(clean_dictation_text("eBay works", &rules()), "eBay works");
        // but an ordinary lowercase first word is still capitalized
        assert_eq!(
            clean_dictation_text("john went home", &rules()),
            "John went home"
        );
    }

    #[test]
    fn does_not_mangle_abbreviations() {
        // terminator must be followed by whitespace to start a new sentence
        assert_eq!(
            clean_dictation_text("see e.g. the docs", &rules()),
            "See e.g. the docs"
        );
        assert_eq!(
            clean_dictation_text("the U.S. market", &rules()),
            "The U.S. market"
        );
        assert_eq!(
            clean_dictation_text("version 1.2 is out", &rules()),
            "Version 1.2 is out"
        );
    }

    #[test]
    fn capitalizes_after_leading_and_trailing_quotes() {
        assert_eq!(
            clean_dictation_text("\"hello\" she said", &rules()),
            "\"Hello\" she said"
        );
        assert_eq!(
            clean_dictation_text("he said \"hello.\" then left", &rules()),
            "He said \"hello.\" Then left"
        );
    }

    #[test]
    fn removes_conservative_fillers_only() {
        assert_eq!(
            clean_dictation_text("um i think uh we should erm ship it", &rules()),
            "I think we should ship it"
        );
        assert_eq!(
            clean_dictation_text("i like the umbrella so much", &rules()),
            "I like the umbrella so much"
        );
    }

    #[test]
    fn filler_removal_keeps_structure() {
        // bracketed / quoted filler is preserved
        assert_eq!(
            clean_dictation_text("the function (um) returns", &rules()),
            "The function (um) returns"
        );
        // filler with a trailing comma is removed cleanly
        assert_eq!(
            clean_dictation_text("um, so we ship", &rules()),
            "So we ship"
        );
    }

    #[test]
    fn filler_removal_can_be_disabled() {
        let opts = CleanupOptions {
            remove_fillers: false,
            ..rules()
        };
        assert_eq!(clean_dictation_text("um okay", &opts), "Um okay");
    }

    #[test]
    fn applies_vocabulary_replacements_case_insensitively() {
        let opts = CleanupOptions {
            replacements: vec![
                ("gpt".into(), "GPT".into()),
                ("new york".into(), "New York".into()),
            ],
            ..rules()
        }
        .with_sorted_replacements();
        assert_eq!(
            clean_dictation_text("i used gpt in new york", &opts),
            "I used GPT in New York"
        );
        // sub-word occurrence is left alone (no rewrite inside a larger token)
        assert_eq!(
            clean_dictation_text("gpts are models", &opts),
            "Gpts are models"
        );
    }

    #[test]
    fn replacements_survive_length_changing_unicode_elsewhere() {
        // a Turkish dotted capital İ anywhere must not disable all replacements
        let opts = CleanupOptions {
            replacements: vec![("gpt".into(), "GPT".into())],
            ..rules()
        }
        .with_sorted_replacements();
        assert_eq!(
            clean_dictation_text("İstanbul gpt notes", &opts),
            "İstanbul GPT notes"
        );
    }

    #[test]
    fn replacements_apply_longest_first() {
        let opts = CleanupOptions {
            replacements: vec![
                ("york".into(), "York".into()),
                ("new york".into(), "NYC".into()),
            ],
            ..rules()
        }
        .with_sorted_replacements();
        assert_eq!(clean_dictation_text("new york city", &opts), "NYC city");
    }

    #[test]
    fn spoken_punctuation_is_opt_in() {
        assert_eq!(
            clean_dictation_text("the trial period ended", &rules()),
            "The trial period ended"
        );
        let opts = CleanupOptions {
            spoken_punctuation: true,
            ..rules()
        };
        assert_eq!(
            clean_dictation_text("hello there period new line bye", &opts),
            "Hello there.\nBye"
        );
    }

    #[test]
    fn spoken_punctuation_does_not_double_existing_punctuation() {
        let opts = CleanupOptions {
            spoken_punctuation: true,
            ..rules()
        };
        assert_eq!(clean_dictation_text("hello. period", &opts), "Hello.");
    }

    #[test]
    fn fixes_punctuation_spacing() {
        assert_eq!(
            clean_dictation_text("hello , world . done", &rules()),
            "Hello, world. Done"
        );
    }

    #[test]
    fn collapses_blank_lines() {
        let opts = CleanupOptions {
            spoken_punctuation: true,
            ..rules()
        };
        // three "new paragraph" runs cannot stack beyond one blank line
        assert_eq!(
            clean_dictation_text("a new paragraph new paragraph b", &opts),
            "A\n\nB"
        );
    }

    #[test]
    fn idempotent() {
        let opts = CleanupOptions {
            spoken_punctuation: true,
            replacements: vec![("gpt".into(), "GPT".into())],
            ..rules()
        }
        .with_sorted_replacements();
        let inputs = [
            "um i think gpt is good period new paragraph next thing",
            "hello world",
            "i'm here. are you?",
            "  messy   spacing , here .  ",
            "iPhone and iOS. e.g. the docs",
            "\"hello\" world. done.",
        ];
        for input in inputs {
            let once = clean_dictation_text(input, &opts);
            let twice = clean_dictation_text(&once, &opts);
            assert_eq!(once, twice, "not idempotent for {input:?}");
        }
    }

    #[test]
    fn clean_input_is_left_alone() {
        let already = "I shipped the GPT integration. It works well.";
        assert_eq!(clean_dictation_text(already, &rules()), already);
    }

    #[test]
    fn empty_and_whitespace_inputs() {
        assert_eq!(clean_dictation_text("", &rules()), "");
        assert_eq!(clean_dictation_text("   \n  ", &rules()), "");
    }
}
