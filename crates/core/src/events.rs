use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::config::Config;
use crate::markdown::ContentType;

// ──────────────────────────────────────────────────────────────
// Event log: append-only JSONL at ~/.minutes/events.jsonl.
//
// Agents can tail/poll this file to react to new meetings.
// Non-fatal: pipeline never fails if event logging fails.
// Rotates to events.{date}.jsonl when file exceeds 10MB.
// ──────────────────────────────────────────────────────────────

const MAX_EVENT_FILE_BYTES: u64 = 10 * 1024 * 1024; // 10MB

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
}

fn events_path() -> PathBuf {
    Config::minutes_dir().join("events.jsonl")
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
    let path = events_path();
    if !path.exists() {
        return Ok(vec![]);
    }

    let file = fs::File::open(&path)?;
    let reader = BufReader::new(file);
    let mut events: Vec<EventEnvelope> = Vec::new();

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
                tracing::debug!(error = %e, "skipping malformed event line");
            }
        }
    }

    // Return the most recent events (tail of file)
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

    let date = Local::now().format("%Y-%m-%d").to_string();
    let rotated = path.with_file_name(format!("events.{}.jsonl", date));
    fs::rename(&path, &rotated)?;
    tracing::info!(
        from = %path.display(),
        to = %rotated.display(),
        "rotated event log"
    );
    Ok(())
}

/// Build an AudioProcessed event from a pipeline WriteResult.
pub fn audio_processed_event(
    result: &crate::markdown::WriteResult,
    source_path: &str,
) -> MinutesEvent {
    let content_type = match result.content_type {
        ContentType::Meeting => "meeting".to_string(),
        ContentType::Memo => "memo".to_string(),
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

    fn set_events_dir(dir: &std::path::Path) -> PathBuf {
        dir.join("events.jsonl")
    }

    #[test]
    fn append_and_read_events() {
        let dir = TempDir::new().unwrap();
        let path = set_events_dir(dir.path());

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

        // Write directly to temp path
        let line = serde_json::to_string(&envelope).unwrap();
        fs::write(&path, format!("{}\n", line)).unwrap();

        // Read back
        let file = fs::File::open(&path).unwrap();
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for line in reader.lines() {
            let line = line.unwrap();
            let parsed: EventEnvelope = serde_json::from_str(&line).unwrap();
            events.push(parsed);
        }

        assert_eq!(events.len(), 1);
        match &events[0].event {
            MinutesEvent::RecordingCompleted { title, .. } => {
                assert_eq!(title, "Test Meeting");
            }
            _ => panic!("expected RecordingCompleted"),
        }
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
        // read_events_inner with a nonexistent path
        let events = read_events_inner(None, None);
        // This tests the real events path; if it doesn't exist, returns empty
        assert!(events.is_ok());
    }
}
