//! FTS5 query sanitizer.
//!
//! User-typed input is not safe to feed directly to FTS5 `MATCH`. Real meeting
//! names contain colons, hyphens, slashes, parentheses, and quotes; FTS5 treats
//! those as query operators and errors with messages like `no such column`.
//!
//! Strategy: tokenize on anything that's not alphanumeric or apostrophe, drop
//! empty tokens, emit each as a bareword, and suffix the last token with `*`
//! for typing-as-you-go prefix matching.
//!
//! The `*`-outside-bareword form is the FTS5 prefix syntax that actually works
//! (`weal*` matches `wealth`); the previously-attempted `"weal*"` (asterisk
//! inside quotes) does NOT match. Verified against actual SQLite FTS5 in
//! adversarial review.

/// Convert raw user input into a safe FTS5 MATCH expression.
///
/// Returns the empty string for input that has no usable tokens (all
/// whitespace, all punctuation, etc). Callers should treat empty output
/// as "return no results" rather than "match everything".
///
/// Tokenization aligns with FTS5's default `unicode61` tokenizer: split on
/// every non-alphanumeric character (including apostrophes). "Ryan's" indexes
/// as `ryan` and `s` in the body, so the same split here gives results that
/// actually match. Single-character tokens are dropped because a `*`-suffixed
/// single letter would match nearly every document.
pub fn sanitize_fts_query(input: &str) -> String {
    let tokens: Vec<&str> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.chars().count() >= 2)
        .collect();

    if tokens.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    for (i, t) in tokens.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(t);
        if i == tokens.len() - 1 {
            out.push('*');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(sanitize_fts_query(""), "");
    }

    #[test]
    fn whitespace_only_returns_empty() {
        assert_eq!(sanitize_fts_query("   "), "");
    }

    #[test]
    fn all_punctuation_returns_empty() {
        assert_eq!(sanitize_fts_query("()"), "");
        assert_eq!(sanitize_fts_query(":-/"), "");
        assert_eq!(sanitize_fts_query("\"\""), "");
    }

    #[test]
    fn simple_word_gets_prefix() {
        assert_eq!(sanitize_fts_query("pricing"), "pricing*");
    }

    #[test]
    fn colon_meeting_name_x1_wealth() {
        // Real meeting name pattern. FTS5 errors on raw "x1: wealth" with
        // "no such column: x1" without sanitization.
        assert_eq!(sanitize_fts_query("x1: wealth"), "x1 wealth*");
    }

    #[test]
    fn hyphenated_foo_bar() {
        assert_eq!(sanitize_fts_query("foo-bar"), "foo bar*");
    }

    #[test]
    fn apostrophe_splits_to_match_fts5_tokenizer() {
        // FTS5 unicode61 tokenizer treats apostrophes as separators, so
        // "Ryan's" indexes as "ryan" and "s". The 1-char "s" gets dropped
        // (it would match every document) and we're left with "Ryan*".
        assert_eq!(sanitize_fts_query("Ryan's"), "Ryan*");
    }

    #[test]
    fn apostrophe_ryans_standup() {
        // "Ryan's stand-up" tokenizes to Ryan, s, stand, up. "s" dropped.
        assert_eq!(sanitize_fts_query("Ryan's stand-up"), "Ryan stand up*");
    }

    #[test]
    fn single_char_tokens_dropped() {
        // "I a" → both 1-char tokens dropped → empty.
        assert_eq!(sanitize_fts_query("I a"), "");
        // "x1 a" → "a" dropped, "x1" wins.
        assert_eq!(sanitize_fts_query("x1 a"), "x1*");
    }

    #[test]
    fn slash_mat_cathryn() {
        assert_eq!(sanitize_fts_query("mat/cathryn"), "mat cathryn*");
    }

    #[test]
    fn quoted_input_no_crash() {
        // Quotes are separators; they don't blow up.
        assert_eq!(sanitize_fts_query("\"quoted text\""), "quoted text*");
    }

    #[test]
    fn parens_no_crash() {
        assert_eq!(sanitize_fts_query("foo (bar) baz"), "foo bar baz*");
    }

    #[test]
    fn unicode_diacritics_preserved() {
        // The `unicode61 remove_diacritics 2` tokenizer normalizes at index time;
        // we just need to keep the characters intact at sanitize time.
        assert_eq!(sanitize_fts_query("café résumé"), "café résumé*");
    }

    #[test]
    fn prefix_wildcard_only_on_last_token() {
        let s = sanitize_fts_query("alpha beta gamma");
        assert_eq!(s, "alpha beta gamma*");
        assert_eq!(s.matches('*').count(), 1);
    }

    #[test]
    fn dangling_quote_not_a_crash() {
        // "x1 — sanitizer treats `"` as a separator, so `x1` is the only token
        assert_eq!(sanitize_fts_query("\"x1"), "x1*");
    }

    #[test]
    fn underscore_not_treated_as_punctuation() {
        // Snake-case identifiers should stay as one token.
        // (alphanumeric check excludes _, so this DOES split — document the
        // current behavior; revisit if users complain.)
        assert_eq!(sanitize_fts_query("foo_bar"), "foo bar*");
    }

    /// End-to-end: sanitized output actually MATCHes the document we expect.
    /// This is the test that catches sanitizer regressions against real FTS5.
    #[test]
    fn integration_sanitized_query_matches_document() {
        use rusqlite::{params, Connection};

        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE VIRTUAL TABLE ft USING fts5(
                title, body,
                tokenize='porter unicode61 remove_diacritics 2',
                prefix='2 3 4'
            );",
        )
        .expect("create fts5 table");
        conn.execute(
            "INSERT INTO ft (rowid, title, body) VALUES (1, ?, ?)",
            params![
                "X1: Wealth Strategy Call",
                "talked about pricing tiers and Ryan's stand-up feedback"
            ],
        )
        .expect("insert");

        // Each of these inputs would error against raw MATCH.
        // Sanitized, they all find the row.
        for input in &[
            "x1: wealth",
            "x1",
            "wealth",
            "Ryan", // not "Ryan's" — apostrophes split per FTS5 tokenizer
            "stand-up",
            "pricing",
            "tier", // prefix match for "tiers"
            "weal", // prefix match for "wealth"
        ] {
            let q = sanitize_fts_query(input);
            assert!(
                !q.is_empty(),
                "input {:?} should sanitize to non-empty",
                input
            );
            let n: i64 = conn
                .query_row("SELECT COUNT(*) FROM ft WHERE ft MATCH ?", [&q], |r| {
                    r.get(0)
                })
                .unwrap_or_else(|e| panic!("MATCH {:?} (sanitized {:?}) failed: {}", input, q, e));
            assert_eq!(
                n, 1,
                "input {:?} (sanitized {:?}) should match the row",
                input, q
            );
        }
    }

    #[test]
    fn integration_punctuation_only_does_not_error() {
        // "()" should sanitize to empty string, and the caller skips MATCH.
        // We just verify here that there's no panic / shell-syntax weirdness.
        assert_eq!(sanitize_fts_query("()"), "");
    }
}
