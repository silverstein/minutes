use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::config::Config;
use crate::markdown::ContentType;

// ──────────────────────────────────────────────────────────────
// Event log: append-only JSONL at ~/.minutes/events.jsonl.
//
// Agents can tail/poll this file to react to new meetings.
// Non-fatal: pipeline never fails if event logging fails.
// Rotates to events.{date}.jsonl when file exceeds 10MB.
//
// Meeting insights (decisions, commitments, approvals, etc.) are
// emitted as MeetingInsight events after pipeline processing.
// External systems subscribe via MCP notifications or poll the log.
// ──────────────────────────────────────────────────────────────

const MAX_EVENT_FILE_BYTES: u64 = 10 * 1024 * 1024; // 10MB

// ── Confidence model ──────────────────────────────────────────
// Mirrors the speaker attribution confidence system (L0–L3).
// Only Explicit + Strong should trigger downstream actions by default.

/// How confident we are that this insight was actually stated/decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InsightConfidence {
    /// Topic discussed, possible direction mentioned.
    Tentative,
    /// Inferred from discussion flow but not explicitly stated.
    Inferred,
    /// Clear discussion → conclusion pattern, strong signal.
    Strong,
    /// Explicitly stated: "We've decided...", "I commit to...", "Approved."
    Explicit,
}

impl InsightConfidence {
    /// Returns true if this confidence level should trigger downstream actions.
    pub fn is_actionable(&self) -> bool {
        matches!(
            self,
            InsightConfidence::Strong | InsightConfidence::Explicit
        )
    }
}

/// The type of structured insight extracted from a meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsightKind {
    /// "We decided X" — has rationale, optional deadline.
    Decision,
    /// "I'll do X by Y" — has owner, deliverable, deadline.
    Commitment,
    /// "Approved X" — has approver, what was approved, conditions.
    Approval,
    /// "We need to figure out X" — has context, who raised it.
    Question,
    /// "Can't proceed until X" — has dependency, owner.
    Blocker,
    /// "Let's discuss X next week" — has topic, participants, timeframe.
    FollowUp,
    /// "If X happens, we're in trouble" — has severity context.
    Risk,
}

/// A structured insight extracted from a meeting, suitable for agent subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingInsight {
    pub kind: InsightKind,
    pub content: String,
    pub confidence: InsightConfidence,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Path to the source meeting markdown file.
    pub source_meeting: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub timestamp: DateTime<Local>,
    #[serde(flatten)]
    pub event: MinutesEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type")]
pub enum MinutesEvent {
    RecordingCompleted {
        path: String,
        title: String,
        word_count: usize,
        content_type: String,
        duration: String,
    },
    AudioProcessed {
        path: String,
        title: String,
        word_count: usize,
        content_type: String,
        source_path: String,
    },
    WatchProcessed {
        path: String,
        title: String,
        word_count: usize,
        source_path: String,
    },
    NoteAdded {
        meeting_path: String,
        text: String,
    },
    VaultSynced {
        source_path: String,
        vault_path: String,
        strategy: String,
    },
    VoiceMemoProcessed {
        path: String,
        title: String,
        word_count: usize,
        source_path: String,
        device: Option<String>,
    },
    /// Audio input device changed mid-recording (e.g., Bluetooth headset connected).
    DeviceChanged {
        old_device: String,
        new_device: String,
    },
    /// Structured insight extracted from a meeting (decision, commitment, etc.).
    /// Subscribable by external systems via MCP notifications.
    MeetingInsightExtracted {
        insight: MeetingInsight,
        meeting_title: String,
    },
    /// Knowledge base updated after meeting ingestion.
    KnowledgeUpdated {
        meeting_path: String,
        facts_written: usize,
        facts_skipped: usize,
        people_updated: Vec<String>,
    },
    /// Chunked transcription started. Emitted once at the top of a chunked run
    /// so UIs can render a progress shell before any chunk finishes.
    TranscribeStarted {
        /// Absolute path to the audio file being transcribed.
        audio_path: String,
        chunk_count: u32,
        total_duration_sec: f64,
        engine: String,
        worker_count: u32,
    },
    /// A single chunk finished transcribing. `chunk_index` is zero-based and
    /// chunks MAY complete out of order — subscribers should not assume
    /// sequential arrival.
    TranscribeChunkCompleted {
        audio_path: String,
        chunk_index: u32,
        chunk_count: u32,
        start_sec: f64,
        end_sec: f64,
        words: usize,
        duration_ms: u64,
        engine: String,
    },
    /// Chunked transcription finished — emitted after all chunks assemble.
    TranscribeFinished {
        audio_path: String,
        chunk_count: u32,
        total_words: usize,
        total_duration_ms: u64,
        engine: String,
    },
}

fn events_path() -> PathBuf {
    Config::minutes_dir().join("events.jsonl")
}

fn event_log_paths() -> std::io::Result<Vec<PathBuf>> {
    let dir = Config::minutes_dir();
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut paths = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| {
                    name == "events.jsonl"
                        || (name.starts_with("events.") && name.ends_with(".jsonl"))
                })
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    paths.sort_by_key(|path| {
        path.metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH)
    });
    Ok(paths)
}

fn rotated_events_path_for(now: DateTime<Local>) -> PathBuf {
    let dir = Config::minutes_dir();
    let base = now.format("events.%Y-%m-%d-%H%M%S%3f").to_string();

    for suffix in 0.. {
        let filename = if suffix == 0 {
            format!("{base}.jsonl")
        } else {
            format!("{base}-{suffix}.jsonl")
        };
        let candidate = dir.join(filename);
        if !candidate.exists() {
            return candidate;
        }
    }

    unreachable!("rotation path generation should always find a free filename")
}

/// Append one event as a JSON line to ~/.minutes/events.jsonl.
pub fn append_event(event: MinutesEvent) {
    let envelope = EventEnvelope {
        timestamp: Local::now(),
        event,
    };

    if let Err(e) = append_event_inner(&envelope) {
        tracing::warn!(error = %e, "failed to append event");
    }
}

fn append_event_inner(envelope: &EventEnvelope) -> std::io::Result<()> {
    rotate_if_needed()?;

    let path = events_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let creating = !path.exists();
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;

    // Set 0600 on newly created files (sensitive meeting data)
    #[cfg(unix)]
    if creating {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }

    let line = serde_json::to_string(envelope).map_err(|e| std::io::Error::other(e.to_string()))?;
    writeln!(file, "{}", line)?;
    Ok(())
}

/// Read events from the log, optionally filtered by time and limited.
pub fn read_events(since: Option<DateTime<Local>>, limit: Option<usize>) -> Vec<EventEnvelope> {
    match read_events_inner(since, limit) {
        Ok(events) => events,
        Err(e) => {
            tracing::warn!(error = %e, "failed to read events");
            vec![]
        }
    }
}

fn read_events_inner(
    since: Option<DateTime<Local>>,
    limit: Option<usize>,
) -> std::io::Result<Vec<EventEnvelope>> {
    let paths = event_log_paths()?;
    if paths.is_empty() {
        return Ok(vec![]);
    }

    let mut events: Vec<EventEnvelope> = Vec::new();

    for path in paths {
        let file = fs::File::open(&path)?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<EventEnvelope>(&line) {
                Ok(envelope) => {
                    if let Some(ref since_dt) = since {
                        if envelope.timestamp < *since_dt {
                            continue;
                        }
                    }
                    events.push(envelope);
                }
                Err(e) => {
                    tracing::debug!(error = %e, path = %path.display(), "skipping malformed event line");
                }
            }
        }
    }

    events.sort_by_key(|envelope| envelope.timestamp);

    if let Some(limit) = limit {
        let skip = events.len().saturating_sub(limit);
        events = events.into_iter().skip(skip).collect();
    }

    Ok(events)
}

/// Rotate the event file if it exceeds 10MB.
fn rotate_if_needed() -> std::io::Result<()> {
    let path = events_path();
    if !path.exists() {
        return Ok(());
    }

    let metadata = fs::metadata(&path)?;
    if metadata.len() < MAX_EVENT_FILE_BYTES {
        return Ok(());
    }

    let rotated = rotated_events_path_for(Local::now());
    fs::rename(&path, &rotated)?;
    tracing::info!(
        from = %path.display(),
        to = %rotated.display(),
        "rotated event log"
    );
    Ok(())
}

// ── Insight queries ───────────────────────────────────────────

/// Filter criteria for querying meeting insights.
#[derive(Default)]
pub struct InsightFilter {
    pub kind: Option<InsightKind>,
    pub min_confidence: Option<InsightConfidence>,
    pub participant: Option<String>,
    pub since: Option<DateTime<Local>>,
    pub limit: Option<usize>,
}

/// Read MeetingInsight events from the log with filtering.
pub fn read_insights(filter: &InsightFilter) -> Vec<(DateTime<Local>, MeetingInsight, String)> {
    let events = read_events(filter.since, None);
    let mut results: Vec<(DateTime<Local>, MeetingInsight, String)> = Vec::new();

    for envelope in events {
        if let MinutesEvent::MeetingInsightExtracted {
            insight,
            meeting_title,
        } = envelope.event
        {
            if let Some(ref kind) = filter.kind {
                if insight.kind != *kind {
                    continue;
                }
            }
            if let Some(ref min_conf) = filter.min_confidence {
                if insight.confidence < *min_conf {
                    continue;
                }
            }
            if let Some(ref participant) = filter.participant {
                let p_lower = participant.to_lowercase();
                let matches = insight
                    .participants
                    .iter()
                    .any(|p| p.to_lowercase().contains(&p_lower))
                    || insight
                        .owner
                        .as_ref()
                        .is_some_and(|o| o.to_lowercase().contains(&p_lower));
                if !matches {
                    continue;
                }
            }
            results.push((envelope.timestamp, insight, meeting_title));
        }
    }

    if let Some(limit) = filter.limit {
        let skip = results.len().saturating_sub(limit);
        results = results.into_iter().skip(skip).collect();
    }

    results
}

/// Read only actionable insights (Strong or Explicit confidence).
pub fn read_actionable_insights(
    since: Option<DateTime<Local>>,
) -> Vec<(DateTime<Local>, MeetingInsight, String)> {
    read_insights(&InsightFilter {
        min_confidence: Some(InsightConfidence::Strong),
        since,
        ..Default::default()
    })
}

// ── Insight emission helpers ──────────────────────────────────

/// Emit MeetingInsight events from pipeline extraction results.
/// Called after summarization produces structured decisions/actions/commitments.
/// Deduplicates across action_items and commitments (LLMs sometimes emit the same
/// item in both lists).
pub fn emit_insights_from_summary(
    summary: &crate::summarize::Summary,
    meeting_path: &str,
    meeting_title: &str,
    participants: &[String],
) {
    let mut existing_keys = existing_insight_keys_for_meeting(meeting_path);
    // Track emitted commitment content to avoid duplicates across action_items + commitments
    let mut seen_commitments: std::collections::HashSet<String> = std::collections::HashSet::new();

    for decision in &summary.decisions {
        let confidence = infer_decision_confidence(decision);
        append_insight_if_new(
            &mut existing_keys,
            MeetingInsight {
                kind: InsightKind::Decision,
                content: decision.clone(),
                confidence,
                participants: participants.to_vec(),
                owner: None,
                deadline: None,
                topic: infer_topic_from_text(decision),
                source_meeting: meeting_path.to_string(),
            },
            meeting_title,
        );
    }

    for item in &summary.action_items {
        let (owner, task) = parse_owner_prefix(item);
        let deadline = extract_inline_deadline(item);
        let confidence = if owner.is_some() {
            InsightConfidence::Strong
        } else {
            InsightConfidence::Inferred
        };
        seen_commitments.insert(task.to_lowercase());
        append_insight_if_new(
            &mut existing_keys,
            MeetingInsight {
                kind: InsightKind::Commitment,
                content: task,
                confidence,
                participants: participants.to_vec(),
                owner,
                deadline,
                topic: None,
                source_meeting: meeting_path.to_string(),
            },
            meeting_title,
        );
    }

    for commitment in &summary.commitments {
        let (owner, content) = parse_owner_prefix(commitment);
        // Skip if already emitted from action_items
        if seen_commitments.contains(&content.to_lowercase()) {
            continue;
        }
        let deadline = extract_inline_deadline(commitment);
        append_insight_if_new(
            &mut existing_keys,
            MeetingInsight {
                kind: InsightKind::Commitment,
                content,
                confidence: InsightConfidence::Strong,
                participants: participants.to_vec(),
                owner,
                deadline,
                topic: None,
                source_meeting: meeting_path.to_string(),
            },
            meeting_title,
        );
    }

    for question in &summary.open_questions {
        let (who, content) = parse_owner_prefix(question);
        append_insight_if_new(
            &mut existing_keys,
            MeetingInsight {
                kind: InsightKind::Question,
                content,
                // Questions represent uncertainty, not decisions — Inferred, not actionable
                confidence: InsightConfidence::Inferred,
                participants: participants.to_vec(),
                owner: who,
                deadline: None,
                topic: None,
                source_meeting: meeting_path.to_string(),
            },
            meeting_title,
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct InsightKey {
    kind: InsightKind,
    content: String,
    owner: Option<String>,
    deadline: Option<String>,
    topic: Option<String>,
    source_meeting: String,
}

fn normalize_insight_field(value: &str) -> String {
    value.trim().to_lowercase()
}

fn insight_key(insight: &MeetingInsight) -> InsightKey {
    InsightKey {
        kind: insight.kind,
        content: normalize_insight_field(&insight.content),
        owner: insight.owner.as_deref().map(normalize_insight_field),
        deadline: insight.deadline.as_deref().map(normalize_insight_field),
        topic: insight.topic.as_deref().map(normalize_insight_field),
        source_meeting: normalize_insight_field(&insight.source_meeting),
    }
}

fn existing_insight_keys_for_meeting(meeting_path: &str) -> std::collections::HashSet<InsightKey> {
    let meeting_key = normalize_insight_field(meeting_path);
    read_insights(&InsightFilter::default())
        .into_iter()
        .map(|(_, insight, _)| insight)
        .filter(|insight| normalize_insight_field(&insight.source_meeting) == meeting_key)
        .map(|insight| insight_key(&insight))
        .collect()
}

fn append_insight_if_new(
    existing_keys: &mut std::collections::HashSet<InsightKey>,
    insight: MeetingInsight,
    meeting_title: &str,
) {
    let key = insight_key(&insight);
    if !existing_keys.insert(key) {
        return;
    }
    append_event(MinutesEvent::MeetingInsightExtracted {
        insight,
        meeting_title: meeting_title.to_string(),
    });
}

/// Heuristic: decisions with explicit language get Explicit confidence.
fn infer_decision_confidence(text: &str) -> InsightConfidence {
    let lower = text.to_lowercase();
    let explicit_signals = [
        "we decided",
        "we agreed",
        "decision:",
        "approved",
        "we will",
        "we're going with",
        "final decision",
        "confirmed",
    ];
    let tentative_signals = [
        "we should consider",
        "might want to",
        "we could",
        "possibly",
        "maybe",
        "thinking about",
    ];

    if explicit_signals.iter().any(|s| lower.contains(s)) {
        InsightConfidence::Explicit
    } else if tentative_signals.iter().any(|s| lower.contains(s)) {
        InsightConfidence::Tentative
    } else {
        InsightConfidence::Strong
    }
}

/// Extract "@owner: content" pattern used by the summarizer.
fn parse_owner_prefix(text: &str) -> (Option<String>, String) {
    if let Some(rest) = text.strip_prefix('@') {
        if let Some(colon_pos) = rest.find(':') {
            let owner = rest[..colon_pos].trim().to_string();
            let content = rest[colon_pos + 1..].trim().to_string();
            if !owner.is_empty() {
                return (Some(owner), content);
            }
        }
    }
    (None, text.to_string())
}

/// Extract inline deadline patterns like "(due Friday)", "(by March 21)".
/// Uses lowercased text consistently to avoid Unicode byte-index mismatches.
fn extract_inline_deadline(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for prefix in &["(due ", "(by ", "(deadline "] {
        if let Some(start) = lower.find(prefix) {
            let after = &lower[start + prefix.len()..];
            if let Some(end) = after.find(')') {
                return Some(after[..end].trim().to_string());
            }
        }
    }
    // Bare "by " — require word boundary (not preceded by a letter) to avoid
    // false positives on "nearby", "standby", "Abby", etc.
    if let Some(start) = lower.find("by ") {
        let at_word_boundary = start == 0 || !lower.as_bytes()[start - 1].is_ascii_alphabetic();
        if at_word_boundary {
            let after = &lower[start + 3..];
            let deadline: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '/')
                .collect();
            let trimmed = deadline.trim();
            if !trimmed.is_empty() && trimmed.len() <= 30 {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Infer a topic from the first clause of a text.
/// Only splits on `: `, ` – `, ` — ` (with surrounding spaces) to avoid
/// false positives on hyphenated words like "AI-powered".
fn infer_topic_from_text(text: &str) -> Option<String> {
    let separators = [": ", " – ", " — "];
    for sep in &separators {
        if let Some(pos) = text.find(sep) {
            let topic = text[..pos].trim();
            if topic.len() >= 2 && topic.len() <= 60 {
                return Some(topic.to_string());
            }
        }
    }
    None
}

/// Build an AudioProcessed event from a pipeline WriteResult.
pub fn audio_processed_event(
    result: &crate::markdown::WriteResult,
    source_path: &str,
) -> MinutesEvent {
    let content_type = match result.content_type {
        ContentType::Meeting => "meeting".to_string(),
        ContentType::Memo => "memo".to_string(),
        ContentType::Dictation => "dictation".to_string(),
    };

    MinutesEvent::AudioProcessed {
        path: result.path.display().to_string(),
        title: result.title.clone(),
        word_count: result.word_count,
        content_type,
        source_path: source_path.to_string(),
    }
}

/// Build a RecordingCompleted event from a pipeline WriteResult.
pub fn recording_completed_event(
    result: &crate::markdown::WriteResult,
    duration: &str,
) -> MinutesEvent {
    let content_type = match result.content_type {
        ContentType::Meeting => "meeting".to_string(),
        ContentType::Memo => "memo".to_string(),
        ContentType::Dictation => "dictation".to_string(),
    };

    MinutesEvent::RecordingCompleted {
        path: result.path.display().to_string(),
        title: result.title.clone(),
        word_count: result.word_count,
        content_type,
        duration: duration.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn with_temp_home<T>(f: impl FnOnce(&TempDir) -> T) -> T {
        let _guard = crate::test_home_env_lock();
        let dir = TempDir::new().unwrap();
        let original_home = std::env::var_os("HOME");
        let original_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", dir.path());
        std::env::set_var("USERPROFILE", dir.path());
        let result = f(&dir);
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        if let Some(userprofile) = original_userprofile {
            std::env::set_var("USERPROFILE", userprofile);
        } else {
            std::env::remove_var("USERPROFILE");
        }
        result
    }

    #[test]
    fn append_and_read_events() {
        with_temp_home(|_| {
            let envelope = EventEnvelope {
                timestamp: Local::now(),
                event: MinutesEvent::RecordingCompleted {
                    path: "/tmp/test.md".into(),
                    title: "Test Meeting".into(),
                    word_count: 100,
                    content_type: "meeting".into(),
                    duration: "5m".into(),
                },
            };

            append_event_inner(&envelope).unwrap();

            let events = read_events_inner(None, None).unwrap();
            assert_eq!(events.len(), 1);
            match &events[0].event {
                MinutesEvent::RecordingCompleted { title, .. } => {
                    assert_eq!(title, "Test Meeting");
                }
                _ => panic!("expected RecordingCompleted"),
            }
        });
    }

    #[test]
    fn event_envelope_serializes_with_tag() {
        let envelope = EventEnvelope {
            timestamp: Local::now(),
            event: MinutesEvent::NoteAdded {
                meeting_path: "/tmp/test.md".into(),
                text: "Important point".into(),
            },
        };

        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains("\"event_type\":\"NoteAdded\""));
        assert!(json.contains("\"text\":\"Important point\""));
    }

    #[test]
    fn read_events_returns_empty_for_missing_file() {
        with_temp_home(|_| {
            let events = read_events_inner(None, None);
            assert!(events.is_ok());
            assert!(events.unwrap().is_empty());
        });
    }

    #[test]
    fn read_events_includes_rotated_logs() {
        with_temp_home(|_| {
            let older = EventEnvelope {
                timestamp: Local::now() - chrono::Duration::minutes(10),
                event: MinutesEvent::NoteAdded {
                    meeting_path: "/tmp/older.md".into(),
                    text: "older".into(),
                },
            };
            let newer = EventEnvelope {
                timestamp: Local::now(),
                event: MinutesEvent::NoteAdded {
                    meeting_path: "/tmp/newer.md".into(),
                    text: "newer".into(),
                },
            };

            let rotated_path = rotated_events_path_for(older.timestamp);
            fs::create_dir_all(rotated_path.parent().unwrap()).unwrap();
            fs::write(
                &rotated_path,
                format!("{}\n", serde_json::to_string(&older).unwrap()),
            )
            .unwrap();
            fs::write(
                events_path(),
                format!("{}\n", serde_json::to_string(&newer).unwrap()),
            )
            .unwrap();

            let events = read_events_inner(None, None).unwrap();
            assert_eq!(events.len(), 2);
            match &events[0].event {
                MinutesEvent::NoteAdded { text, .. } => assert_eq!(text, "older"),
                _ => panic!("expected older NoteAdded"),
            }
            match &events[1].event {
                MinutesEvent::NoteAdded { text, .. } => assert_eq!(text, "newer"),
                _ => panic!("expected newer NoteAdded"),
            }
        });
    }

    #[test]
    fn transcribe_progress_events_roundtrip() {
        with_temp_home(|_| {
            let started = EventEnvelope {
                timestamp: Local::now(),
                event: MinutesEvent::TranscribeStarted {
                    audio_path: "/tmp/session.wav".into(),
                    chunk_count: 3,
                    total_duration_sec: 1800.0,
                    engine: "parakeet".into(),
                    worker_count: 2,
                },
            };
            let chunk = EventEnvelope {
                timestamp: Local::now(),
                event: MinutesEvent::TranscribeChunkCompleted {
                    audio_path: "/tmp/session.wav".into(),
                    chunk_index: 0,
                    chunk_count: 3,
                    start_sec: 0.0,
                    end_sec: 600.0,
                    words: 1234,
                    duration_ms: 42_000,
                    engine: "parakeet".into(),
                },
            };
            let finished = EventEnvelope {
                timestamp: Local::now(),
                event: MinutesEvent::TranscribeFinished {
                    audio_path: "/tmp/session.wav".into(),
                    chunk_count: 3,
                    total_words: 3800,
                    total_duration_ms: 126_000,
                    engine: "parakeet".into(),
                },
            };

            append_event_inner(&started).unwrap();
            append_event_inner(&chunk).unwrap();
            append_event_inner(&finished).unwrap();

            let events = read_events_inner(None, None).unwrap();
            assert_eq!(events.len(), 3);
            assert!(matches!(
                &events[0].event,
                MinutesEvent::TranscribeStarted {
                    chunk_count: 3,
                    worker_count: 2,
                    ..
                }
            ));
            assert!(matches!(
                &events[1].event,
                MinutesEvent::TranscribeChunkCompleted {
                    chunk_index: 0,
                    words: 1234,
                    ..
                }
            ));
            assert!(matches!(
                &events[2].event,
                MinutesEvent::TranscribeFinished {
                    total_words: 3800,
                    ..
                }
            ));

            // Tag shape — serde flatten should produce `event_type` alongside
            // `audio_path` etc., so any JSONL consumer tailing `events.jsonl`
            // sees a discriminated union.
            let raw = std::fs::read_to_string(events_path()).unwrap();
            assert!(raw.contains("\"event_type\":\"TranscribeStarted\""));
            assert!(raw.contains("\"event_type\":\"TranscribeChunkCompleted\""));
            assert!(raw.contains("\"event_type\":\"TranscribeFinished\""));
        });
    }

    #[test]
    fn rotated_events_path_adds_suffix_when_base_exists() {
        with_temp_home(|_| {
            let now = Local::now();
            let base = rotated_events_path_for(now);
            fs::create_dir_all(base.parent().unwrap()).unwrap();
            fs::write(&base, "existing").unwrap();

            let next = rotated_events_path_for(now);
            assert_ne!(base, next);
            let base_stem = base.file_stem().and_then(|name| name.to_str()).unwrap();
            let next_name = next.file_name().and_then(|name| name.to_str()).unwrap();
            assert!(
                next_name.starts_with(base_stem) && next_name.ends_with(".jsonl"),
                "expected suffixed rotation filename, got {next_name}"
            );
        });
    }

    // ── MeetingInsight tests ──────────────────────────────────

    #[test]
    fn meeting_insight_serializes_roundtrip() {
        let insight = MeetingInsight {
            kind: InsightKind::Decision,
            content: "Switch to vendor X by Q3".into(),
            confidence: InsightConfidence::Explicit,
            participants: vec!["Mat".into(), "Alex".into()],
            owner: None,
            deadline: Some("Q3 2026".into()),
            topic: Some("vendor selection".into()),
            source_meeting: "/meetings/2026-03-30-vendor-review.md".into(),
        };

        let json = serde_json::to_string(&insight).unwrap();
        let parsed: MeetingInsight = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.kind, InsightKind::Decision);
        assert_eq!(parsed.confidence, InsightConfidence::Explicit);
        assert_eq!(parsed.participants.len(), 2);
        assert_eq!(parsed.deadline.as_deref(), Some("Q3 2026"));
    }

    #[test]
    fn insight_event_serializes_with_tag() {
        let envelope = EventEnvelope {
            timestamp: Local::now(),
            event: MinutesEvent::MeetingInsightExtracted {
                insight: MeetingInsight {
                    kind: InsightKind::Commitment,
                    content: "Send pricing doc".into(),
                    confidence: InsightConfidence::Strong,
                    participants: vec![],
                    owner: Some("Sarah".into()),
                    deadline: Some("Friday".into()),
                    topic: None,
                    source_meeting: "/meetings/test.md".into(),
                },
                meeting_title: "Pricing Review".into(),
            },
        };

        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains("\"event_type\":\"MeetingInsightExtracted\""));
        assert!(json.contains("\"kind\":\"commitment\""));
        assert!(json.contains("\"confidence\":\"strong\""));

        // Round-trip
        let parsed: EventEnvelope = serde_json::from_str(&json).unwrap();
        match parsed.event {
            MinutesEvent::MeetingInsightExtracted {
                insight,
                meeting_title,
            } => {
                assert_eq!(insight.kind, InsightKind::Commitment);
                assert_eq!(insight.owner.as_deref(), Some("Sarah"));
                assert_eq!(meeting_title, "Pricing Review");
            }
            _ => panic!("expected MeetingInsightExtracted"),
        }
    }

    #[test]
    fn confidence_ordering() {
        assert!(InsightConfidence::Tentative < InsightConfidence::Inferred);
        assert!(InsightConfidence::Inferred < InsightConfidence::Strong);
        assert!(InsightConfidence::Strong < InsightConfidence::Explicit);
    }

    #[test]
    fn confidence_is_actionable() {
        assert!(!InsightConfidence::Tentative.is_actionable());
        assert!(!InsightConfidence::Inferred.is_actionable());
        assert!(InsightConfidence::Strong.is_actionable());
        assert!(InsightConfidence::Explicit.is_actionable());
    }

    #[test]
    fn infer_decision_confidence_explicit() {
        assert_eq!(
            infer_decision_confidence("We decided to switch to REST"),
            InsightConfidence::Explicit
        );
        assert_eq!(
            infer_decision_confidence("Approved the Q3 budget of $50k"),
            InsightConfidence::Explicit
        );
        assert_eq!(
            infer_decision_confidence("We agreed on monthly billing"),
            InsightConfidence::Explicit
        );
    }

    #[test]
    fn infer_decision_confidence_tentative() {
        assert_eq!(
            infer_decision_confidence("We should consider switching providers"),
            InsightConfidence::Tentative
        );
        assert_eq!(
            infer_decision_confidence("Maybe we could try a different approach"),
            InsightConfidence::Tentative
        );
    }

    #[test]
    fn infer_decision_confidence_strong_default() {
        assert_eq!(
            infer_decision_confidence("Use REST over GraphQL for the new API"),
            InsightConfidence::Strong
        );
    }

    #[test]
    fn parse_owner_prefix_with_at() {
        let (owner, content) = parse_owner_prefix("@sarah: Send pricing doc by Friday");
        assert_eq!(owner.as_deref(), Some("sarah"));
        assert_eq!(content, "Send pricing doc by Friday");
    }

    #[test]
    fn parse_owner_prefix_without_at() {
        let (owner, content) = parse_owner_prefix("Send pricing doc by Friday");
        assert!(owner.is_none());
        assert_eq!(content, "Send pricing doc by Friday");
    }

    #[test]
    fn extract_inline_deadline_parenthesized() {
        assert_eq!(
            extract_inline_deadline("Send doc (due Friday)").as_deref(),
            Some("friday")
        );
        assert_eq!(
            extract_inline_deadline("Review spec (by March 21)").as_deref(),
            Some("march 21")
        );
        assert_eq!(
            extract_inline_deadline("Ship it (deadline April 1)").as_deref(),
            Some("april 1")
        );
    }

    #[test]
    fn extract_inline_deadline_bare_by() {
        assert_eq!(
            extract_inline_deadline("Send pricing doc by Friday").as_deref(),
            Some("friday")
        );
    }

    #[test]
    fn extract_inline_deadline_no_false_positive_on_nearby() {
        // "nearby" contains "by " but should NOT match
        assert!(extract_inline_deadline("Meet at the nearby office").is_none());
    }

    #[test]
    fn extract_inline_deadline_no_false_positive_on_standby() {
        assert!(extract_inline_deadline("Standby for updates").is_none());
    }

    #[test]
    fn infer_topic_from_text_with_colon() {
        assert_eq!(
            infer_topic_from_text("Pricing: switch to monthly billing").as_deref(),
            Some("Pricing")
        );
    }

    #[test]
    fn infer_topic_from_text_with_em_dash() {
        assert_eq!(
            infer_topic_from_text("Vendor selection — switch to Acme Corp").as_deref(),
            Some("Vendor selection")
        );
    }

    #[test]
    fn infer_topic_from_text_no_separator() {
        assert!(infer_topic_from_text("Switch to monthly billing").is_none());
    }

    #[test]
    fn infer_topic_from_text_no_false_positive_on_hyphen() {
        // "AI-powered" should NOT split on the hyphen
        assert!(infer_topic_from_text("AI-powered document storage").is_none());
    }

    #[test]
    fn all_insight_kinds_serialize() {
        let kinds = [
            InsightKind::Decision,
            InsightKind::Commitment,
            InsightKind::Approval,
            InsightKind::Question,
            InsightKind::Blocker,
            InsightKind::FollowUp,
            InsightKind::Risk,
        ];
        for kind in &kinds {
            let json = serde_json::to_string(kind).unwrap();
            let parsed: InsightKind = serde_json::from_str(&json).unwrap();
            assert_eq!(*kind, parsed);
        }
    }

    #[test]
    fn emit_insights_from_summary_is_idempotent_for_same_meeting() {
        with_temp_home(|_| {
            let summary = crate::summarize::Summary {
                text: "summary".into(),
                decisions: vec!["We decided to ship it".into()],
                action_items: vec!["@mat: Send the recap by Friday".into()],
                open_questions: vec!["Who owns rollout?".into()],
                commitments: vec!["@mat: Send the recap by Friday".into()],
                key_points: vec![],
                participants: vec!["Mat".into(), "Alex".into()],
            };

            emit_insights_from_summary(
                &summary,
                "/meetings/2026-03-31-demo.md",
                "Demo Meeting",
                &summary.participants,
            );
            emit_insights_from_summary(
                &summary,
                "/meetings/2026-03-31-demo.md",
                "Demo Meeting",
                &summary.participants,
            );

            let insights = read_insights(&InsightFilter::default());
            assert_eq!(insights.len(), 3);
        });
    }

    #[test]
    fn emit_insights_from_summary_adds_only_new_items_on_retry() {
        with_temp_home(|_| {
            let initial = crate::summarize::Summary {
                text: "summary".into(),
                decisions: vec!["We decided to ship it".into()],
                action_items: vec!["@mat: Send the recap by Friday".into()],
                open_questions: vec![],
                commitments: vec![],
                key_points: vec![],
                participants: vec!["Mat".into(), "Alex".into()],
            };
            let retried = crate::summarize::Summary {
                text: "summary".into(),
                decisions: vec![
                    "We decided to ship it".into(),
                    "Use weekly rollout checkpoints".into(),
                ],
                action_items: vec!["@mat: Send the recap by Friday".into()],
                open_questions: vec!["Who owns rollout?".into()],
                commitments: vec![],
                key_points: vec![],
                participants: vec!["Mat".into(), "Alex".into()],
            };

            emit_insights_from_summary(
                &initial,
                "/meetings/2026-03-31-demo.md",
                "Demo Meeting",
                &initial.participants,
            );
            emit_insights_from_summary(
                &retried,
                "/meetings/2026-03-31-demo.md",
                "Demo Meeting",
                &retried.participants,
            );

            let insights = read_insights(&InsightFilter::default());
            assert_eq!(insights.len(), 4);
            let contents = insights
                .into_iter()
                .map(|(_, insight, _)| insight.content)
                .collect::<Vec<_>>();
            assert_eq!(
                contents,
                vec![
                    "We decided to ship it",
                    "Send the recap by Friday",
                    "Use weekly rollout checkpoints",
                    "Who owns rollout?",
                ]
            );
        });
    }
}
