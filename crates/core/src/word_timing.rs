//! Word-level timings for transcript segments (#1413).
//!
//! Whisper computes a time span for every token (`set_token_timestamps`).
//! [`words_from_tokens`] groups those tokens into words; the whisper caller
//! puts them on [`crate::transcribe::TranscriptSegment::words`].
//!
//! Clock: whisper-rs `WhisperState::full` calls `whisper_full_with_state`,
//! which does not apply whisper.cpp's integrated VAD (only `whisper_full`
//! and `whisper_full_parallel` do). So token times are on the clock of the
//! samples passed in, the same clock as segment times. If that ever changes,
//! token times would be on VAD-compressed audio while segment times are not;
//! the caller checks every segment's last token against the segment end and
//! drops the words of a call on any mismatch.
//!
//! Token times between whisper's timestamp anchors are interpolated by
//! whisper.cpp in proportion to token length, so a word time is an estimate,
//! not an alignment. Attribution must treat it that way.

use serde::{Deserialize, Serialize};

/// One word and its span, in seconds on the same clock as its segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordTiming {
    /// Start in seconds.
    pub start: f64,
    /// End in seconds.
    pub end: f64,
    /// The word as it appears in the segment text, without surrounding spaces.
    pub text: String,
}

/// A decoded token: raw bytes and its span in centiseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawToken {
    pub bytes: Vec<u8>,
    pub t0: i64,
    pub t1: i64,
}

/// Group text tokens into words. A token whose text starts with a space
/// begins a new word. Bytes are joined before decoding, because whisper can
/// split one UTF-8 character across tokens.
/// Pass only text tokens (ids below `token_eot`).
pub fn words_from_tokens(tokens: &[RawToken]) -> Vec<(i64, i64, String)> {
    let mut words: Vec<(i64, i64, Vec<u8>)> = Vec::new();
    for token in tokens {
        if token.bytes.is_empty() {
            continue;
        }
        let starts_word = token.bytes[0] == b' ' || words.is_empty();
        if starts_word {
            words.push((token.t0, token.t1, token.bytes.clone()));
        } else if let Some(word) = words.last_mut() {
            word.1 = token.t1;
            word.2.extend_from_slice(&token.bytes);
        }
    }
    words
        .into_iter()
        .filter_map(|(t0, t1, bytes)| {
            let text = String::from_utf8_lossy(&bytes).trim().to_string();
            (!text.is_empty()).then_some((t0, t1.max(t0), text))
        })
        .collect()
}

/// Whether `words`, joined by single spaces, are the whitespace tokens of
/// `text`. Word timings are only trusted for a segment when this holds.
pub fn words_match_text(words: &[WordTiming], text: &str) -> bool {
    let mut tokens = text.split_whitespace();
    for word in words {
        for part in word.text.split_whitespace() {
            if tokens.next() != Some(part) {
                return false;
            }
        }
    }
    tokens.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(text: &str, t0: i64, t1: i64) -> RawToken {
        RawToken {
            bytes: text.as_bytes().to_vec(),
            t0,
            t1,
        }
    }

    #[test]
    fn words_join_subword_tokens_and_keep_outer_times() {
        let words = words_from_tokens(&[
            token(" Hello", 0, 40),
            token(" every", 40, 70),
            token("one", 70, 95),
            token(",", 95, 96),
            token(" thanks", 110, 150),
        ]);
        assert_eq!(
            words,
            vec![
                (0, 40, "Hello".to_string()),
                (40, 96, "everyone,".to_string()),
                (110, 150, "thanks".to_string()),
            ]
        );
    }

    #[test]
    fn words_join_bytes_before_decoding_split_characters() {
        // "é" is 0xC3 0xA9; whisper can emit it as two tokens.
        let words = words_from_tokens(&[
            RawToken {
                bytes: vec![b' ', b'c', b'a', b'f', 0xC3],
                t0: 0,
                t1: 20,
            },
            RawToken {
                bytes: vec![0xA9],
                t0: 20,
                t1: 30,
            },
        ]);
        assert_eq!(words, vec![(0, 30, "café".to_string())]);
    }

    #[test]
    fn first_token_without_leading_space_still_starts_a_word() {
        let words = words_from_tokens(&[token("Hi", 5, 10), token(" there", 10, 20)]);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].2, "Hi");
    }

    #[test]
    fn words_match_text_requires_identical_tokens() {
        let words = vec![
            WordTiming {
                start: 0.0,
                end: 0.4,
                text: "Hello".into(),
            },
            WordTiming {
                start: 0.4,
                end: 0.9,
                text: "everyone,".into(),
            },
        ];
        assert!(words_match_text(&words, "Hello everyone,"));
        assert!(words_match_text(&words, "  Hello   everyone, "));
        assert!(!words_match_text(&words, "Hello everyone"));
        assert!(!words_match_text(&words, "Hello everyone, again"));
        assert!(!words_match_text(&words[..1], "Hello everyone,"));
    }
}
