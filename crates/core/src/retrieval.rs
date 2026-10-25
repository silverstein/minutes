//! Literal lexical contract shared with `minutes-sdk/src/retrieval.ts`.
//! This module has no filesystem access and grants no authority to source text.
use std::sync::OnceLock;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

pub const RETRIEVAL_CONTRACT: &str = "literal-and-prefix-v1";
pub const SEARCH_QUERY_MAX_CHARS: usize = 2048;

/// NFD, drop combining marks, lowercase. Search folds every corpus file on
/// every query, so ASCII (most meeting text) takes a direct lowercase path.
/// That is exact, not approximate: folding is per character (NFD only
/// reorders combining marks, and every one is dropped), and NFD and
/// lowercasing of an ASCII character are the ASCII lowercase.
pub fn fold_search_text(value: &str) -> String {
    if value.is_ascii() {
        return value.to_ascii_lowercase();
    }
    let mut folded = String::with_capacity(value.len());
    for c in value.chars() {
        if c.is_ascii() {
            folded.push(c.to_ascii_lowercase());
        } else {
            folded.extend(
                std::iter::once(c)
                    .nfd()
                    .filter(|c| !is_combining_mark(*c))
                    .flat_map(char::to_lowercase),
            );
        }
    }
    folded
}

/// Maximal runs of `\p{L}` / `\p{N}` in the folded text. ASCII letters and
/// digits are exactly `[a-z0-9]` once folded; other characters are classified
/// by the same Unicode property regex as before, one character at a time.
pub fn search_tokens(value: &str) -> Vec<String> {
    static TOKEN_CHAR: OnceLock<regex::Regex> = OnceLock::new();
    let token_char = TOKEN_CHAR
        .get_or_init(|| regex::Regex::new(r"^[\p{L}\p{N}]$").expect("literal token-char regex"));
    let folded = fold_search_text(value);
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    let mut buf = [0u8; 4];
    for (index, c) in folded.char_indices() {
        let in_token = if c.is_ascii() {
            c.is_ascii_alphanumeric()
        } else {
            token_char.is_match(c.encode_utf8(&mut buf))
        };
        match (in_token, start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                tokens.push(folded[from..index].to_string());
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        tokens.push(folded[from..].to_string());
    }
    tokens
}

pub fn query_is_bounded(query: &str) -> bool {
    query
        .encode_utf16()
        .take(SEARCH_QUERY_MAX_CHARS + 1)
        .count()
        <= SEARCH_QUERY_MAX_CHARS
}

pub fn token_matches(token: &str, wanted: &str, final_token: bool) -> bool {
    if final_token && wanted.chars().count() >= 2 {
        token.starts_with(wanted)
    } else {
        token == wanted
    }
}

/// AND across fields; only the final multi-character token is a prefix.
/// The score is lexical relevance, never a confidence/probability.
pub fn lexical_score(title: &str, body: &str, query: &str) -> Option<f64> {
    if !query_is_bounded(query) {
        return None;
    }
    let wanted = search_tokens(query);
    if wanted.is_empty() {
        return None;
    }
    let title_tokens = search_tokens(title);
    let body_tokens = search_tokens(body);
    let mut score = 0.0;
    for (index, term) in wanted.iter().enumerate() {
        let matches = |token: &&String| token_matches(token, term, index + 1 == wanted.len());
        let title_count = title_tokens.iter().filter(matches).count();
        let body_count = body_tokens.iter().filter(matches).count();
        if title_count + body_count == 0 {
            return None;
        }
        score += 4.0 * (1.0 + title_count as f64).log2() + (1.0 + body_count as f64).log2();
    }
    let phrase = wanted.join(" ");
    if title_tokens.join(" ").contains(&phrase) {
        score += 4.0;
    }
    if body_tokens.join(" ").contains(&phrase) {
        score += 1.0;
    }
    Some(score)
}

/// Relevance tier, compared before [`lexical_score`]. The score alone let a
/// long transcript that merely contains the words (hundreds of `me*` hits)
/// outrank a meeting whose title starts with the exact phrase typed:
/// 3 = phrase in the title, 2 = every term in the title, 1 = phrase in the
/// body, 0 = terms scattered. Phrases match token by token (the final term
/// as a prefix, like the match itself), not by substring.
pub fn lexical_tier(title: &str, body: &str, query: &str) -> u8 {
    if !query_is_bounded(query) {
        return 0;
    }
    let wanted = search_tokens(query);
    if wanted.is_empty() {
        return 0;
    }
    let title_tokens = search_tokens(title);
    if contains_phrase(&title_tokens, &wanted) {
        return 3;
    }
    let last = wanted.len() - 1;
    if wanted.iter().enumerate().all(|(index, term)| {
        title_tokens
            .iter()
            .any(|token| tier_token_matches(token, term, index == last))
    }) {
        return 2;
    }
    if contains_phrase(&search_tokens(body), &wanted) {
        return 1;
    }
    0
}

fn contains_phrase(tokens: &[String], wanted: &[String]) -> bool {
    if wanted.is_empty() || tokens.len() < wanted.len() {
        return false;
    }
    let last = wanted.len() - 1;
    tokens.windows(wanted.len()).any(|window| {
        window
            .iter()
            .zip(wanted)
            .enumerate()
            .all(|(index, (token, term))| tier_token_matches(token, term, index == last))
    })
}

/// Ranking-only leniency: "harness" ranks a *Harnesses* title like the full
/// text match does (the FTS projection stems; the literal match does not).
fn tier_token_matches(token: &str, wanted: &str, final_token: bool) -> bool {
    token_matches(token, wanted, final_token)
        || token
            .strip_prefix(wanted)
            .is_some_and(|rest| rest == "s" || rest == "es")
}

/// Function words that make a typed query fail or rank by noise ("planning a
/// ch" required the word "a"). Folded forms; see [`content_query`].
pub const SEARCH_STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "at", "be", "by", "for", "from", "in", "is", "it", "of", "on", "or",
    "our", "the", "this", "that", "to", "was", "with", "my", "your",
];

/// The query a person means: stopwords are dropped except the final token
/// (still being typed, and a prefix) and except when nothing else would remain.
/// Returns folded tokens joined by spaces, which tokenize back identically.
pub fn content_query(query: &str) -> String {
    let tokens = search_tokens(query);
    let last = tokens.len().saturating_sub(1);
    let kept: Vec<&str> = tokens
        .iter()
        .enumerate()
        .filter(|(index, token)| *index == last || !SEARCH_STOPWORDS.contains(&token.as_str()))
        .map(|(_, token)| token.as_str())
        .collect();
    if kept.iter().all(|token| SEARCH_STOPWORDS.contains(token)) {
        return tokens.join(" ");
    }
    kept.join(" ")
}

/// Fallback queries when nothing matches every term: each leaves out one
/// term, so one wrong or misremembered word no longer means zero results.
/// The final token stays a prefix only when it is still last.
pub fn relaxed_queries(query: &str) -> Vec<String> {
    let tokens = search_tokens(query);
    if !(2..=8).contains(&tokens.len()) {
        return Vec::new();
    }
    (0..tokens.len())
        .map(|skip| {
            tokens
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != skip)
                .map(|(_, token)| token.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Select a bounded original-text excerpt only after the full match succeeds.
pub fn lexical_snippet(title: &str, body: &str, query: &str) -> Option<String> {
    lexical_score(title, body, query)?;
    let wanted = search_tokens(query);
    let line = body
        .lines()
        .find(|line| {
            search_tokens(line).iter().any(|token| {
                wanted
                    .iter()
                    .enumerate()
                    .any(|(i, term)| token_matches(token, term, i + 1 == wanted.len()))
            })
        })
        .unwrap_or_else(|| {
            body.lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or(title)
        });
    let mut snippet: String = line.chars().take(320).collect();
    if line.chars().count() > 320 {
        snippet.push('…');
    }
    Some(snippet)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The original implementations, kept as the reference the fast paths
    // must reproduce exactly (this is a contract shared with the SDK).
    fn reference_fold(value: &str) -> String {
        value
            .nfd()
            .filter(|c| !is_combining_mark(*c))
            .flat_map(char::to_lowercase)
            .collect()
    }

    fn reference_tokens(value: &str) -> Vec<String> {
        regex::Regex::new(r"[\p{L}\p{N}]+")
            .unwrap()
            .find_iter(&reference_fold(value))
            .map(|token| token.as_str().to_string())
            .collect()
    }

    #[test]
    fn tiers_put_the_typed_title_phrase_above_scattered_transcript_words() {
        let chatty = "let me show you. also the media plan. let me know. ".repeat(200);
        let query = "also let me";
        // The reported case: the long transcript scores higher on raw counts.
        assert!(
            lexical_score("gg media", &chatty, query).unwrap()
                > lexical_score(
                    "Also let me know if or when there's time",
                    "short note",
                    query
                )
                .unwrap()
        );
        // The tier fixes the order regardless of counts.
        assert_eq!(
            lexical_tier(
                "Also let me know if or when there's time",
                "short note",
                query
            ),
            3
        );
        assert_eq!(lexical_tier("gg media", &chatty, query), 0);
        assert_eq!(lexical_tier("Me, also: let it be", "", query), 2);
        assert_eq!(lexical_tier("Notes", "and also let me explain", query), 1);
        // Token-wise, not substring: "malso" must not satisfy "also".
        assert_eq!(lexical_tier("Malso let me", "", query), 0);
        // Final term is a prefix, like the match itself.
        assert_eq!(lexical_tier("Also let media go", "", query), 3);
    }

    #[test]
    fn plural_title_words_rank_like_the_stemmed_full_text_match() {
        let title = "Planning Chaos Harnesses for Codex";
        assert_eq!(lexical_tier(title, "", "chaos harness codex"), 2);
        assert_eq!(lexical_tier(title, "", "chaos harness for"), 3);
        assert_eq!(lexical_tier("Harnessing Chaos", "", "harness chaos"), 0);
    }

    #[test]
    fn stopwords_are_optional_unless_they_are_all_there_is() {
        // Reported: "planning a ch" while typing "Planning Chaos Harnesses".
        assert_eq!(content_query("Planning a ch"), "planning ch");
        let title = "Planning Chaos Harnesses for Codex and Claude Code";
        assert_eq!(lexical_tier(title, "", &content_query("planning a ch")), 3);
        assert_eq!(
            lexical_tier(title, "", &content_query("planning a chaos harness")),
            3
        );
        // The final token is still being typed, so it stays (a prefix).
        assert_eq!(content_query("pricing for"), "pricing for");
        assert_eq!(content_query("The Office"), "office");
        assert_eq!(content_query("to be or"), "to be or");
        assert_eq!(content_query("a"), "a");
    }

    #[test]
    fn relaxed_queries_leave_out_one_term_each() {
        assert_eq!(
            relaxed_queries("planning chaos desktop"),
            vec!["chaos desktop", "planning desktop", "planning chaos"]
        );
        assert!(relaxed_queries("planning").is_empty());
        assert!(relaxed_queries("a b c d e f g h i").is_empty());
    }

    #[test]
    fn fast_fold_and_tokens_match_the_reference_exactly() {
        let samples = [
            "",
            "Plain ASCII meeting about Q3 pricing, v2.0 and 50% off!",
            "Zürich café naïve résumé coöperate",
            "İstanbul ISTANBUL ıi",
            "Straße STRASSE ß ẞ",
            "ΟΔΥΣΣΕΥΣ Ὀδυσσεύς σοφός",
            "한국어 회의록 Hangul 가",
            "会議の議事録 中文会议 2026年",
            "e\u{301}\u{323}a\u{323}\u{301} combining order",
            "ﬁnance ﬀ ligatures ①②③ ¹²³ ½ Ⅻ",
            "ＡＢＣ１２３ fullwidth",
            "emoji 🎙️ mid👍word smart “quotes” — dashes…",
            "\u{0301}leading combining and trailing e\u{0301}",
            "tabs\tnew\nlines\r\n and   spaces",
            "Mixed: Řehoř met O'Brien at 9:30am re: Āmbient ĀI",
            "Devanagari हिन्दी Thai ภาษาไทย Arabic العربية",
        ];
        for sample in samples {
            assert_eq!(
                fold_search_text(sample),
                reference_fold(sample),
                "fold {sample:?}"
            );
            assert_eq!(
                search_tokens(sample),
                reference_tokens(sample),
                "tokens {sample:?}"
            );
        }
        // Every BMP-plus-some code point, in small mixed runs.
        let mut chunk = String::new();
        for code in (0u32..0x3_0000).step_by(7) {
            if let Some(c) = char::from_u32(code) {
                chunk.push(c);
                if code % 3 == 0 {
                    chunk.push('a');
                }
            }
            if chunk.len() > 512 {
                assert_eq!(fold_search_text(&chunk), reference_fold(&chunk));
                assert_eq!(search_tokens(&chunk), reference_tokens(&chunk));
                chunk.clear();
            }
        }
    }
    #[test]
    fn shared_literal_contract() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/search-contract-v1.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let actual = lexical_score(
                case["title"].as_str().unwrap(),
                case["body"].as_str().unwrap(),
                case["query"].as_str().unwrap(),
            );
            assert_eq!(
                actual.is_some(),
                case["matches"].as_bool().unwrap(),
                "{}",
                case["id"]
            );
            if let Some(expected) = case["score"].as_f64() {
                assert!((actual.unwrap() - expected).abs() < 1e-9, "{}", case["id"]);
            }
        }
    }
    #[test]
    fn bounded_unicode_query_and_original_excerpt() {
        assert!(!query_is_bounded(&"😀".repeat(1025)));
        assert_eq!(
            lexical_snippet("", "[0:12] Café costs $50.", "cafe"),
            Some("[0:12] Café costs $50.".into())
        );
        assert!(lexical_score("running", "", "runs").is_none());
    }
}
