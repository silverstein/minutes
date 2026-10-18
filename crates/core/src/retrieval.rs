//! Literal lexical contract shared with `minutes-sdk/src/retrieval.ts`.
//! This module has no filesystem access and grants no authority to source text.
use std::sync::OnceLock;
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

pub const RETRIEVAL_CONTRACT: &str = "literal-and-prefix-v1";
pub const SEARCH_QUERY_MAX_CHARS: usize = 2048;

pub fn fold_search_text(value: &str) -> String {
    value
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn search_tokens(value: &str) -> Vec<String> {
    static TOKENS: OnceLock<regex::Regex> = OnceLock::new();
    TOKENS
        .get_or_init(|| regex::Regex::new(r"[\p{L}\p{N}]+").expect("literal token regex"))
        .find_iter(&fold_search_text(value))
        .map(|token| token.as_str().to_string())
        .collect()
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
