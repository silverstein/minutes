use std::collections::BTreeSet;

const MIN_TOPIC_TERMS: usize = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct TopicShift {
    pub prior_keywords: Vec<String>,
    pub current_keywords: Vec<String>,
    pub overlap: f64,
}

#[derive(Debug, Default)]
pub struct TopicShiftDetector {
    anchor: BTreeSet<String>,
}

impl TopicShiftDetector {
    pub fn observe_final(&mut self, text: &str) -> Option<TopicShift> {
        let current = keywords(text);
        if current.len() < MIN_TOPIC_TERMS {
            return None;
        }
        if self.anchor.is_empty() {
            self.anchor = current;
            return None;
        }
        let intersection = self.anchor.intersection(&current).count();
        let union = self.anchor.union(&current).count().max(1);
        let overlap = intersection as f64 / union as f64;
        let explicit_transition = [
            "moving on",
            "next topic",
            "switching to",
            "turn to",
            "separately",
            "new subject",
        ]
        .iter()
        .any(|cue| text.to_ascii_lowercase().contains(cue));
        let shifted =
            explicit_transition || (overlap <= 0.10 && self.anchor.len() >= MIN_TOPIC_TERMS);
        if shifted {
            let prior_keywords = self.anchor.iter().cloned().collect();
            let current_keywords = current.iter().cloned().collect();
            self.anchor = current;
            Some(TopicShift {
                prior_keywords,
                current_keywords,
                overlap,
            })
        } else {
            self.anchor.extend(current);
            while self.anchor.len() > 18 {
                let Some(first) = self.anchor.first().cloned() else {
                    break;
                };
                self.anchor.remove(&first);
            }
            None
        }
    }

    pub fn reset(&mut self) {
        self.anchor.clear();
    }
}

pub fn keywords(text: &str) -> BTreeSet<String> {
    const STOP: &[&str] = &[
        "about", "after", "again", "also", "because", "been", "before", "being", "could", "from",
        "have", "into", "just", "more", "next", "only", "other", "should", "that", "their",
        "there", "these", "they", "this", "those", "through", "today", "very", "want", "were",
        "what", "when", "where", "which", "while", "with", "would", "your",
    ];
    text.to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .filter(|word| word.len() >= 4 && !STOP.contains(word))
        .map(str::to_string)
        .collect()
}

pub fn is_decisive_final(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    [
        "we decided",
        "we agreed",
        "decision is",
        "approved",
        "rejected",
        "go with",
        "final answer",
        "i will",
        "we will",
        "we'll",
    ]
    .iter()
    .any(|cue| normalized.contains(cue))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_new_topic_is_detected_without_embeddings() {
        let mut detector = TopicShiftDetector::default();
        assert!(detector
            .observe_final("Pricing packaging discounts and annual contract terms remain open")
            .is_none());
        let shift = detector
            .observe_final(
                "Moving on, hiring interview scorecards need calibration across engineering",
            )
            .expect("transition cue should produce a shift");
        assert!(shift.current_keywords.contains(&"hiring".into()));
    }
}
