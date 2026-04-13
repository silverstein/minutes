//! Word-to-sentence grouping for Parakeet ASR output.
//!
//! Parakeet emits word-level timestamps. This module groups consecutive words
//! into `[M:SS] text` segment lines matching the format produced by the whisper
//! path, so downstream diarization / summarization / markdown code is engine-agnostic.

/// A single timestamped word from an ASR engine.
pub(crate) struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

const GAP_FLUSH_SECS: f64 = 0.5;
const WORD_CAP: usize = 30;

/// Group word-level timestamps into `[M:SS] text` segment lines.
///
/// Flush rules (evaluated after each word):
/// 1. Punctuation flush — word's trimmed text ends with `.`, `!`, or `?`.
/// 2. Gap flush — gap to the next word exceeds `GAP_FLUSH_SECS`.
/// 3. Word-cap flush — buffer reaches `WORD_CAP` words (runaway safety net).
/// 4. Trailing flush — final word always flushes any remaining buffer.
pub(crate) fn group_words_into_lines(words: &[Word]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut buf: Vec<&str> = Vec::new();
    let mut segment_start: f64 = 0.0;

    for (i, word) in words.iter().enumerate() {
        let trimmed = word.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if buf.is_empty() {
            segment_start = word.start;
        }
        buf.push(trimmed);

        let is_last = i + 1 == words.len();
        let punct_flush = ends_sentence(trimmed);
        let gap_flush = !is_last && (words[i + 1].start - word.end) > GAP_FLUSH_SECS;
        let cap_flush = buf.len() >= WORD_CAP;

        if punct_flush || gap_flush || cap_flush || is_last {
            lines.push(format_line(segment_start, &buf));
            buf.clear();
        }
    }

    lines
}

fn ends_sentence(word: &str) -> bool {
    matches!(word.chars().last(), Some('.') | Some('!') | Some('?'))
}

fn format_line(start_secs: f64, words: &[&str]) -> String {
    let mins = (start_secs / 60.0) as u64;
    let secs = (start_secs % 60.0) as u64;
    format!("[{}:{:02}] {}", mins, secs, words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, start: f64, end: f64) -> Word {
        Word {
            text: text.into(),
            start,
            end,
        }
    }

    #[test]
    fn punctuation_flush_breaks_on_terminator() {
        // No gaps anywhere — only punctuation should split the segments.
        let words = vec![
            w("Hello", 0.0, 0.4),
            w("world.", 0.4, 0.9),
            w("Again", 0.9, 1.3),
        ];
        let lines = group_words_into_lines(&words);
        assert_eq!(
            lines,
            vec!["[0:00] Hello world.".to_string(), "[0:00] Again".to_string()]
        );
    }

    #[test]
    fn gap_flush_breaks_on_long_pause() {
        // No punctuation — a >0.5s gap is the only thing that can split.
        let words = vec![
            w("one", 0.0, 0.3),
            w("two", 1.0, 1.3), // 0.7s gap after "one"
            w("three", 62.0, 62.3), // later timestamp to exercise M:SS
        ];
        let lines = group_words_into_lines(&words);
        assert_eq!(
            lines,
            vec![
                "[0:00] one".to_string(),
                "[0:01] two".to_string(),
                "[1:02] three".to_string(),
            ]
        );
    }

    #[test]
    fn trailing_flush_emits_final_segment() {
        // Single word, no punctuation, no successor — trailing flush must emit it.
        let words = vec![w("solitary", 5.0, 5.4)];
        let lines = group_words_into_lines(&words);
        assert_eq!(lines, vec!["[0:05] solitary".to_string()]);
    }
}
