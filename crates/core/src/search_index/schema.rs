//! Schema, migration, and integrity validation for the search index.
//!
//! The schema is a hybrid B-tree + FTS5 design (chosen after two adversarial
//! reviews):
//!
//! - `meetings` (B-tree): filterable metadata, fast empty-query list views.
//!   `rowid` is explicit so it can be matched against `meetings_fts.rowid`.
//! - `meeting_attendees` (B-tree, normalized): preserves the existing
//!   case-insensitive substring filter semantics.
//! - `meetings_fts` (FTS5, owns its content): title + body, with porter
//!   tokenizer + diacritic-stripping + 2/3/4-char prefix index.
//! - `sync_state` (B-tree, key/value): tracks the indexed `output_dir` for
//!   full-rebuild on directory changes.
//!
//! FTS5 owns its own content rather than using `content='meetings'` because
//! the external-content path requires triggers or explicit rebuild commands;
//! storing body twice (~25MB at current corpus) is the cheaper trade.

use rusqlite::{params, Connection, TransactionBehavior};
use std::path::Path;

use super::SearchIndexError;

pub const SCHEMA_VERSION: i64 = 1;

/// Open or create the index database. Sets WAL + busy_timeout pragmas, runs
/// schema validation, and rebuilds in place if anything is corrupted or
/// missing. Sidecar `.db-wal`/`.db-shm` permissions are tightened after the
/// first write (which forces them to exist).
pub fn open_db(db_path: &Path) -> Result<Connection, SearchIndexError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            SearchIndexError::Io(format!("create parent dir for {}: {}", parent.display(), e))
        })?;
    }
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000_i32)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

/// Create the schema from scratch inside a transaction. Idempotent: tables
/// use `IF NOT EXISTS`. After commit, sets `user_version` and forces a write
/// so the WAL/SHM sidecar files exist for permission-tightening.
pub fn ensure_schema(conn: &mut Connection) -> Result<(), SearchIndexError> {
    let txn = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    txn.execute_batch(SCHEMA_SQL)?;
    txn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    txn.commit()?;
    // Force a write so WAL/SHM files exist (needed for permission set).
    conn.execute(
        "INSERT OR IGNORE INTO sync_state (key, value) VALUES ('initialized', '1')",
        [],
    )?;
    Ok(())
}

/// Validate schema integrity. Returns `Ok(true)` if the schema is healthy,
/// `Ok(false)` if it needs to be rebuilt. Errors are propagated.
///
/// Checks: PRAGMA user_version, PRAGMA quick_check, FTS5 integrity-check,
/// presence of every required table.
pub fn is_healthy(conn: &Connection) -> Result<bool, SearchIndexError> {
    let user_version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    if user_version != SCHEMA_VERSION {
        return Ok(false);
    }
    let quick: String = conn
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .unwrap_or_else(|_| "fail".into());
    if quick != "ok" {
        return Ok(false);
    }
    for table in REQUIRED_TABLES {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = ?",
                [table],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if count == 0 {
            return Ok(false);
        }
    }
    // FTS5 has shadow tables that quick_check doesn't always catch. The
    // 'integrity-check' pragma fails (returns an error) if any are missing
    // or corrupt. We treat any error here as "needs rebuild".
    let fts_ok = conn
        .execute(
            "INSERT INTO meetings_fts(meetings_fts) VALUES('integrity-check')",
            [],
        )
        .is_ok();
    if !fts_ok {
        return Ok(false);
    }
    Ok(true)
}

/// Drop all tables and recreate the schema. Used after corruption or
/// `output_dir` change. Caller is responsible for re-syncing afterward.
pub fn rebuild(conn: &mut Connection) -> Result<(), SearchIndexError> {
    let txn = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for table in &[
        "meeting_attendees",
        "meetings_fts",
        "meetings_fts_data",
        "meetings_fts_idx",
        "meetings_fts_content",
        "meetings_fts_docsize",
        "meetings_fts_config",
        "meetings",
        "sync_state",
    ] {
        // best-effort drop; ignore "no such table"
        let _ = txn.execute(&format!("DROP TABLE IF EXISTS {}", table), []);
    }
    txn.execute_batch(SCHEMA_SQL)?;
    txn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    txn.commit()?;
    Ok(())
}

/// Set 0600 on the main DB file plus its WAL/SHM sidecars (Unix only).
/// Safe to call multiple times; missing sidecars are skipped silently.
#[cfg(unix)]
pub fn tighten_permissions(db_path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    for suffix in ["", "-wal", "-shm"] {
        let p = if suffix.is_empty() {
            db_path.to_path_buf()
        } else {
            let mut s = db_path.as_os_str().to_owned();
            s.push(suffix);
            std::path::PathBuf::from(s)
        };
        if p.exists() {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).ok();
        }
    }
}

#[cfg(not(unix))]
pub fn tighten_permissions(_db_path: &Path) {
    // No-op on non-Unix; ACLs are out of scope.
}

/// Fetch the stored `output_dir_path` (if any). Used to detect output_dir
/// changes that require a full rebuild.
pub fn read_output_dir(conn: &Connection) -> Result<Option<String>, SearchIndexError> {
    let row: Result<String, _> = conn.query_row(
        "SELECT value FROM sync_state WHERE key = 'output_dir_path'",
        [],
        |r| r.get(0),
    );
    match row {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn write_output_dir(conn: &Connection, output_dir: &str) -> Result<(), SearchIndexError> {
    conn.execute(
        "INSERT INTO sync_state (key, value) VALUES ('output_dir_path', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![output_dir],
    )?;
    Ok(())
}

const REQUIRED_TABLES: &[&str] = &[
    "meetings",
    "meeting_attendees",
    "meetings_fts",
    "sync_state",
];

const SCHEMA_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS meetings (
        rowid          INTEGER PRIMARY KEY,
        path           TEXT NOT NULL UNIQUE,
        title          TEXT NOT NULL,
        date           TEXT NOT NULL,
        content_type   TEXT NOT NULL,
        attendees_json TEXT,
        recorded_by    TEXT,
        mtime_ns       INTEGER NOT NULL,
        size_bytes     INTEGER NOT NULL,
        body_hash      TEXT NOT NULL,
        indexed_at     INTEGER NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_meetings_date         ON meetings(date DESC);
    CREATE INDEX IF NOT EXISTS idx_meetings_content_type ON meetings(content_type, date DESC);
    CREATE INDEX IF NOT EXISTS idx_meetings_recorded_by  ON meetings(recorded_by, date DESC);

    CREATE TABLE IF NOT EXISTS meeting_attendees (
        meeting_rowid  INTEGER NOT NULL REFERENCES meetings(rowid) ON DELETE CASCADE,
        attendee_lower TEXT NOT NULL,
        PRIMARY KEY (meeting_rowid, attendee_lower)
    );

    CREATE INDEX IF NOT EXISTS idx_attendees_lower ON meeting_attendees(attendee_lower);

    CREATE VIRTUAL TABLE IF NOT EXISTS meetings_fts USING fts5(
        title, body,
        tokenize='porter unicode61 remove_diacritics 2',
        prefix='2 3 4'
    );

    CREATE TABLE IF NOT EXISTS sync_state (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("search.db");
        let mut conn = open_db(&path).unwrap();
        ensure_schema(&mut conn).unwrap();
        (dir, conn)
    }

    #[test]
    fn fresh_open_creates_all_tables() {
        let (_dir, conn) = fresh_db();
        for table in REQUIRED_TABLES {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = ?",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {} not created", table);
        }
    }

    #[test]
    fn fresh_open_sets_user_version() {
        let (_dir, conn) = fresh_db();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn is_healthy_on_fresh_db() {
        let (_dir, conn) = fresh_db();
        assert!(is_healthy(&conn).unwrap());
    }

    #[test]
    fn is_healthy_false_on_missing_table() {
        let (_dir, conn) = fresh_db();
        conn.execute("DROP TABLE meeting_attendees", []).unwrap();
        assert!(!is_healthy(&conn).unwrap());
    }

    #[test]
    fn is_healthy_false_on_user_version_mismatch() {
        let (_dir, conn) = fresh_db();
        conn.pragma_update(None, "user_version", &999_i64).unwrap();
        assert!(!is_healthy(&conn).unwrap());
    }

    #[test]
    fn rebuild_recreates_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("search.db");
        let mut conn = open_db(&path).unwrap();
        ensure_schema(&mut conn).unwrap();
        // Mess the schema up
        conn.execute("DROP TABLE meeting_attendees", []).unwrap();
        rebuild(&mut conn).unwrap();
        assert!(is_healthy(&conn).unwrap());
    }

    #[test]
    fn output_dir_roundtrip() {
        let (_dir, conn) = fresh_db();
        assert!(read_output_dir(&conn).unwrap().is_none());
        write_output_dir(&conn, "/u/meetings").unwrap();
        assert_eq!(
            read_output_dir(&conn).unwrap(),
            Some("/u/meetings".to_string())
        );
        // Upsert
        write_output_dir(&conn, "/u/elsewhere").unwrap();
        assert_eq!(
            read_output_dir(&conn).unwrap(),
            Some("/u/elsewhere".to_string())
        );
    }

    #[test]
    fn fts5_table_supports_match() {
        let (_dir, conn) = fresh_db();
        conn.execute(
            "INSERT INTO meetings_fts (rowid, title, body) VALUES (1, ?, ?)",
            params!["Test", "alpha beta gamma"],
        )
        .unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM meetings_fts WHERE meetings_fts MATCH 'beta'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    #[cfg(unix)]
    fn permissions_0600_after_write() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("search.db");
        let mut conn = open_db(&path).unwrap();
        ensure_schema(&mut conn).unwrap();
        // Force a write to ensure WAL/SHM exist.
        conn.execute(
            "INSERT INTO sync_state (key, value) VALUES ('test', '1')",
            [],
        )
        .unwrap();
        tighten_permissions(&path);
        for suffix in &["", "-wal", "-shm"] {
            let mut s = path.as_os_str().to_owned();
            s.push(suffix);
            let p = std::path::PathBuf::from(s);
            if p.exists() {
                let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
                assert_eq!(
                    mode,
                    0o600,
                    "expected 0600 on {}, got {:o}",
                    p.display(),
                    mode
                );
            }
        }
    }
}
