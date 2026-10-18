//! Literal, bounded FTS5 query construction. Operators in user input are data.
use crate::retrieval::{query_is_bounded, search_tokens};

/// Match all literal tokens, with a prefix only on the final multi-character term.
/// Quotes stay outside the wildcard; `"pric"*` is a prefix, `"pric*"` is not.
pub fn sanitize_fts_query(input: &str) -> String {
    if !query_is_bounded(input) {
        return String::new();
    }
    let tokens = search_tokens(input);
    tokens
        .iter()
        .enumerate()
        .map(|(index, token)| {
            let suffix = if index + 1 == tokens.len() && token.chars().count() >= 2 {
                "*"
            } else {
                ""
            };
            format!("\"{token}\"{suffix}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operators_and_punctuation_are_literals() {
        assert_eq!(sanitize_fts_query("AND pricing"), "\"and\" \"pricing\"*");
        assert_eq!(sanitize_fts_query("OR"), "\"or\"*");
        assert_eq!(
            sanitize_fts_query("Ryan's stand-up"),
            "\"ryan\" \"s\" \"stand\" \"up\"*"
        );
        assert_eq!(sanitize_fts_query("x1: wealth"), "\"x1\" \"wealth\"*");
        assert_eq!(sanitize_fts_query("a"), "\"a\"");
        for value in ["", " ", "()", ":-/", "\"\""] {
            assert_eq!(sanitize_fts_query(value), "");
        }
    }
    #[test]
    fn generated_queries_execute_against_real_sqlite() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE VIRTUAL TABLE ft USING fts5(title,body,tokenize='unicode61 remove_diacritics 2');").unwrap();
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/search-contract-v1.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            conn.execute("DELETE FROM ft", []).unwrap();
            conn.execute(
                "INSERT INTO ft VALUES (?1, ?2)",
                rusqlite::params![
                    crate::retrieval::search_tokens(case["title"].as_str().unwrap()).join(" "),
                    crate::retrieval::search_tokens(case["body"].as_str().unwrap()).join(" ")
                ],
            )
            .unwrap();
            let q = sanitize_fts_query(case["query"].as_str().unwrap());
            let count: i64 = if q.is_empty() {
                0
            } else {
                conn.query_row("SELECT count(*) FROM ft WHERE ft MATCH ?1", [q], |row| {
                    row.get(0)
                })
                .unwrap()
            };
            assert_eq!(
                count > 0,
                case["matches"].as_bool().unwrap(),
                "{}",
                case["id"]
            );
        }
    }
}
