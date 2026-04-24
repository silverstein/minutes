use crate::diarize::{AttributionSource, Confidence, SpeakerAttribution};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OverlayError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct SpeakerConfirmation {
    pub meeting_key: String,
    pub speaker_label: String,
    pub name: String,
    pub confidence: Confidence,
    pub source: AttributionSource,
    pub reversible_to: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
}

/// Database path: ~/.minutes/overlays.db
///
/// This is additive state layered over immutable meeting markdown. Deleting it
/// removes user-confirmed corrections but never damages raw capture files.
pub fn default_db_path() -> PathBuf {
    dirs::home_dir()
        .expect("home directory must exist for overlays.db")
        .join(".minutes")
        .join("overlays.db")
}

pub fn db_path() -> PathBuf {
    let path = default_db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    path
}

/// Resolve the overlay database next to a graph database.
///
/// Production graph rebuilds use ~/.minutes/graph.db, so this resolves to
/// ~/.minutes/overlays.db. Tests pass a temp graph path and get an isolated
/// temp overlays.db beside it.
pub fn db_path_for_graph_path(graph_path: &Path) -> PathBuf {
    graph_path
        .parent()
        .map(|parent| parent.join("overlays.db"))
        .unwrap_or_else(db_path)
}

#[cfg(unix)]
fn set_db_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
    }
}

#[cfg(not(unix))]
fn set_db_permissions(_path: &Path) {}

fn open_db(path: &Path) -> Result<Connection, OverlayError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    create_schema(&conn)?;
    set_db_permissions(path);
    Ok(conn)
}

fn create_schema(conn: &Connection) -> Result<(), OverlayError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS overlays (
            id INTEGER PRIMARY KEY,
            entity_key TEXT NOT NULL,
            overlay_type TEXT NOT NULL,
            value TEXT NOT NULL,
            confidence TEXT NOT NULL,
            source TEXT NOT NULL,
            reversible_to TEXT,
            note TEXT,
            created_at TEXT NOT NULL,
            UNIQUE(entity_key, overlay_type)
        );
        CREATE INDEX IF NOT EXISTS idx_overlays_entity_key ON overlays(entity_key);
        CREATE INDEX IF NOT EXISTS idx_overlays_type ON overlays(overlay_type);",
    )?;
    Ok(())
}

fn meeting_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn speaker_entity_key(meeting_path: &Path, speaker_label: &str) -> String {
    format!(
        "meeting:{}#speaker:{}",
        meeting_key(meeting_path),
        speaker_label
    )
}

pub fn write_speaker_confirmation(
    meeting_path: &Path,
    speaker_label: &str,
    name: &str,
    reversible_to: Option<&str>,
    note: Option<&str>,
) -> Result<SpeakerConfirmation, OverlayError> {
    write_speaker_confirmation_at(
        &db_path(),
        meeting_path,
        speaker_label,
        name,
        reversible_to,
        note,
    )
}

pub fn write_speaker_confirmation_at(
    db_path: &Path,
    meeting_path: &Path,
    speaker_label: &str,
    name: &str,
    reversible_to: Option<&str>,
    note: Option<&str>,
) -> Result<SpeakerConfirmation, OverlayError> {
    let conn = open_db(db_path)?;
    let created_at = chrono::Utc::now().to_rfc3339();
    let entity_key = speaker_entity_key(meeting_path, speaker_label);

    conn.execute(
        "INSERT INTO overlays
            (entity_key, overlay_type, value, confidence, source, reversible_to, note, created_at)
         VALUES (?1, 'speaker', ?2, 'high', 'manual', ?3, ?4, ?5)
         ON CONFLICT(entity_key, overlay_type) DO UPDATE SET
            value = excluded.value,
            confidence = excluded.confidence,
            source = excluded.source,
            reversible_to = excluded.reversible_to,
            note = excluded.note,
            created_at = excluded.created_at",
        params![entity_key, name, reversible_to, note, created_at],
    )?;

    Ok(SpeakerConfirmation {
        meeting_key: meeting_key(meeting_path),
        speaker_label: speaker_label.to_string(),
        name: name.to_string(),
        confidence: Confidence::High,
        source: AttributionSource::Manual,
        reversible_to: reversible_to.map(str::to_string),
        note: note.map(str::to_string),
        created_at,
    })
}

pub fn load_speaker_confirmations_for_meeting_at(
    db_path: &Path,
    meeting_path: &Path,
) -> Result<Vec<SpeakerConfirmation>, OverlayError> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }

    let conn = open_db(db_path)?;
    let meeting_key = meeting_key(meeting_path);
    let prefix = format!("meeting:{}#speaker:", meeting_key);
    let like = format!("{prefix}%");
    let mut stmt = conn.prepare(
        "SELECT entity_key, value, confidence, source, reversible_to, note, created_at
         FROM overlays
         WHERE overlay_type = 'speaker' AND entity_key LIKE ?1
         ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![like], |row| {
        let entity_key: String = row.get(0)?;
        let speaker_label = entity_key
            .strip_prefix(&prefix)
            .unwrap_or_default()
            .to_string();
        Ok(SpeakerConfirmation {
            meeting_key: meeting_key.clone(),
            speaker_label,
            name: row.get(1)?,
            confidence: Confidence::High,
            source: AttributionSource::Manual,
            reversible_to: row.get(4)?,
            note: row.get(5)?,
            created_at: row.get(6)?,
        })
    })?;

    let mut confirmations = Vec::new();
    for row in rows {
        let confirmation = row?;
        if !confirmation.speaker_label.is_empty() {
            confirmations.push(confirmation);
        }
    }
    Ok(confirmations)
}

pub fn apply_speaker_confirmations(
    speaker_map: &mut Vec<SpeakerAttribution>,
    confirmations: &[SpeakerConfirmation],
) {
    for confirmation in confirmations {
        if let Some(existing) = speaker_map
            .iter_mut()
            .find(|attr| attr.speaker_label == confirmation.speaker_label)
        {
            existing.name = confirmation.name.clone();
            existing.confidence = Confidence::High;
            existing.source = AttributionSource::Manual;
        } else {
            speaker_map.push(SpeakerAttribution {
                speaker_label: confirmation.speaker_label.clone(),
                name: confirmation.name.clone(),
                confidence: Confidence::High,
                source: AttributionSource::Manual,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn speaker_confirmation_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("overlays.db");
        let meeting = tmp.path().join("meeting.md");
        std::fs::write(&meeting, "---\ntitle: Test\n---\n").unwrap();

        write_speaker_confirmation_at(
            &db,
            &meeting,
            "SPEAKER_0",
            "Alex Kim",
            Some("Speaker 0"),
            Some("confirmed in test"),
        )
        .unwrap();

        let confirmations = load_speaker_confirmations_for_meeting_at(&db, &meeting).unwrap();
        assert_eq!(confirmations.len(), 1);
        assert_eq!(confirmations[0].speaker_label, "SPEAKER_0");
        assert_eq!(confirmations[0].name, "Alex Kim");
        assert_eq!(confirmations[0].reversible_to.as_deref(), Some("Speaker 0"));
    }
}
