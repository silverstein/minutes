//! SQLite (B-tree + FTS5) backed search index.
//!
//! Replaces the legacy walk-and-grep search implementation with an index that:
//!
//! - Hits sub-5ms per query on a warm corpus.
//! - Stays current via a watcher coalescer (Tauri side, see `coalescer`) and
//!   per-file mtime+size diff (`SyncMode::Auto`) on every CLI invocation.
//! - Sanitizes user input through [`sanitize::sanitize_fts_query`] so real
//!   meeting names with colons, hyphens, slashes, and quotes don't error.
//! - Shares the [`exclusions::is_excluded_path`] predicate with the legacy
//!   walker, so archived/processed/failed directories never enter the index.
//! - Survives partial corruption via `PRAGMA quick_check` plus FTS5
//!   `integrity-check` validation on open, with full-rebuild fallback.
//!
//! See `.claude/search-fts5-plan.local.md` for the full design rationale and
//! the two adversarial-review passes that shaped the architecture.

pub mod exclusions;
pub mod retry;
pub mod sanitize;
pub mod schema;

use crate::config::Config;
use crate::error::SearchError;
use crate::markdown::{extract_field, split_frontmatter};
use crate::search::{SearchFilters, SearchResult};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use exclusions::is_excluded_path;
use retry::with_retry_on_busy;
use sanitize::sanitize_fts_query;

/// Errors from the search index. Convertible into [`SearchError`] for the
/// existing public API in [`crate::search`].
#[derive(Debug, thiserror::Error)]
pub enum SearchIndexError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(String),

    #[error("frontmatter parse error in {path}: {message}")]
    Frontmatter { path: String, message: String },
}

impl From<SearchIndexError> for SearchError {
    fn from(e: SearchIndexError) -> Self {
        SearchError::Index(e.to_string())
    }
}

/// How aggressively the search index should sync filesystem state before
/// answering a query.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Per-file mtime+size scan. Default. ~5ms at 155 docs, ~50ms at 10K.
    /// Catches in-place edits while Tauri (and its watcher) is closed.
    #[default]
    Auto,
    /// Full re-walk + reindex. Catches mtime-collision edge cases.
    Force,
    /// Skip sync entirely; query the index as-is. For piped/scripted use.
    Skip,
}

/// Stats for the most recent sync, returned for logging/telemetry.
#[derive(Debug, Default, Clone, Serialize)]
pub struct SyncStats {
    pub indexed: usize,
    pub updated: usize,
    pub removed: usize,
    pub errored: usize,
    pub duration_ms: u64,
}

/// SQLite-backed search index. The connection is mutex-guarded because
/// `rusqlite::Connection` is `!Sync`.
pub struct SearchIndex {
    conn: Mutex<Connection>,
    db_path: PathBuf,
}

impl SearchIndex {
    /// Default location: `~/.minutes/search.db`.
    pub fn default_db_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".minutes")
            .join("search.db")
    }

    /// Open or create the index. Validates schema, rebuilds if corrupted,
    /// triggers a full rebuild if `output_dir` has changed since last open.
    pub fn open(config: &Config) -> Result<Self, SearchIndexError> {
        let db_path = Self::default_db_path();
        let mut conn = schema::open_db(&db_path)?;

        // Ensure schema exists. Idempotent on existing DBs.
        if let Err(e) = schema::ensure_schema(&mut conn) {
            tracing::warn!(?e, "ensure_schema failed; rebuilding");
            schema::rebuild(&mut conn)?;
        }

        // Validate. If unhealthy, rebuild.
        match schema::is_healthy(&conn) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                tracing::warn!("search index unhealthy; rebuilding");
                schema::rebuild(&mut conn)?;
            }
        }

        // Detect output_dir change → full rebuild.
        let output_dir_str = config.output_dir.to_string_lossy().into_owned();
        if let Some(stored) = schema::read_output_dir(&conn)? {
            if stored != output_dir_str {
                tracing::info!(old = %stored, new = %output_dir_str, "output_dir changed; rebuilding index");
                schema::rebuild(&mut conn)?;
                schema::write_output_dir(&conn, &output_dir_str)?;
            }
        } else {
            schema::write_output_dir(&conn, &output_dir_str)?;
        }

        schema::tighten_permissions(&db_path);

        Ok(SearchIndex {
            conn: Mutex::new(conn),
            db_path,
        })
    }

    /// Sync filesystem state into the index per the requested mode.
    pub fn sync(&self, config: &Config, mode: SyncMode) -> Result<SyncStats, SearchIndexError> {
        let start = std::time::Instant::now();
        let mut stats = SyncStats::default();

        if mode == SyncMode::Skip {
            return Ok(stats);
        }

        let dir = config.output_dir.clone();
        if !dir.exists() {
            return Ok(stats);
        }

        if mode == SyncMode::Force {
            let mut conn = self.conn.lock().unwrap();
            schema::rebuild(&mut conn)?;
            schema::write_output_dir(&conn, &dir.to_string_lossy())?;
            // Fall through to Auto-style scan to repopulate.
        }

        // Walk + per-file diff
        let mut seen_paths: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

        for entry in walkdir::WalkDir::new(&dir)
            .follow_links(true)
            .into_iter()
            .filter_entry(|e| {
                if e.file_type().is_dir() {
                    !is_excluded_path(e.path(), &dir)
                } else {
                    true
                }
            })
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().and_then(|s| s.to_str()) == Some("md")
                    && !is_excluded_path(e.path(), &dir)
            })
        {
            let path = entry.path().to_path_buf();
            seen_paths.insert(path.clone());

            let meta = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime_ns = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);
            let size_bytes = meta.len() as i64;

            let needs_index = {
                let conn = self.conn.lock().unwrap();
                let row: Option<(i64, i64)> = conn
                    .query_row(
                        "SELECT mtime_ns, size_bytes FROM meetings WHERE path = ?",
                        params![path.to_string_lossy()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                match row {
                    None => true,
                    Some((stored_m, stored_s)) => stored_m != mtime_ns || stored_s != size_bytes,
                }
            };

            if !needs_index {
                continue;
            }

            match self.upsert_file_inner(&path, mtime_ns, size_bytes) {
                Ok(true) => stats.indexed += 1,
                Ok(false) => stats.updated += 1,
                Err(e) => {
                    tracing::warn!(?e, path = %path.display(), "upsert failed");
                    stats.errored += 1;
                }
            }
        }

        // Find paths in the index that no longer exist on disk.
        let removed = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare("SELECT path FROM meetings")?;
            let rows: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .collect();
            rows.into_iter()
                .filter(|p| !seen_paths.contains(&PathBuf::from(p)))
                .collect::<Vec<_>>()
        };
        for p in removed {
            if let Err(e) = self.delete_file(&PathBuf::from(&p)) {
                tracing::warn!(?e, path = %p, "delete failed");
                stats.errored += 1;
            } else {
                stats.removed += 1;
            }
        }

        stats.duration_ms = start.elapsed().as_millis() as u64;
        Ok(stats)
    }

    /// Index one file. Called from the watcher coalescer or explicit sync.
    /// Returns `Ok(true)` if a new row was inserted, `Ok(false)` if updated.
    pub fn upsert_file(&self, path: &Path) -> Result<(), SearchIndexError> {
        let meta = std::fs::metadata(path)
            .map_err(|e| SearchIndexError::Io(format!("stat {}: {}", path.display(), e)))?;
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let size_bytes = meta.len() as i64;
        self.upsert_file_inner(path, mtime_ns, size_bytes)
            .map(|_| ())
    }

    fn upsert_file_inner(
        &self,
        path: &Path,
        mtime_ns: i64,
        size_bytes: i64,
    ) -> Result<bool, SearchIndexError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| SearchIndexError::Io(format!("read {}: {}", path.display(), e)))?;
        let (frontmatter, body) = split_frontmatter(&content);
        let title = extract_field(frontmatter, "title").unwrap_or_default();
        let date = extract_field(frontmatter, "date").unwrap_or_default();
        let content_type = extract_field(frontmatter, "type").unwrap_or_else(|| "meeting".into());
        let recorded_by = extract_field(frontmatter, "recorded_by").unwrap_or_default();
        let attendees_raw = extract_field(frontmatter, "attendees").unwrap_or_default();
        let attendees: Vec<String> = attendees_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let attendees_json = serde_json::to_string(&attendees).unwrap_or_else(|_| "[]".into());
        let body_hash = hash_body(body);
        let indexed_at = chrono::Local::now().timestamp();
        let path_str = path.to_string_lossy().into_owned();

        let mut conn = self.conn.lock().unwrap();
        with_retry_on_busy(|| {
            let txn = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let rowid: i64 = txn.query_row(
                "INSERT INTO meetings
                    (path, title, date, content_type, attendees_json, recorded_by,
                     mtime_ns, size_bytes, body_hash, indexed_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(path) DO UPDATE SET
                     title = excluded.title,
                     date = excluded.date,
                     content_type = excluded.content_type,
                     attendees_json = excluded.attendees_json,
                     recorded_by = excluded.recorded_by,
                     mtime_ns = excluded.mtime_ns,
                     size_bytes = excluded.size_bytes,
                     body_hash = excluded.body_hash,
                     indexed_at = excluded.indexed_at
                 RETURNING rowid",
                params![
                    path_str,
                    title,
                    date,
                    content_type,
                    attendees_json,
                    recorded_by,
                    mtime_ns,
                    size_bytes,
                    body_hash,
                    indexed_at
                ],
                |r| r.get(0),
            )?;

            // Replace FTS row (FTS5 has no UPSERT).
            txn.execute("DELETE FROM meetings_fts WHERE rowid = ?", [rowid])?;
            txn.execute(
                "INSERT INTO meetings_fts (rowid, title, body) VALUES (?, ?, ?)",
                params![rowid, title, body],
            )?;

            // Replace attendees.
            txn.execute(
                "DELETE FROM meeting_attendees WHERE meeting_rowid = ?",
                [rowid],
            )?;
            for attendee in &attendees {
                txn.execute(
                    "INSERT OR IGNORE INTO meeting_attendees (meeting_rowid, attendee_lower)
                     VALUES (?, ?)",
                    params![rowid, attendee.to_lowercase()],
                )?;
            }
            txn.commit()
        })?;
        Ok(true)
    }

    /// Remove one file from the index. CASCADE handles meeting_attendees.
    /// Idempotent: missing rows are a no-op.
    pub fn delete_file(&self, path: &Path) -> Result<(), SearchIndexError> {
        let path_str = path.to_string_lossy().into_owned();
        let mut conn = self.conn.lock().unwrap();
        with_retry_on_busy(|| {
            let txn = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Look up rowid for FTS cleanup
            let rowid: Option<i64> = txn
                .query_row(
                    "SELECT rowid FROM meetings WHERE path = ?",
                    params![path_str],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = rowid {
                txn.execute("DELETE FROM meetings_fts WHERE rowid = ?", [id])?;
                txn.execute("DELETE FROM meetings WHERE rowid = ?", [id])?;
            }
            txn.commit()
        })?;
        Ok(())
    }

    /// Run a search. Empty query → list mode (B-tree). Non-empty → FTS5 MATCH.
    pub fn search(
        &self,
        query: &str,
        filters: &SearchFilters,
        limit: Option<usize>,
    ) -> Result<Vec<SearchResult>, SearchIndexError> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.unwrap_or(usize::MAX);
        if query.trim().is_empty() {
            return search_list(&conn, filters, limit);
        }
        let sanitized = sanitize_fts_query(query);
        if sanitized.is_empty() {
            // All-punctuation input. Caller treats this as "no match" rather than error.
            return Ok(Vec::new());
        }
        search_match(&conn, &sanitized, filters, limit)
    }

    /// Force full rebuild + resync. Corruption recovery, output_dir change.
    pub fn rebuild(&self, config: &Config) -> Result<SyncStats, SearchIndexError> {
        {
            let mut conn = self.conn.lock().unwrap();
            schema::rebuild(&mut conn)?;
            schema::write_output_dir(&conn, &config.output_dir.to_string_lossy())?;
        }
        self.sync(config, SyncMode::Auto)
    }

    /// Path to the underlying SQLite file. Useful for tests + diagnostics.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}

/// Empty-query path: no MATCH, just B-tree filtered list ordered by date.
fn search_list(
    conn: &Connection,
    filters: &SearchFilters,
    limit: usize,
) -> Result<Vec<SearchResult>, SearchIndexError> {
    let mut sql = String::from(
        "SELECT m.path, m.title, m.date, m.content_type
         FROM meetings m
         WHERE 1=1",
    );
    let mut args: Vec<String> = Vec::new();
    if let Some(ct) = &filters.content_type {
        sql.push_str(" AND m.content_type = ?");
        args.push(ct.clone());
    }
    if let Some(since) = &filters.since {
        sql.push_str(" AND m.date >= ?");
        args.push(since.clone());
    }
    if let Some(rb) = &filters.recorded_by {
        sql.push_str(" AND m.recorded_by LIKE ?");
        args.push(format!("%{}%", rb));
    }
    if let Some(att) = &filters.attendee {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM meeting_attendees a
                           WHERE a.meeting_rowid = m.rowid
                             AND a.attendee_lower LIKE ?)",
        );
        args.push(format!("%{}%", att.to_lowercase()));
    }
    sql.push_str(" ORDER BY m.date DESC LIMIT ?");

    let mut stmt = conn.prepare(&sql)?;
    let mut bound: Vec<&dyn rusqlite::ToSql> =
        args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let limit_i64 = limit as i64;
    bound.push(&limit_i64);

    let rows = stmt
        .query_map(rusqlite::params_from_iter(bound.iter()), |r| {
            Ok(SearchResult {
                path: PathBuf::from(r.get::<_, String>(0)?),
                title: r.get(1)?,
                date: r.get(2)?,
                content_type: r.get(3)?,
                snippet: String::new(),
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Non-empty-query path: FTS5 MATCH joined to meetings for filters + ordering.
fn search_match(
    conn: &Connection,
    sanitized_query: &str,
    filters: &SearchFilters,
    limit: usize,
) -> Result<Vec<SearchResult>, SearchIndexError> {
    // Use rare control characters as snippet delimiters so we can strip them
    // without confusing legitimate transcript content like `<Alex>` or `<code>`.
    const SNIP_OPEN: char = '\u{2}';
    const SNIP_CLOSE: char = '\u{3}';

    let mut sql = String::from(
        "SELECT m.path, m.title, m.date, m.content_type,
                snippet(meetings_fts, 1, char(2), char(3), '…', 24) AS snippet
         FROM meetings_fts
         JOIN meetings m ON m.rowid = meetings_fts.rowid
         WHERE meetings_fts MATCH ?",
    );
    let mut args: Vec<String> = vec![sanitized_query.to_string()];
    if let Some(ct) = &filters.content_type {
        sql.push_str(" AND m.content_type = ?");
        args.push(ct.clone());
    }
    if let Some(since) = &filters.since {
        sql.push_str(" AND m.date >= ?");
        args.push(since.clone());
    }
    if let Some(rb) = &filters.recorded_by {
        sql.push_str(" AND m.recorded_by LIKE ?");
        args.push(format!("%{}%", rb));
    }
    if let Some(att) = &filters.attendee {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM meeting_attendees a
                           WHERE a.meeting_rowid = m.rowid
                             AND a.attendee_lower LIKE ?)",
        );
        args.push(format!("%{}%", att.to_lowercase()));
    }
    sql.push_str(" ORDER BY rank, m.date DESC LIMIT ?");

    let mut stmt = conn.prepare(&sql)?;
    let mut bound: Vec<&dyn rusqlite::ToSql> =
        args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let limit_i64 = limit as i64;
    bound.push(&limit_i64);

    let rows = stmt
        .query_map(rusqlite::params_from_iter(bound.iter()), |r| {
            let raw_snip: String = r.get(4)?;
            let snip = raw_snip.replace([SNIP_OPEN, SNIP_CLOSE], "");
            Ok(SearchResult {
                path: PathBuf::from(r.get::<_, String>(0)?),
                title: r.get(1)?,
                date: r.get(2)?,
                content_type: r.get(3)?,
                snippet: snip,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Cheap stable hash of body content. Used to detect content changes when
/// mtime rolls back or filesystem precision is coarse.
fn hash_body(body: &str) -> String {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(body.as_bytes());
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn temp_config() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.output_dir = dir.path().join("meetings");
        std::fs::create_dir_all(&config.output_dir).unwrap();
        (dir, config)
    }

    fn write_meeting(dir: &Path, name: &str, title: &str, body: &str) -> PathBuf {
        write_meeting_with_date(dir, name, title, "2026-04-29", body, None, &[])
    }

    fn write_meeting_with_date(
        dir: &Path,
        name: &str,
        title: &str,
        date: &str,
        body: &str,
        recorded_by: Option<&str>,
        attendees: &[&str],
    ) -> PathBuf {
        let path = dir.join(format!("{}.md", name));
        let attendees_line = if attendees.is_empty() {
            String::new()
        } else {
            format!("attendees: {}\n", attendees.join(", "))
        };
        let recorded_by_line = recorded_by
            .map(|r| format!("recorded_by: {}\n", r))
            .unwrap_or_default();
        let content = format!(
            "---\ntitle: {}\ndate: {}\ntype: meeting\n{}{}---\n\n{}",
            title, date, attendees_line, recorded_by_line, body
        );
        std::fs::write(&path, content).unwrap();
        path
    }

    fn make_index(_dir: &tempfile::TempDir, config: &Config) -> SearchIndex {
        // Override default db path to a tempdir to keep tests isolated.
        let db_path = config.output_dir.parent().unwrap().join("search.db");
        let mut conn = schema::open_db(&db_path).unwrap();
        schema::ensure_schema(&mut conn).unwrap();
        if !schema::is_healthy(&conn).unwrap() {
            schema::rebuild(&mut conn).unwrap();
        }
        let output_dir_str = config.output_dir.to_string_lossy().into_owned();
        schema::write_output_dir(&conn, &output_dir_str).unwrap();
        SearchIndex {
            conn: Mutex::new(conn),
            db_path,
        }
    }

    #[test]
    fn sync_indexes_existing_meetings() {
        let (dir, config) = temp_config();
        write_meeting(
            &config.output_dir,
            "2026-04-01-alpha",
            "Alpha",
            "talked about pricing tiers",
        );
        write_meeting(
            &config.output_dir,
            "2026-04-02-beta",
            "Beta",
            "weekly review of metrics",
        );
        let idx = make_index(&dir, &config);
        let stats = idx.sync(&config, SyncMode::Auto).unwrap();
        assert_eq!(stats.indexed, 2);
        assert_eq!(stats.errored, 0);
    }

    #[test]
    fn sync_skips_already_indexed_via_mtime() {
        let (dir, config) = temp_config();
        write_meeting(&config.output_dir, "a", "Alpha", "body");
        let idx = make_index(&dir, &config);
        let s1 = idx.sync(&config, SyncMode::Auto).unwrap();
        assert_eq!(s1.indexed, 1);
        let s2 = idx.sync(&config, SyncMode::Auto).unwrap();
        assert_eq!(s2.indexed, 0);
        assert_eq!(s2.updated, 0);
    }

    #[test]
    fn sync_removes_deleted_files() {
        let (dir, config) = temp_config();
        let p = write_meeting(&config.output_dir, "a", "Alpha", "body");
        let idx = make_index(&dir, &config);
        idx.sync(&config, SyncMode::Auto).unwrap();
        std::fs::remove_file(&p).unwrap();
        let stats = idx.sync(&config, SyncMode::Auto).unwrap();
        assert_eq!(stats.removed, 1);
    }

    #[test]
    fn sync_excludes_archive_dir() {
        let (dir, config) = temp_config();
        let archive = config.output_dir.join("archive");
        std::fs::create_dir_all(&archive).unwrap();
        write_meeting(&archive, "old", "Old", "should not appear");
        write_meeting(&config.output_dir, "new", "New", "active");
        let idx = make_index(&dir, &config);
        idx.sync(&config, SyncMode::Auto).unwrap();
        let results = idx.search("", &SearchFilters::default(), None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "New");
    }

    #[test]
    fn empty_query_returns_all_ordered_by_date() {
        let (dir, config) = temp_config();
        write_meeting_with_date(&config.output_dir, "a", "Old", "2026-01-01", "x", None, &[]);
        write_meeting_with_date(&config.output_dir, "b", "Mid", "2026-02-01", "x", None, &[]);
        write_meeting_with_date(&config.output_dir, "c", "New", "2026-03-01", "x", None, &[]);
        let idx = make_index(&dir, &config);
        idx.sync(&config, SyncMode::Auto).unwrap();
        let results = idx.search("", &SearchFilters::default(), None).unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].title, "New");
        assert_eq!(results[2].title, "Old");
    }

    #[test]
    fn match_query_finds_body() {
        let (dir, config) = temp_config();
        write_meeting(&config.output_dir, "a", "Alpha", "we talked about pricing");
        write_meeting(&config.output_dir, "b", "Beta", "weekly review of metrics");
        let idx = make_index(&dir, &config);
        idx.sync(&config, SyncMode::Auto).unwrap();
        let results = idx
            .search("pricing", &SearchFilters::default(), None)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Alpha");
    }

    #[test]
    fn punctuation_query_does_not_error() {
        let (dir, config) = temp_config();
        write_meeting(&config.output_dir, "a", "X1: Wealth", "body");
        let idx = make_index(&dir, &config);
        idx.sync(&config, SyncMode::Auto).unwrap();
        // "x1: wealth" would error against raw FTS5; sanitizer rewrites to "x1 wealth*"
        let results = idx
            .search("x1: wealth", &SearchFilters::default(), None)
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn all_punctuation_query_returns_empty_not_error() {
        let (dir, config) = temp_config();
        write_meeting(&config.output_dir, "a", "Test", "body");
        let idx = make_index(&dir, &config);
        idx.sync(&config, SyncMode::Auto).unwrap();
        let results = idx.search("()", &SearchFilters::default(), None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn filter_by_content_type() {
        let (dir, config) = temp_config();
        let p1 = write_meeting(&config.output_dir, "m", "Meet", "body");
        std::fs::write(
            &p1,
            "---\ntitle: Meet\ndate: 2026-04-01\ntype: meeting\n---\n\nbody",
        )
        .unwrap();
        let p2 = write_meeting(&config.output_dir, "memo", "Memo", "body");
        std::fs::write(
            &p2,
            "---\ntitle: Memo\ndate: 2026-04-02\ntype: memo\n---\n\nbody",
        )
        .unwrap();
        let idx = make_index(&dir, &config);
        idx.sync(&config, SyncMode::Auto).unwrap();
        let mut filters = SearchFilters::default();
        filters.content_type = Some("memo".into());
        let results = idx.search("", &filters, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Memo");
    }

    #[test]
    fn filter_by_attendee() {
        let (dir, config) = temp_config();
        write_meeting_with_date(
            &config.output_dir,
            "a",
            "With Mat",
            "2026-04-01",
            "body",
            None,
            &["Mat", "Cathryn"],
        );
        write_meeting_with_date(
            &config.output_dir,
            "b",
            "With Alex",
            "2026-04-02",
            "body",
            None,
            &["Alex"],
        );
        let idx = make_index(&dir, &config);
        idx.sync(&config, SyncMode::Auto).unwrap();
        let mut filters = SearchFilters::default();
        filters.attendee = Some("mat".into()); // case-insensitive substring
        let results = idx.search("", &filters, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "With Mat");
    }

    #[test]
    fn upsert_replaces_existing_row() {
        let (dir, config) = temp_config();
        let p = write_meeting(&config.output_dir, "a", "Alpha", "old body");
        let idx = make_index(&dir, &config);
        idx.upsert_file(&p).unwrap();
        std::fs::write(
            &p,
            "---\ntitle: Alpha\ndate: 2026-04-29\ntype: meeting\n---\n\nbrand new body",
        )
        .unwrap();
        idx.upsert_file(&p).unwrap();

        // Old body no longer searchable
        let r1 = idx.search("old", &SearchFilters::default(), None).unwrap();
        assert!(r1.is_empty());
        // New body searchable
        let r2 = idx
            .search("brand", &SearchFilters::default(), None)
            .unwrap();
        assert_eq!(r2.len(), 1);
    }

    #[test]
    fn delete_removes_from_search() {
        let (dir, config) = temp_config();
        let p = write_meeting(&config.output_dir, "a", "Alpha", "find me");
        let idx = make_index(&dir, &config);
        idx.upsert_file(&p).unwrap();
        let r1 = idx.search("find", &SearchFilters::default(), None).unwrap();
        assert_eq!(r1.len(), 1);
        idx.delete_file(&p).unwrap();
        let r2 = idx.search("find", &SearchFilters::default(), None).unwrap();
        assert!(r2.is_empty());
    }

    #[test]
    fn snippet_strips_control_char_sentinels() {
        let (dir, config) = temp_config();
        write_meeting(
            &config.output_dir,
            "a",
            "Alpha",
            "the user mentioned pricing in the third paragraph",
        );
        let idx = make_index(&dir, &config);
        idx.sync(&config, SyncMode::Auto).unwrap();
        let results = idx
            .search("pricing", &SearchFilters::default(), None)
            .unwrap();
        assert_eq!(results.len(), 1);
        let snip = &results[0].snippet;
        assert!(!snip.contains('\u{2}'));
        assert!(!snip.contains('\u{3}'));
        assert!(snip.contains("pricing"));
    }
}

