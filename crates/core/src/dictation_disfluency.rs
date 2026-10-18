//! Bounded English repair patterns, not a semantic rewrite engine.
//!
//! Work only from the utterance. Quoted/structured text and uncertain repairs
//! pass through. The caller retains raw ASR output for recovery.

#[derive(Debug)]
struct Word {
    start: usize,
    end: usize,
    lower: String,
}

fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut start = None;
    for (index, ch) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if ch.is_alphanumeric() || matches!(ch, '\'' | '’') {
            start.get_or_insert(index);
        } else if let Some(begin) = start.take() {
            out.push(Word {
                start: begin,
                end: index,
                lower: text[begin..index].to_lowercase(),
            });
        }
    }
    out
}

pub(super) fn clean(text: &str) -> String {
    // Do not interpret quoted examples, code, paths, parentheticals or decimal
    // quantities. Bound work even for unusually long ASR output.
    if text.len() > 4096
        || text.contains([
            '"', '“', '”', '`', '(', ')', '[', ']', '{', '}', '/', '\\', '=', '<', '>', '|', '_',
        ])
        || text
            .as_bytes()
            .windows(3)
            .any(|w| w[0].is_ascii_digit() && w[1] == b'.' && w[2].is_ascii_digit())
        || text.char_indices().any(|(i, c)| {
            matches!(c, '\'' | '‘')
                && (i == 0
                    || !text[..i]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_alphanumeric))
        })
    {
        return text.to_string();
    }
    let mut result = text.to_string();
    // Every successful pass removes words. Stop when no supported edit remains.
    loop {
        let tokens = words(&result);
        let edit = correction(&result, &tokens).or_else(|| hesitation(&result, &tokens));
        let Some((start, end)) = edit else { break };
        result.replace_range(start..end, "");
    }
    result.trim().to_string()
}

fn separator(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_whitespace() || matches!(c, ',' | '-' | '—' | '–'))
}

fn sentence_start(text: &str, tokens: &[Word], index: usize) -> bool {
    index == 0 || text[tokens[index - 1].end..tokens[index].start].contains(['.', '!', '?'])
}

// Only comma-separated clusters at a sentence start: individual meaningful
// discourse words ("Well, that changes things", "Yeah, I agree") remain.
fn hesitation(text: &str, tokens: &[Word]) -> Option<(usize, usize)> {
    for start in 0..tokens.len() {
        if !sentence_start(text, tokens, start) {
            continue;
        }
        let mut index = start;
        let mut count = 0;
        let mut has_mean = false;
        while index < tokens.len() {
            let width = if tokens[index].lower == "i"
                && tokens.get(index + 1).is_some_and(|w| w.lower == "mean")
                && text[tokens[index].end..tokens[index + 1].start]
                    .trim()
                    .is_empty()
            {
                has_mean = true;
                2
            } else if matches!(tokens[index].lower.as_str(), "well" | "yeah" | "okay") {
                1
            } else {
                break;
            };
            let next = index + width;
            let Some(word) = tokens.get(next) else { break };
            let gap = &text[tokens[next - 1].end..word.start];
            if !gap.contains(',') || !separator(gap) {
                break;
            }
            count += 1;
            index = next;
        }
        if count >= 2
            && has_mean
            && index < tokens.len()
            && !matches!(tokens[index].lower.as_str(), "well" | "yeah" | "okay")
        {
            return Some((tokens[start].start, tokens[index].start));
        }
    }
    None
}

fn correction(text: &str, tokens: &[Word]) -> Option<(usize, usize)> {
    for marker in 1..tokens.len() {
        let width = match tokens[marker].lower.as_str() {
            "actually" => 1,
            "i" if tokens.get(marker + 1).is_some_and(|w| w.lower == "mean") => 2,
            "scratch" if tokens.get(marker + 1).is_some_and(|w| w.lower == "that") => 2,
            _ => continue,
        };
        let mut next = marker + width;
        if next >= tokens.len() {
            continue;
        }
        if width == 2
            && !text[tokens[marker].end..tokens[marker + 1].start]
                .trim()
                .is_empty()
        {
            continue;
        }
        if let Some((start, end)) = hesitation(text, tokens) {
            if start == tokens[marker].start {
                next = tokens.iter().position(|w| w.start == end).unwrap_or(next);
            }
        }
        let before = &text[tokens[marker - 1].end..tokens[marker].start];
        let after = &text[tokens[next - 1].end..tokens[next].start];
        let marker_continuation = separator(after);
        // ASR can put a sentence boundary after the repair marker. Only a
        // repeated phrase may repair across it; a bare next value stays literal.
        if !marker_continuation && after.trim() != "." {
            continue;
        }
        let same_clause = separator(before);
        let right_end = (next + 1..tokens.len())
            .find(|&i| sentence_start(text, tokens, i))
            .unwrap_or(tokens.len());
        let continuation = &tokens[next + 1..right_end];
        // A bare replacement or a known quantity unit is unambiguous. Do not
        // guess whether arbitrary following prose is a correction or addition.
        let replacement_tail = continuation.is_empty()
            || continuation.len() == 1
                && matches!(
                    continuation[0].lower.as_str(),
                    "tickets"
                        | "copies"
                        | "items"
                        | "minutes"
                        | "hours"
                        | "days"
                        | "weeks"
                        | "months"
                        | "years"
                        | "dollars"
                        | "percent"
                        | "people"
                );
        if same_clause
            && marker_continuation
            && replacement_tail
            && same_value_kind(&tokens[marker - 1].lower, &tokens[next].lower)
            && !text[..tokens[marker - 1].start].ends_with(['-', '+', ',', '.'])
        {
            return Some((tokens[marker - 1].start, tokens[next].start));
        }
        // Repeated wording requires an explicit pause before the repair marker.
        // Bare "actually" is meaningful in ordinary prose.
        if !before.contains([',', '—', '–', '.', '!', '?']) {
            continue;
        }
        let left_start = (0..marker)
            .rev()
            .find(|&i| sentence_start(text, tokens, i))
            .unwrap_or(0);
        for start in left_start..marker {
            let common = tokens[start..marker]
                .iter()
                .zip(&tokens[next..right_end])
                .take_while(|(a, b)| a.lower == b.lower)
                .count();
            let repeats_complete_tail = common == marker - start && common == right_end - next;
            if common >= 2 && (start == left_start || repeats_complete_tail) {
                return Some((tokens[start].start, tokens[next].start));
            }
            // "a recording test, actually a dictation test": repeated article
            // and final noun frame. Require complete phrases on both sides.
            if common == 1
                && matches!(tokens[start].lower.as_str(), "a" | "an" | "the")
                && marker - start >= 3
                && right_end - next >= 3
                && tokens[marker - 1].lower == tokens[right_end - 1].lower
            {
                return Some((tokens[start].start, tokens[next].start));
            }
        }
    }
    None
}

fn same_value_kind(left: &str, right: &str) -> bool {
    const DAYS: &[&str] = &[
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    let integer = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    (DAYS.contains(&left) && DAYS.contains(&right)) || (integer(left) && integer(right))
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn supported_repairs_and_hesitations() {
        for (raw, expected) in [
            (
                "Alright, this is a dictation app test. Well, I mean, yeah, let's try that again.",
                "Alright, this is a dictation app test. let's try that again.",
            ),
            (
                "I mean, well, let's try that again.",
                "let's try that again.",
            ),
            ("Meet on Tuesday—actually, Thursday.", "Meet on Thursday."),
            (
                "Let's meet on Tuesday, actually. Let's meet on Thursday.",
                "Let's meet on Thursday.",
            ),
            ("Buy 15, actually 50 tickets.", "Buy 50 tickets."),
            (
                "This is a recording test—actually, a dictation test.",
                "This is a dictation test.",
            ),
            (
                "Send the report to Alice, actually send the report to Bob.",
                "send the report to Bob.",
            ),
            (
                "Let's meet on Tuesday, scratch that, let's meet on Thursday.",
                "let's meet on Thursday.",
            ),
            (
                "Please fix the issue, I mean, please fix the warning.",
                "please fix the warning.",
            ),
            (
                "Café opens Monday, actually Tuesday.",
                "Café opens Tuesday.",
            ),
        ] {
            let result = clean(raw);
            assert_eq!(result, expected, "{raw}");
            assert_eq!(clean(&result), result, "idempotency: {raw}");
        }
    }

    #[test]
    fn meaningful_and_uncertain_content_is_preserved() {
        for text in [
            "Well, that changes things.",
            "Yeah, I agree.",
            "I actually enjoyed the movie.",
            "I mean what I said.",
            "I mean, this matters.",
            "Well, I mean, yeah.",
            "We should not actually delete the files.",
            "I mean no, do not send it.",
            "Alice, actually Bob is joining too.",
            "15 actually describes the old version.",
            "Tuesday, actually Thursday is also available.",
            "The phrase \"Tuesday, actually Thursday\" is an example.",
            "Use `15, actually 50` as sample input.",
            "Keep (well, I mean, yeah) in the quote.",
            "Use /tmp/actually/data.",
            "if day == Tuesday, actually Thursday",
            "The field day_name is Tuesday, actually Thursday",
            "The phrase 'Tuesday, actually Thursday' is literal.",
            "Set it to 1.5, actually 2.5.",
            "Set it to -2, actually 3.",
            "Buy 1,500, actually 20 tickets.",
            "Tuesday. Actually, Thursday also works.",
            "I like the coffee, actually the coffee is good.",
        ] {
            // Ambiguous additions must not look like a replacement.
            assert_eq!(clean(text), text, "{text}");
        }
    }
}
