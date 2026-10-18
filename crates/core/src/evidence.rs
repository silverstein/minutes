//! Exact source citations shared with `minutes-sdk/src/evidence.ts`.
//!
//! This module is pure: a path is a citation, never a read grant. Callers must
//! supply an already-authorized, immutable source snapshot. Search attaches
//! evidence only after its live policy and corpus-revision checks.
use crate::diarize::{Confidence, SpeakerAttribution};
use crate::retrieval::{fold_search_text, query_is_bounded, search_tokens, token_matches};
use serde::Serialize;
use sha2::{Digest, Sha256};

const MAX_QUOTE_UTF16: usize = 1200;

pub struct EvidenceSource<'a> {
    pub path: &'a str,
    pub content: &'a str,
    /// The exact body suffix of `content`, never a re-rendered Markdown body.
    pub body: &'a str,
    pub speaker_map: &'a [SpeakerAttribution],
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvidenceSpeaker {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceEvidence {
    pub id: String,
    pub source_id: String,
    pub source_revision: String,
    pub path: String,
    pub quote: String,
    /// Zero-based UTF-16 offsets into the exact original Markdown snapshot.
    pub start: usize,
    pub end: usize,
    pub offset_unit: &'static str,
    pub line_start: usize,
    pub line_end: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<EvidenceSpeaker>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvidenceBundle {
    pub source_id: String,
    pub source_revision: String,
    pub segments: Vec<SourceEvidence>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceError {
    InvalidSpan,
    InvalidLimit,
    BodyMismatch,
    QueryTooLong,
}

impl std::fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSpan => "evidence span is outside its snapshot or splits a UTF-16 pair",
            Self::InvalidLimit => "evidence limit must be between 1 and 128",
            Self::BodyMismatch => "evidence body does not match its source snapshot",
            Self::QueryTooLong => "evidence query exceeds the search query limit",
        })
    }
}

impl std::error::Error for EvidenceError {}

pub fn evidence_hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn source_id(path: &str) -> String {
    evidence_hash(&format!("minutes-source-v1\0{path}"))
}

/// Convert only scalar-aligned UTF-16 offsets, preserving exact UTF-8 quotes.
fn byte_offset(text: &str, target: usize) -> Option<usize> {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        if units == target {
            return Some(index);
        }
        units += character.len_utf16();
        if units > target {
            return None;
        }
    }
    (units == target).then_some(text.len())
}

/// Bind an exact source span. Offsets use JavaScript's UTF-16 string convention
/// so CLI and MCP citations have identical identifiers, including for emoji.
pub fn evidence_for_span(
    source: &EvidenceSource<'_>,
    start: usize,
    end: usize,
) -> Result<SourceEvidence, EvidenceError> {
    span_evidence(
        source,
        start,
        end,
        &source_id(source.path),
        &evidence_hash(source.content),
    )
}

fn span_evidence(
    source: &EvidenceSource<'_>,
    start: usize,
    requested_end: usize,
    source_id: &str,
    revision: &str,
) -> Result<SourceEvidence, EvidenceError> {
    if requested_end <= start {
        return Err(EvidenceError::InvalidSpan);
    }
    let byte_start = byte_offset(source.content, start).ok_or(EvidenceError::InvalidSpan)?;
    let byte_requested_end =
        byte_offset(source.content, requested_end).ok_or(EvidenceError::InvalidSpan)?;
    let requested = &source.content[byte_start..byte_requested_end];
    let mut kept_units = 0;
    let mut kept_bytes = 0;
    for character in requested.chars() {
        if kept_units + character.len_utf16() > MAX_QUOTE_UTF16 {
            break;
        }
        kept_units += character.len_utf16();
        kept_bytes += character.len_utf8();
    }
    let end = start + kept_units;
    let quote = &requested[..kept_bytes];
    let line_start = source.content[..byte_start]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    let line_end = line_start + quote.bytes().filter(|byte| *byte == b'\n').count();
    let (timestamp_seconds, label) = timestamp_label(quote);
    let speaker = label.map(|label| {
        let attribution = source
            .speaker_map
            .iter()
            .find(|entry| entry.speaker_label == label);
        EvidenceSpeaker {
            label: label.to_owned(),
            name: attribution
                .filter(|entry| entry.confidence == Confidence::High)
                .map(|entry| entry.name.clone()),
            confidence: attribution.map(|entry| {
                match entry.confidence {
                    Confidence::High => "high",
                    Confidence::Medium => "medium",
                    Confidence::Low => "low",
                }
                .to_owned()
            }),
            provenance: attribution
                .and_then(|entry| serde_json::to_value(&entry.source).ok())
                .and_then(|value| value.as_str().map(str::to_owned)),
        }
    });
    Ok(SourceEvidence {
        id: evidence_hash(&format!(
            "minutes-segment-v1\0{source_id}\0{revision}\0{start}\0{end}"
        )),
        source_id: source_id.to_owned(),
        source_revision: revision.to_owned(),
        path: source.path.to_owned(),
        quote: quote.to_owned(),
        start,
        end,
        offset_unit: "utf16",
        line_start,
        line_end,
        quote_truncated: (end < requested_end).then_some(true),
        timestamp_seconds,
        speaker,
    })
}

/// Parse the same anchored `[label M:SS]` / `[H:MM:SS.f]` form as the SDK.
/// Invalid seconds suppress the time but do not invent a different speaker.
fn timestamp_label(quote: &str) -> (Option<f64>, Option<&str>) {
    static TIMESTAMP: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = TIMESTAMP.get_or_init(|| {
        // ECMAScript whitespace and ASCII digits match the SDK regular
        // expression, including BOM and excluding the Unicode NEL character.
        let ws = r"[\t\n\x0B\x0C\r\x20\u{00A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]";
        regex::Regex::new(&format!(r"^{ws}*(?:[-*]{ws}*)?\[(?:([^\n\r\u{{2028}}\u{{2029}}]+?){ws}+)?([0-9]+(?::[0-9]{{2}}){{1,2}}(?:\.[0-9]+)?)\]"))
            .expect("static evidence timestamp expression")
    });
    let Some(captures) = pattern.captures(quote) else {
        return (None, None);
    };
    let label = captures.get(1).map(|matched| matched.as_str());
    let Some(time) = captures.get(2) else {
        return (None, None);
    };
    let mut seconds = 0.0;
    for (index, part) in time.as_str().split(':').enumerate() {
        let Ok(value) = part.parse::<f64>() else {
            return (None, label);
        };
        if !value.is_finite() || (index > 0 && value >= 60.0) {
            return (None, label);
        }
        seconds = seconds * 60.0 + value;
    }
    (seconds.is_finite().then_some(seconds), label)
}

/// Preserve raw token offsets while folding for lexical comparisons. Like the
/// SDK's L/N/M token regex, combining marks stay with their original token.
fn line_tokens_with_offsets(line: &str) -> Vec<(String, usize)> {
    static TOKENS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = TOKENS.get_or_init(|| {
        regex::Regex::new(r"[\p{L}\p{N}\p{M}]+").expect("static evidence token expression")
    });
    let mut tokens = Vec::new();
    let mut units = 0;
    let mut byte_end = 0;
    for matched in pattern.find_iter(line) {
        units += line[byte_end..matched.start()].encode_utf16().count();
        tokens.push((fold_search_text(matched.as_str()), units));
        units += matched.as_str().encode_utf16().count();
        byte_end = matched.end();
    }
    tokens
}

/// Rank original nonempty body lines by covered query tokens, then source
/// order. This is a bounded citation list, not authorization or semantic proof.
pub fn build_source_evidence(
    source: &EvidenceSource<'_>,
    query: &str,
    limit: usize,
) -> Result<EvidenceBundle, EvidenceError> {
    if !(1..=128).contains(&limit) {
        return Err(EvidenceError::InvalidLimit);
    }
    if !source.content.ends_with(source.body) {
        return Err(EvidenceError::BodyMismatch);
    }
    if !query_is_bounded(query) {
        return Err(EvidenceError::QueryTooLong);
    }
    let tokens = search_tokens(query);
    let prefix_bytes = source.content.len() - source.body.len();
    let mut offset = source.content[..prefix_bytes].encode_utf16().count();
    let mut candidates = Vec::new();
    for line in source.body.split('\n') {
        let length = line.encode_utf16().count();
        if !line.trim().is_empty() {
            let line_tokens = line_tokens_with_offsets(line);
            let score = tokens
                .iter()
                .enumerate()
                .filter(|(index, wanted)| {
                    line_tokens
                        .iter()
                        .any(|(token, _)| token_matches(token, wanted, *index + 1 == tokens.len()))
                })
                .count();
            if tokens.is_empty() || score > 0 {
                let hit = line_tokens.iter().find(|(token, _)| {
                    tokens.iter().enumerate().any(|(index, wanted)| {
                        token_matches(token, wanted, index + 1 == tokens.len())
                    })
                });
                let mut start = hit.map_or(0, |(_, units)| units.saturating_sub(120));
                if byte_offset(line, start).is_none() {
                    start += 1;
                }
                candidates.push((offset + start, offset + length, score, start > 0));
            }
        }
        offset += length + 1;
    }
    candidates.sort_by(|left, right| right.2.cmp(&left.2).then(left.0.cmp(&right.0)));
    let source_id = source_id(source.path);
    let source_revision = evidence_hash(source.content);
    let segments: Vec<_> = candidates
        .iter()
        .take(limit)
        .map(|(start, end, _, clipped)| {
            let mut evidence = span_evidence(source, *start, *end, &source_id, &source_revision)?;
            if *clipped {
                evidence.quote_truncated = Some(true);
            }
            Ok(evidence)
        })
        .collect::<Result<_, _>>()?;
    let truncated = candidates.len() > limit
        || segments
            .iter()
            .any(|segment| segment.quote_truncated == Some(true));
    Ok(EvidenceBundle {
        source_id,
        source_revision,
        segments,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diarize::AttributionSource;

    const BODY: &str =
        "# Notes\n[SPEAKER_1 01:02.5] Café 😀 launch.\n[01:03] Decision: ship Friday.\n";
    const CONTENT: &str = "---\ntitle: Plan\n---\n# Notes\n[SPEAKER_1 01:02.5] Café 😀 launch.\n[01:03] Decision: ship Friday.\n";

    fn source() -> EvidenceSource<'static> {
        EvidenceSource {
            path: "/meetings/emoji.md",
            content: CONTENT,
            body: BODY,
            speaker_map: &[],
        }
    }

    #[test]
    fn evidence_matches_sdk_golden_hashes_and_utf16_spans() {
        let bundle = build_source_evidence(&source(), "cafe", 32).unwrap();
        assert_eq!(
            bundle.source_id,
            "2e0d3ab9af2dcb5cbbab8c73b995ce98ab2ddf621ad6d991c96de20a1f7dc4ff"
        );
        assert_eq!(
            bundle.source_revision,
            "85803628fcb381e7e2767a56499435b8d8a815d638a58b4fdb85a5ededeb640e"
        );
        assert_eq!(bundle.segments.len(), 1);
        let segment = &bundle.segments[0];
        assert_eq!(
            segment.id,
            "fe300c55ff9e0a6b219ede5bb70da3676327241596784d4d2586320e13cda582"
        );
        assert_eq!((segment.start, segment.end), (28, 63));
        assert_eq!((segment.line_start, segment.line_end), (5, 5));
        assert_eq!(segment.quote, "[SPEAKER_1 01:02.5] Café 😀 launch.");
        assert_eq!(segment.timestamp_seconds, Some(62.5));
        assert_eq!(segment.offset_unit, "utf16");
        assert_eq!(segment.speaker.as_ref().unwrap().label, "SPEAKER_1");
        assert!(!bundle.truncated);
        let units: Vec<_> = CONTENT.encode_utf16().collect();
        assert_eq!(
            String::from_utf16(&units[segment.start..segment.end]).unwrap(),
            segment.quote
        );
    }

    #[test]
    fn any_source_revision_invalidates_segment_ids() {
        let original = build_source_evidence(&source(), "cafe", 32).unwrap();
        let content = format!("{CONTENT}\n");
        let body = format!("{BODY}\n");
        let updated = build_source_evidence(
            &EvidenceSource {
                content: &content,
                body: &body,
                ..source()
            },
            "cafe",
            32,
        )
        .unwrap();
        assert_eq!(original.source_id, updated.source_id);
        assert_ne!(original.source_revision, updated.source_revision);
        assert_ne!(original.segments[0].id, updated.segments[0].id);
        assert_eq!(original.segments[0].quote, updated.segments[0].quote);
    }

    #[test]
    fn only_high_confidence_attributions_add_a_person_name() {
        for confidence in [Confidence::High, Confidence::Medium, Confidence::Low] {
            let mapping = [SpeakerAttribution {
                speaker_label: "SPEAKER_1".into(),
                name: "Ann".into(),
                confidence,
                source: AttributionSource::Manual,
            }];
            let bundle = build_source_evidence(
                &EvidenceSource {
                    speaker_map: &mapping,
                    ..source()
                },
                "cafe",
                32,
            )
            .unwrap();
            let speaker = bundle.segments[0].speaker.as_ref().unwrap();
            assert_eq!(
                speaker.name.as_deref(),
                (confidence == Confidence::High).then_some("Ann")
            );
            assert_eq!(speaker.provenance.as_deref(), Some("manual"));
        }
        assert_eq!(
            timestamp_label("- [Ann 1:02:03.25] hello"),
            (Some(3723.25), Some("Ann"))
        );
        assert_eq!(
            timestamp_label("[SPEAKER_1 1:99] hello"),
            (None, Some("SPEAKER_1"))
        );
        assert_eq!(timestamp_label("[made up] hello"), (None, None));
        assert_eq!(timestamp_label("[1:2] hello"), (None, None));
    }

    #[test]
    fn raw_token_categories_match_sdk_including_circled_letters() {
        assert_eq!(
            line_tokens_with_offsets("abⓐcd"),
            vec![("ab".into(), 0), ("cd".into(), 3)]
        );
    }

    #[test]
    fn long_quotes_never_split_emoji_and_expose_truncation() {
        let content = format!("{}😀{}", "x".repeat(1199), "x".repeat(2000));
        let source = EvidenceSource {
            path: "long.md",
            content: &content,
            body: &content,
            speaker_map: &[],
        };
        let bundle = build_source_evidence(&source, "", 32).unwrap();
        assert!(bundle.truncated);
        assert_eq!(bundle.segments[0].end, 1199);
        assert_eq!(bundle.segments[0].quote, "x".repeat(1199));
        assert_eq!(bundle.segments[0].quote_truncated, Some(true));
        assert_eq!(
            evidence_for_span(&source, 1200, 1203),
            Err(EvidenceError::InvalidSpan)
        );
        assert_eq!(
            evidence_for_span(&source, 1199, 1200),
            Err(EvidenceError::InvalidSpan)
        );
    }

    #[test]
    fn long_line_window_contains_the_match_near_its_tail() {
        let content = format!("{} Café deployment approved.", "😀 ".repeat(1000));
        let source = EvidenceSource {
            path: "long.md",
            content: &content,
            body: &content,
            speaker_map: &[],
        };
        let bundle = build_source_evidence(&source, "cafe", 32).unwrap();
        assert!(bundle.truncated);
        assert!(bundle.segments[0]
            .quote
            .contains("Café deployment approved"));
        assert!(bundle.segments[0].start > 0);
        let units: Vec<_> = content.encode_utf16().collect();
        assert_eq!(
            String::from_utf16(&units[bundle.segments[0].start..bundle.segments[0].end]).unwrap(),
            bundle.segments[0].quote
        );
    }

    #[test]
    fn validation_limits_and_ranking_match_the_sdk_contract() {
        assert_eq!(
            build_source_evidence(&source(), "", 0),
            Err(EvidenceError::InvalidLimit)
        );
        assert_eq!(
            build_source_evidence(&source(), "", 129),
            Err(EvidenceError::InvalidLimit)
        );
        assert_eq!(
            build_source_evidence(&source(), &"😀".repeat(1025), 32),
            Err(EvidenceError::QueryTooLong)
        );
        assert_eq!(
            build_source_evidence(
                &EvidenceSource {
                    body: "unrelated body",
                    ..source()
                },
                "",
                32
            ),
            Err(EvidenceError::BodyMismatch)
        );
        assert_eq!(
            evidence_for_span(&source(), 4, 4),
            Err(EvidenceError::InvalidSpan)
        );
        assert_eq!(
            evidence_for_span(&source(), 0, usize::MAX),
            Err(EvidenceError::InvalidSpan)
        );
        let bundle = build_source_evidence(&source(), "", 1).unwrap();
        assert_eq!(bundle.segments[0].quote, "# Notes");
        assert!(bundle.truncated);
        let bundle = build_source_evidence(&source(), "decision fri", 1).unwrap();
        assert_eq!(bundle.segments[0].quote, "[01:03] Decision: ship Friday.");
        assert_eq!(bundle.segments[0].timestamp_seconds, Some(63.0));
    }
}
