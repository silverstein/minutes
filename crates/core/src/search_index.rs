//! SQLite (B-tree + FTS5) backed search index.
//!
//! Replaces the legacy walk-and-grep search implementation with an index that:
//!
//! - Keeps the SQLite lookup itself fast after a projection is populated; the
//!   public search facade still rebuilds and re-authorizes the live corpus on
//!   every call.
//! - Builds a process-private projection from stable live-source snapshots;
//!   meeting text never enters a durable SQLite cache.
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
use crate::markdown::{
    extract_field, read_stable_active_markdown, split_frontmatter, ActiveCorpusReadBudget,
    Frontmatter, Sensitivity, StableActiveCorpusRevision, StableMarkdownSnapshot,
};
use crate::search::{SearchFilters, SearchResult};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use exclusions::is_excluded_path;
use retry::with_retry_on_busy;
use sanitize::sanitize_fts_query;

fn open_private_memory_connection() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(
        "PRAGMA page_size=4096;
         PRAGMA max_page_count=32768;
         PRAGMA cache_size=-32768;
         PRAGMA temp_store=MEMORY;
         PRAGMA temp.page_size=4096;
         PRAGMA temp.max_page_count=16384;
         PRAGMA temp.cache_size=-16384;",
    )?;
    let mode: i64 = conn.query_row("PRAGMA temp_store", [], |row| row.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let max_pages: i64 = conn.query_row("PRAGMA max_page_count", [], |row| row.get(0))?;
    let temp_max_pages: i64 = conn.query_row("PRAGMA temp.max_page_count", [], |row| row.get(0))?;
    if mode != 2 || page_size != 4096 || max_pages > 32768 || temp_max_pages > 16384 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(conn)
}

/// Re-evaluate an indexed candidate against one live document using the exact
/// FTS5 tokenizer and sanitized-query semantics used by the private index.
///
/// This deliberately creates a second private in-memory projection. The
/// first-stage index is only a ranking/candidate source; its text is never an
/// authorization source for returned content.
pub(crate) fn live_fts_match_snippet(title: &str, body: &str, query: &str) -> Option<String> {
    let sanitized = sanitize_fts_query(query);
    if sanitized.is_empty() {
        return None;
    }

    let conn = open_private_memory_connection().ok()?;
    conn.execute_batch(
        "CREATE VIRTUAL TABLE live_doc USING fts5(
            title,
            body,
            tokenize='porter unicode61 remove_diacritics 2',
            prefix='2 3 4'
        );",
    )
    .ok()?;
    conn.execute(
        "INSERT INTO live_doc(title, body) VALUES (?1, ?2)",
        params![title, body],
    )
    .ok()?;

    const SNIP_OPEN: char = '\u{2}';
    const SNIP_CLOSE: char = '\u{3}';
    conn.query_row(
        "SELECT snippet(live_doc, 1, char(2), char(3), '…', 24)
         FROM live_doc WHERE live_doc MATCH ?1",
        params![sanitized],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .ok()?
    .map(|snippet| snippet.replace([SNIP_OPEN, SNIP_CLOSE], ""))
}

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
    /// Complete stable-source scan into the fresh private projection. Default.
    /// Catches in-place edits while Tauri (and its watcher) is closed.
    #[default]
    Auto,
    /// Full re-walk + reindex. Catches mtime-collision edge cases.
    Force,
    /// Avoid a forced schema rebuild. Because the projection is process-private,
    /// this still performs a complete stable source scan before querying.
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
    /// Cross-process heap admission. Keep the retained lock alive until after
    /// the SQLite connection is dropped.
    _projection_lease: Option<crate::policy_fs::BoundRecoveryLeaseFile>,
    /// Canonical corpus root captured when the index is opened. Incremental
    /// Every source admitted to the private projection is beneath this root.
    corpus_root: PathBuf,
}

struct IndexableDocument {
    path: PathBuf,
    title: String,
    date: String,
    content_type: String,
    attendees: Vec<String>,
    recorded_by: String,
    body: String,
    source_sha256: [u8; 32],
    mtime_ns: i64,
    size_bytes: i64,
}

/// Resolve the configured corpus once, including symlinks in existing parent
/// directories. For a not-yet-created output directory, bind the canonical
/// nearest existing ancestor plus the remaining lexical components.
fn canonical_corpus_root(path: &Path) -> Result<PathBuf, SearchIndexError> {
    let absolute = if path.is_absolute() {
        normalize_absolute_path(path)?
    } else {
        let cwd = std::env::current_dir()
            .map_err(|e| SearchIndexError::Io(format!("resolve current directory: {e}")))?;
        normalize_absolute_path(&cwd.join(path))?
    };

    if absolute.exists() {
        let canonical = std::fs::canonicalize(&absolute).map_err(|e| {
            SearchIndexError::Io(format!(
                "resolve meeting corpus {}: {}",
                absolute.display(),
                e
            ))
        })?;
        if !canonical.is_dir() {
            return Err(SearchIndexError::Io(format!(
                "meeting corpus is not a directory: {}",
                absolute.display()
            )));
        }
        return Ok(canonical);
    }

    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let name = ancestor.file_name().ok_or_else(|| {
            SearchIndexError::Io(format!(
                "meeting corpus has no existing ancestor: {}",
                absolute.display()
            ))
        })?;
        suffix.push(name.to_os_string());
        ancestor = ancestor.parent().ok_or_else(|| {
            SearchIndexError::Io(format!(
                "meeting corpus has no existing ancestor: {}",
                absolute.display()
            ))
        })?;
    }
    let mut canonical = std::fs::canonicalize(ancestor).map_err(|e| {
        SearchIndexError::Io(format!(
            "resolve meeting corpus ancestor {}: {}",
            ancestor.display(),
            e
        ))
    })?;
    for component in suffix.into_iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, SearchIndexError> {
    use std::path::Component;

    if !path.is_absolute() {
        return Err(SearchIndexError::Io(format!(
            "expected absolute meeting path: {}",
            path.display()
        )));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(SearchIndexError::Io(format!(
                        "meeting path escapes filesystem root: {}",
                        path.display()
                    )));
                }
            }
            Component::Normal(name) => normalized.push(name),
        }
    }
    Ok(normalized)
}

/// Open `path` relative to an already-canonical corpus root without following
/// symlinks in any path component. The Unix implementation walks with
/// `openat(O_NOFOLLOW)` so a concurrent symlink swap cannot redirect the read.
#[cfg(unix)]
#[allow(dead_code)]
fn open_file_beneath(root: &Path, path: &Path) -> Result<File, SearchIndexError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let relative = path.strip_prefix(root).map_err(|_| {
        SearchIndexError::Io(format!(
            "meeting path is outside corpus: {}",
            path.display()
        ))
    })?;
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(SearchIndexError::Io(format!(
            "meeting path is not a regular corpus path: {}",
            path.display()
        )));
    }

    let root_c = CString::new(root.as_os_str().as_bytes()).map_err(|_| {
        SearchIndexError::Io(format!(
            "meeting corpus contains a NUL byte: {}",
            root.display()
        ))
    })?;
    // SAFETY: root_c is NUL-terminated and the returned descriptor is checked
    // before File takes ownership.
    let root_fd = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err(SearchIndexError::Io(format!(
            "open meeting corpus {}: {}",
            root.display(),
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: root_fd is a new owned descriptor from libc::open.
    let mut directory = unsafe { File::from_raw_fd(root_fd) };

    for (index, component) in components.iter().enumerate() {
        let std::path::Component::Normal(name) = component else {
            unreachable!("components validated above")
        };
        let name_c = CString::new(name.as_bytes()).map_err(|_| {
            SearchIndexError::Io(format!(
                "meeting path contains a NUL byte: {}",
                path.display()
            ))
        })?;
        let is_last = index + 1 == components.len();
        let flags = if is_last {
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC
        } else {
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
        };
        // SAFETY: directory is an open directory descriptor, name_c is
        // NUL-terminated, and the returned descriptor is checked.
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name_c.as_ptr(), flags) };
        if fd < 0 {
            return Err(SearchIndexError::Io(format!(
                "open meeting file {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: fd is a new owned descriptor returned by libc::openat.
        let opened = unsafe { File::from_raw_fd(fd) };
        if is_last {
            return Ok(opened);
        }
        directory = opened;
    }
    unreachable!("non-empty component list returns from the loop")
}

#[cfg(not(unix))]
#[allow(dead_code)]
fn open_file_beneath(root: &Path, path: &Path) -> Result<File, SearchIndexError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        SearchIndexError::Io(format!(
            "meeting path is outside corpus: {}",
            path.display()
        ))
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(SearchIndexError::Io(format!(
                "meeting path is not a regular corpus path: {}",
                path.display()
            )));
        };
        current.push(name);
        let metadata = std::fs::symlink_metadata(&current).map_err(|e| {
            SearchIndexError::Io(format!("inspect meeting path {}: {}", current.display(), e))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(SearchIndexError::Io(format!(
                "symlinked meeting paths are not indexed: {}",
                current.display()
            )));
        }
    }
    let canonical = std::fs::canonicalize(path).map_err(|e| {
        SearchIndexError::Io(format!("resolve meeting file {}: {}", path.display(), e))
    })?;
    if !canonical.starts_with(root) || canonical != path {
        return Err(SearchIndexError::Io(format!(
            "meeting path escaped the configured corpus: {}",
            path.display()
        )));
    }
    File::open(path)
        .map_err(|e| SearchIndexError::Io(format!("open meeting file {}: {}", path.display(), e)))
}

impl SearchIndex {
    /// Open a process-private search projection.
    ///
    /// Meeting title/body bytes are intentionally never persisted to a cache
    /// file. This removes the cross-filesystem reclassification window where
    /// a meeting could become restricted after a durable FTS write and leave
    /// recoverable bytes in SQLite pages or WAL/SHM sidecars. The live
    /// authorizing facade still revalidates every returned candidate.
    pub fn open(config: &Config) -> Result<Self, SearchIndexError> {
        crate::policy_fs::retire_legacy_policy_caches().map_err(|error| {
            SearchIndexError::Io(format!(
                "retire legacy durable policy caches before search: {error}"
            ))
        })?;
        let corpus_root = canonical_corpus_root(&config.output_dir)?;
        let projection_lease =
            crate::policy_fs::acquire_private_corpus_projection_lease(&corpus_root, cfg!(test))
                .map_err(|error| {
                    SearchIndexError::Io(format!(
                        "private search projection capacity unavailable: {error}"
                    ))
                })?;
        let mut conn = open_private_memory_connection()?;
        schema::ensure_schema(&mut conn)?;

        Ok(SearchIndex {
            conn: Mutex::new(conn),
            _projection_lease: Some(projection_lease),
            corpus_root,
        })
    }

    /// Sync filesystem state into the index per the requested mode.
    #[cfg(test)]
    pub fn sync(&self, config: &Config, mode: SyncMode) -> Result<SyncStats, SearchIndexError> {
        self.sync_with_active_corpus_revision(config, mode, None)
    }

    pub(crate) fn sync_for_active_corpus(
        &self,
        config: &Config,
        mode: SyncMode,
        revision: &StableActiveCorpusRevision,
    ) -> Result<SyncStats, SearchIndexError> {
        self.sync_with_active_corpus_revision(config, mode, Some(revision))
    }

    fn sync_with_active_corpus_revision(
        &self,
        config: &Config,
        mode: SyncMode,
        revision: Option<&StableActiveCorpusRevision>,
    ) -> Result<SyncStats, SearchIndexError> {
        let start = std::time::Instant::now();
        let mut stats = SyncStats::default();
        let budget = revision.map(StableActiveCorpusRevision::budget);
        let check_deadline = || -> Result<(), SearchIndexError> {
            if budget
                .as_ref()
                .is_some_and(|budget| budget.check_deadline().is_err())
            {
                Err(SearchIndexError::Io(
                    "meeting corpus authorization deadline elapsed".into(),
                ))
            } else {
                Ok(())
            }
        };
        check_deadline()?;

        let dir = canonical_corpus_root(&config.output_dir)?;
        if dir != self.corpus_root {
            return Err(SearchIndexError::Io(format!(
                "configured meeting corpus no longer matches the index binding: {}",
                config.output_dir.display()
            )));
        }

        // A SearchIndex is a fresh process-private projection, so Skip cannot
        // mean "query an old durable cache" anymore. Preserve the public
        // no-sync contract by populating this authorized ephemeral projection
        // once without a destructive rebuild; otherwise every Skip query
        // would incorrectly return an empty corpus.

        if mode == SyncMode::Force {
            let mut conn = self.conn.lock().unwrap();
            schema::rebuild(&mut conn)?;
            // Fall through to Auto-style scan to repopulate.
        }

        // Walk + per-file diff
        let mut seen_paths: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let consume = |files, directories, bytes| {
            if budget
                .as_ref()
                .is_some_and(|budget| budget.consume(files, directories, bytes).is_err())
            {
                Err(SearchIndexError::Io(
                    "meeting corpus resource budget exceeded".into(),
                ))
            } else {
                Ok(())
            }
        };
        let consume_path = |path: &Path| {
            if budget
                .as_ref()
                .is_some_and(|budget| budget.consume_path(path).is_err())
            {
                Err(SearchIndexError::Io(
                    "meeting corpus retained-path budget exceeded".into(),
                ))
            } else {
                Ok(())
            }
        };

        let entries: Box<dyn Iterator<Item = Result<(PathBuf, bool), SearchIndexError>> + '_> =
            if let Some(revision) = revision {
                Box::new(revision.paths().map(|path| Ok((path.to_path_buf(), false))))
            } else {
                Box::new(
                    walkdir::WalkDir::new(&self.corpus_root)
                        .follow_links(false)
                        .into_iter()
                        .filter_entry(|entry| {
                            !entry.file_type().is_dir()
                                || !is_excluded_path(entry.path(), &self.corpus_root)
                        })
                        .map(|entry| {
                            entry
                                .map(|entry| {
                                    (entry.path().to_path_buf(), entry.file_type().is_dir())
                                })
                                .map_err(|_| {
                                    SearchIndexError::Io(
                                        "meeting corpus traversal could not be verified".into(),
                                    )
                                })
                        }),
                )
            };

        for entry in entries {
            check_deadline()?;
            let (path, is_directory) = entry?;
            if revision.is_none() {
                consume_path(&path)?;
            }
            if is_directory {
                if !is_excluded_path(&path, &self.corpus_root) {
                    consume(0, 1, 0)?;
                }
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("md")
                || is_excluded_path(&path, &self.corpus_root)
            {
                continue;
            }
            if revision.is_none() {
                consume(1, 0, 0)?;
            }
            // Authorization intentionally precedes the mtime fast path. A
            // normal file can be reclassified as restricted without changing
            // the metadata values cached in an older index.
            let snapshot = match revision {
                Some(revision) => revision.read_snapshot(&path),
                None => read_stable_active_markdown(&path, &self.corpus_root),
            };
            let Some(snapshot) = snapshot else {
                if revision.is_some() {
                    return Err(SearchIndexError::Io(
                        "meeting changed while building the authorized search projection".into(),
                    ));
                }
                let _ = self.delete_file_inner(&path);
                stats.errored += 1;
                continue;
            };
            let document = match self.indexable_document_from_snapshot(snapshot) {
                Ok(Some(document)) => document,
                Ok(None) => {
                    if self.delete_file_inner(&path)? {
                        stats.removed += 1;
                    }
                    continue;
                }
                Err(_) => {
                    // Malformed and policy-uncertain source files are not
                    // allowed to leave a stale raw row behind.
                    let _ = self.delete_file_inner(&path);
                    tracing::warn!("policy-uncertain meeting excluded from search index");
                    stats.errored += 1;
                    continue;
                }
            };
            let path = document.path.clone();
            if revision.is_none() {
                consume_path(&path)?;
            }
            seen_paths.insert(path.clone());

            let needs_index = {
                let conn = self.conn.lock().unwrap();
                let row: Option<(i64, i64, String)> = conn
                    .query_row(
                        "SELECT mtime_ns, size_bytes, body_hash FROM meetings WHERE path = ?",
                        params![path.to_string_lossy()],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?;
                match row {
                    None => true,
                    Some((stored_m, stored_s, stored_hash)) => {
                        stored_m != document.mtime_ns
                            || stored_s != document.size_bytes
                            || stored_hash != hex_sha256(&document.source_sha256)
                    }
                }
            };

            if !needs_index {
                continue;
            }

            let upsert = if revision.is_some() {
                self.upsert_pre_attested_document(document)
            } else {
                self.upsert_document(document, None)
            };
            match upsert {
                Ok(true) => stats.indexed += 1,
                Ok(false) => stats.updated += 1,
                Err(_) => {
                    tracing::warn!("search-index upsert failed for an authorized meeting");
                    stats.errored += 1;
                }
            }
            check_deadline()?;
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
            check_deadline()?;
            if self.delete_file(&PathBuf::from(&p)).is_err() {
                tracing::warn!("search-index deletion failed for a stale meeting row");
                stats.errored += 1;
            } else {
                stats.removed += 1;
            }
        }

        check_deadline()?;
        stats.duration_ms = start.elapsed().as_millis() as u64;
        Ok(stats)
    }

    /// Index one file for direct race/authorization regression tests.
    /// Returns `Ok(true)` if a new row was inserted, `Ok(false)` if updated.
    #[cfg(test)]
    pub fn upsert_file(&self, path: &Path) -> Result<(), SearchIndexError> {
        match self.read_indexable_file(path) {
            Ok(Some(document)) => self.upsert_document(document, None).map(|_| ()),
            Ok(None) => self.delete_file(path),
            Err(e) => {
                let _ = self.delete_file(path);
                Err(e)
            }
        }
    }

    #[cfg(test)]
    fn read_indexable_file(
        &self,
        path: &Path,
    ) -> Result<Option<IndexableDocument>, SearchIndexError> {
        let path = self.bound_path(path)?;
        if path.extension().and_then(|s| s.to_str()) != Some("md")
            || is_excluded_path(&path, &self.corpus_root)
        {
            return Err(SearchIndexError::Io(format!(
                "path is not an indexable meeting file: {}",
                path.display()
            )));
        }

        let snapshot = read_stable_active_markdown(&path, &self.corpus_root).ok_or_else(|| {
            SearchIndexError::Io("meeting could not be read as a stable policy snapshot".into())
        })?;
        self.indexable_document_from_snapshot(snapshot)
    }

    fn indexable_document_from_snapshot(
        &self,
        snapshot: StableMarkdownSnapshot,
    ) -> Result<Option<IndexableDocument>, SearchIndexError> {
        let path = snapshot.path.clone();
        let meta = std::fs::metadata(&snapshot.path)
            .map_err(|e| SearchIndexError::Io(format!("stat {}: {}", path.display(), e)))?;
        if !meta.is_file() {
            return Err(SearchIndexError::Io(format!(
                "meeting path is not a regular file: {}",
                path.display()
            )));
        }
        let (frontmatter, body) = split_frontmatter(&snapshot.content);
        if frontmatter.is_empty() {
            return Err(SearchIndexError::Frontmatter {
                path: path.display().to_string(),
                message: "missing YAML frontmatter".into(),
            });
        }
        let parsed: Frontmatter =
            serde_yaml::from_str(frontmatter).map_err(|e| SearchIndexError::Frontmatter {
                path: path.display().to_string(),
                message: e.to_string(),
            })?;
        if parsed.title.trim().is_empty() {
            return Err(SearchIndexError::Frontmatter {
                path: path.display().to_string(),
                message: "title must not be empty".into(),
            });
        }
        if parsed.sensitivity == Some(Sensitivity::Restricted) {
            return Ok(None);
        }

        // Preserve the legacy SearchResult date representation after the
        // typed parse has established that the field exists and is valid.
        let date = extract_field(frontmatter, "date").unwrap_or_else(|| parsed.date.to_rfc3339());
        let content_type = parsed.r#type.as_str().to_string();
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let size_bytes = meta.len() as i64;

        Ok(Some(IndexableDocument {
            path: snapshot.path,
            title: parsed.title,
            date,
            content_type,
            attendees: parsed.attendees,
            recorded_by: parsed.recorded_by.unwrap_or_default(),
            body: body.to_string(),
            source_sha256: snapshot.content_sha256,
            mtime_ns,
            size_bytes,
        }))
    }

    fn upsert_document(
        &self,
        document: IndexableDocument,
        budget: Option<&ActiveCorpusReadBudget>,
    ) -> Result<bool, SearchIndexError> {
        self.upsert_document_inner(document, budget, true, |_| {})
    }

    fn upsert_pre_attested_document(
        &self,
        document: IndexableDocument,
    ) -> Result<bool, SearchIndexError> {
        self.upsert_document_inner(document, None, false, |_| {})
    }

    #[cfg(test)]
    fn upsert_document_with_hook(
        &self,
        document: IndexableDocument,
        budget: Option<&ActiveCorpusReadBudget>,
        mut after_write_before_reauthorize: impl FnMut(&Path),
    ) -> Result<bool, SearchIndexError> {
        self.upsert_document_inner(document, budget, true, |path| {
            after_write_before_reauthorize(path)
        })
    }

    fn upsert_document_inner(
        &self,
        document: IndexableDocument,
        budget: Option<&ActiveCorpusReadBudget>,
        reauthorize: bool,
        mut after_write_before_reauthorize: impl FnMut(&Path),
    ) -> Result<bool, SearchIndexError> {
        if reauthorize && !self.document_still_authorized(&document, budget) {
            return Err(SearchIndexError::Io(
                "meeting changed before search projection update".into(),
            ));
        }
        let IndexableDocument {
            path,
            title,
            date,
            content_type,
            attendees,
            recorded_by,
            body,
            source_sha256,
            mtime_ns,
            size_bytes,
        } = document;
        let attendees_json = serde_json::to_string(&attendees).unwrap_or_else(|_| "[]".into());
        let body_hash = hex_sha256(&source_sha256);
        let indexed_at = chrono::Local::now().timestamp();
        let path_str = path.to_string_lossy().into_owned();

        let mut conn = self.conn.lock().unwrap();
        let existed = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM meetings WHERE path = ?)",
                params![path_str],
                |r| r.get::<_, bool>(0),
            )
            .unwrap_or(false);
        let txn = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let write_result = (|| -> Result<(), rusqlite::Error> {
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
                params![
                    rowid,
                    crate::retrieval::search_tokens(&title).join(" "),
                    crate::retrieval::search_tokens(&body).join(" "),
                ],
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
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = txn.rollback();
            return Err(error.into());
        }

        if reauthorize {
            after_write_before_reauthorize(&path);
            let reauthorized =
                self.source_path_has_normal_policy(&path, Some(&source_sha256), budget);
            if !reauthorized {
                txn.rollback()?;
                return Err(SearchIndexError::Io(
                    "meeting changed during search projection update".into(),
                ));
            }
        }
        txn.commit()?;
        Ok(!existed)
    }

    fn document_still_authorized(
        &self,
        document: &IndexableDocument,
        budget: Option<&ActiveCorpusReadBudget>,
    ) -> bool {
        self.source_path_has_normal_policy(&document.path, Some(&document.source_sha256), budget)
    }

    fn source_path_has_normal_policy(
        &self,
        path: &Path,
        expected_sha256: Option<&[u8; 32]>,
        budget: Option<&ActiveCorpusReadBudget>,
    ) -> bool {
        let snapshot = match budget {
            Some(budget) => crate::markdown::read_stable_active_markdown_with_budget(
                path,
                &self.corpus_root,
                budget,
            ),
            None => read_stable_active_markdown(path, &self.corpus_root),
        };
        let Some(snapshot) = snapshot else {
            return false;
        };
        if budget.is_some_and(|budget| budget.consume(1, 0, snapshot.content.len() as u64).is_err())
        {
            return false;
        }
        if expected_sha256.is_some_and(|expected| snapshot.content_sha256 != *expected) {
            return false;
        }
        let (frontmatter, _) = split_frontmatter(&snapshot.content);
        !frontmatter.is_empty()
            && serde_yaml::from_str::<Frontmatter>(frontmatter)
                .is_ok_and(|parsed| parsed.sensitivity != Some(Sensitivity::Restricted))
    }

    /// Remove one file from the index. CASCADE handles meeting_attendees.
    /// Idempotent: missing rows are a no-op.
    pub fn delete_file(&self, path: &Path) -> Result<(), SearchIndexError> {
        self.delete_file_inner(path).map(|_| ())
    }

    fn delete_file_inner(&self, path: &Path) -> Result<bool, SearchIndexError> {
        let path = self.bound_path(path)?;
        let path_str = path.to_string_lossy().into_owned();
        let mut conn = self.conn.lock().unwrap();
        let mut removed = false;
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
                removed = true;
            }
            txn.commit()
        })?;
        if removed {
            // secure_delete overwrites the database cells; truncating the WAL
            // removes older frames that could still contain the raw body.
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        Ok(removed)
    }

    fn bound_path(&self, path: &Path) -> Result<PathBuf, SearchIndexError> {
        let candidate = if path.is_absolute() {
            normalize_absolute_path(path)?
        } else {
            normalize_absolute_path(&self.corpus_root.join(path))?
        };

        // macOS exposes some temporary paths lexically through `/var` while
        // canonical paths begin `/private/var`. Find the nearest candidate
        // ancestor that resolves to the already-bound corpus root, then append
        // the untouched relative suffix. This handles that stable alias (and
        // missing delete targets) without canonicalizing below-root symlinks
        // away before the shared stable reader can reject them.
        for ancestor in candidate.ancestors() {
            if std::fs::canonicalize(ancestor).ok().as_ref() != Some(&self.corpus_root) {
                continue;
            }
            let relative = candidate.strip_prefix(ancestor).map_err(|_| {
                SearchIndexError::Io(format!(
                    "meeting path is outside the configured corpus: {}",
                    path.display()
                ))
            })?;
            if relative.as_os_str().is_empty()
                || relative
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                break;
            }
            return Ok(self.corpus_root.join(relative));
        }
        Err(SearchIndexError::Io(format!(
            "meeting path is outside the configured corpus: {}",
            path.display()
        )))
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
        if !crate::retrieval::query_is_bounded(query) {
            return Err(SearchIndexError::Io(
                "search query exceeds 2048 UTF-16 units".into(),
            ));
        }
        let sanitized = sanitize_fts_query(query);
        if sanitized.is_empty() {
            // All-punctuation input. Caller treats this as "no match" rather than error.
            return Ok(Vec::new());
        }
        search_match(&conn, &sanitized, filters, limit)
    }

    /// Volatile restricted-meeting search for an explicitly authorized human
    /// override. Restricted bytes are descriptor-read from the live corpus and
    /// never written to the first-stage projection before the explicit
    /// override candidate is passed through the common final live gate.
    #[cfg(test)]
    pub fn search_restricted_live(
        &self,
        query: &str,
        filters: &SearchFilters,
    ) -> Vec<SearchResult> {
        self.search_restricted_live_with_budget(query, filters, None)
            .unwrap_or_default()
    }

    pub(crate) fn search_restricted_live_for_active_corpus(
        &self,
        query: &str,
        filters: &SearchFilters,
        revision: &StableActiveCorpusRevision,
    ) -> Result<Vec<SearchResult>, SearchIndexError> {
        let _ = query;
        if !filters.include_restricted {
            return Ok(Vec::new());
        }
        let mut results = Vec::new();
        // Iterate only the exact paths admitted by the pre-operation revision.
        // A file created after that snapshot cannot spend this pass's budget;
        // the outer post-snapshot still rejects publication when membership
        // changed.
        for path in revision.paths() {
            let snapshot = revision.read_snapshot(path).ok_or_else(|| {
                SearchIndexError::Io(
                    "meeting changed while building the authorized restricted projection".into(),
                )
            })?;
            if let Some(result) = restricted_live_result(snapshot, filters) {
                results.push(result);
            }
        }
        results.sort_by(|left, right| right.date.cmp(&left.date));
        Ok(results)
    }

    #[cfg(test)]
    fn search_restricted_live_with_budget(
        &self,
        _query: &str,
        filters: &SearchFilters,
        budget: Option<ActiveCorpusReadBudget>,
    ) -> Result<Vec<SearchResult>, SearchIndexError> {
        if !filters.include_restricted {
            return Ok(Vec::new());
        }
        let mut results = Vec::new();
        let consume = |files, directories, bytes| {
            if budget
                .as_ref()
                .is_some_and(|budget| budget.consume(files, directories, bytes).is_err())
            {
                Err(SearchIndexError::Io(
                    "meeting corpus resource budget exceeded".into(),
                ))
            } else {
                Ok(())
            }
        };
        let consume_path = |path: &Path| {
            if budget
                .as_ref()
                .is_some_and(|budget| budget.consume_path(path).is_err())
            {
                Err(SearchIndexError::Io(
                    "meeting corpus retained-path budget exceeded".into(),
                ))
            } else {
                Ok(())
            }
        };

        for entry in walkdir::WalkDir::new(&self.corpus_root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                !entry.file_type().is_dir() || !is_excluded_path(entry.path(), &self.corpus_root)
            })
        {
            if budget
                .as_ref()
                .is_some_and(|budget| budget.check_deadline().is_err())
            {
                return Err(SearchIndexError::Io(
                    "meeting corpus authorization deadline elapsed".into(),
                ));
            }
            let entry = entry.map_err(|_| {
                SearchIndexError::Io("meeting corpus traversal could not be verified".into())
            })?;
            consume_path(entry.path())?;
            if entry.file_type().is_dir() {
                if !is_excluded_path(entry.path(), &self.corpus_root) {
                    consume(0, 1, 0)?;
                }
                continue;
            }
            if !entry.file_type().is_file()
                || entry.path().extension().and_then(|ext| ext.to_str()) != Some("md")
                || is_excluded_path(entry.path(), &self.corpus_root)
            {
                continue;
            }
            consume(1, 0, 0)?;
            let snapshot = match budget.as_ref() {
                Some(budget) => crate::markdown::read_stable_active_markdown_with_budget(
                    entry.path(),
                    &self.corpus_root,
                    budget,
                ),
                None => read_stable_active_markdown(entry.path(), &self.corpus_root),
            };
            let Some(snapshot) = snapshot else {
                continue;
            };
            consume(0, 0, snapshot.content.len() as u64)?;
            if let Some(result) = restricted_live_result(snapshot, filters) {
                results.push(result);
            }
        }

        if budget
            .as_ref()
            .is_some_and(|budget| budget.check_deadline().is_err())
        {
            return Err(SearchIndexError::Io(
                "meeting corpus authorization deadline elapsed".into(),
            ));
        }
        results.sort_by(|left, right| right.date.cmp(&left.date));
        Ok(results)
    }
}

fn restricted_live_result(
    snapshot: StableMarkdownSnapshot,
    filters: &SearchFilters,
) -> Option<SearchResult> {
    let (frontmatter_text, _) = split_frontmatter(&snapshot.content);
    let frontmatter = serde_yaml::from_str::<Frontmatter>(frontmatter_text).ok()?;
    if frontmatter.sensitivity != Some(Sensitivity::Restricted) {
        return None;
    }
    let content_type = frontmatter.r#type.as_str();
    if filters
        .content_type
        .as_deref()
        .is_some_and(|expected| expected != content_type)
    {
        return None;
    }
    let date = frontmatter.date.to_rfc3339();
    if filters
        .since
        .as_deref()
        .is_some_and(|since| date.as_str() < since)
    {
        return None;
    }
    if let Some(attendee) = filters.attendee.as_deref() {
        let needle = attendee.to_lowercase();
        if !frontmatter
            .attendees
            .iter()
            .any(|value| value.to_lowercase().contains(&needle))
        {
            return None;
        }
    }
    if let Some(recorded_by) = filters.recorded_by.as_deref() {
        let needle = recorded_by.to_lowercase();
        if !frontmatter
            .recorded_by
            .as_deref()
            .is_some_and(|value| value.to_lowercase().contains(&needle))
        {
            return None;
        }
    }
    Some(SearchResult {
        path: snapshot.path,
        title: frontmatter.title,
        date,
        content_type: content_type.into(),
        snippet: String::new(),
        matched_via_alias: None,
        evidence: None,
    })
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
                matched_via_alias: None,
                evidence: None,
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
                matched_via_alias: None,
                evidence: None,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Cheap stable hash of body content. Used to detect content changes when
/// mtime rolls back or filesystem precision is coarse.
fn hex_sha256(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn private_search_connections_force_memory_only_temporary_storage() {
        let conn = open_private_memory_connection().unwrap();
        let mode: i64 = conn
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, 2);
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap();
        let max_pages: i64 = conn
            .query_row("PRAGMA max_page_count", [], |row| row.get(0))
            .unwrap();
        let temp_max_pages: i64 = conn
            .query_row("PRAGMA temp.max_page_count", [], |row| row.get(0))
            .unwrap();
        assert_eq!(page_size, 4096);
        assert!(max_pages <= 32768);
        assert!(temp_max_pages <= 16384);
    }

    fn temp_config() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            output_dir: dir.path().join("meetings"),
            ..Default::default()
        };
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
        let attendees_line = attendees
            .iter()
            .map(|attendee| format!("  - {}\n", serde_json::to_string(attendee).unwrap()))
            .collect::<String>();
        let attendees_line = if attendees_line.is_empty() {
            attendees_line
        } else {
            format!("attendees:\n{attendees_line}")
        };
        let recorded_by_line = recorded_by
            .map(|r| format!("recorded_by: {}\n", serde_json::to_string(r).unwrap()))
            .unwrap_or_default();
        let content = format!(
            "---\ntitle: {}\ndate: {}\ntype: meeting\n{}{}---\n\n{}",
            serde_json::to_string(title).unwrap(),
            date,
            attendees_line,
            recorded_by_line,
            body
        );
        std::fs::write(&path, content).unwrap();
        path
    }

    fn make_index(_dir: &tempfile::TempDir, config: &Config) -> SearchIndex {
        let mut conn = Connection::open_in_memory().unwrap();
        schema::ensure_schema(&mut conn).unwrap();
        SearchIndex {
            conn: Mutex::new(conn),
            _projection_lease: None,
            corpus_root: std::fs::canonicalize(&config.output_dir).unwrap(),
        }
    }

    #[test]
    fn live_fts_recheck_matches_private_query_semantics() {
        for (query, body) in [
            (
                "pricing roadmap",
                "The roadmap comes first. Much later we revisited pricing.",
            ),
            (
                "pricing/roadm",
                "Roadmap notes and a separate pricing section.",
            ),
            (
                "café résu",
                "The cafe discussion included detailed resumes.",
            ),
            (
                "running",
                "The team is running the migration every morning.",
            ),
        ] {
            assert!(
                live_fts_match_snippet("Quarterly plan", body, query).is_some(),
                "live FTS semantics rejected {query:?}"
            );
        }

        assert!(live_fts_match_snippet("Plan", "pricing only", "pricing roadmap").is_none());
        assert!(live_fts_match_snippet("Plan", "anything", "---").is_none());
    }

    fn raw_counts(idx: &SearchIndex, canary: &str) -> (i64, i64) {
        let conn = idx.conn.lock().unwrap();
        let meetings = conn
            .query_row("SELECT COUNT(*) FROM meetings", [], |r| r.get(0))
            .unwrap();
        let fts = conn
            .query_row(
                "SELECT COUNT(*) FROM meetings_fts WHERE meetings_fts MATCH ?",
                [canary],
                |r| r.get(0),
            )
            .unwrap();
        (meetings, fts)
    }

    #[test]
    fn production_search_accepts_an_ordinary_state_root_without_private_stores() {
        let _guard = crate::test_home_env_lock();
        let (dir, config) = temp_config();
        let home = dir.path().join("home");
        let state = home.join(".minutes");
        std::fs::create_dir_all(&state).unwrap();
        // Restore the process-wide settings even if a regression panics.
        struct RestoreEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for RestoreEnv {
            fn drop(&mut self) {
                for (name, value) in &self.0 {
                    if let Some(value) = value {
                        std::env::set_var(name, value);
                    } else {
                        std::env::remove_var(name);
                    }
                }
            }
        }
        let _restore = RestoreEnv(
            ["HOME", "MINUTES_HOME"]
                .into_iter()
                .map(|name| (name, std::env::var_os(name)))
                .collect(),
        );
        std::env::set_var("HOME", &home);
        std::env::set_var("MINUTES_HOME", &state);
        write_meeting(
            &config.output_dir,
            "fresh",
            "Fresh installation",
            "searchcanary",
        );

        let index = SearchIndex::open(&config).unwrap();
        index.sync(&config, SyncMode::Auto).unwrap();
        let matches = index
            .search("searchcanary", &SearchFilters::default(), None)
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].title, "Fresh installation");
        assert_eq!(std::fs::read_dir(&state).unwrap().count(), 0);
        #[cfg(windows)]
        assert!(crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&state).is_err());
    }

    #[test]
    fn production_index_is_process_private_and_creates_no_cache_sidecars() {
        let _guard = crate::test_home_env_lock();
        let (dir, config) = temp_config();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_minutes_home = std::env::var_os("MINUTES_HOME");
        std::env::set_var("HOME", &home);
        let state = home.join("isolated-minutes");
        std::env::set_var("MINUTES_HOME", &state);
        drop(crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&state).unwrap());
        let historical_state = home.join(".minutes");
        drop(
            crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&historical_state)
                .unwrap(),
        );
        let legacy = state.join("search.db");
        let historical_legacy = historical_state.join("search.db-wal");
        std::fs::write(&legacy, b"PRIVATE-LEGACY-SEARCH-CANARY").unwrap();
        std::fs::write(&historical_legacy, b"PRIVATE-HISTORICAL-SEARCH-CANARY").unwrap();
        let legacy_holder = File::open(&legacy).unwrap();
        let historical_holder = File::open(&historical_legacy).unwrap();

        let index = SearchIndex::open(&config).unwrap();
        index.sync(&config, SyncMode::Auto).unwrap();
        drop(index);
        assert_eq!(legacy_holder.metadata().unwrap().len(), 0);
        assert_eq!(historical_holder.metadata().unwrap().len(), 0);

        for relative in [
            ".minutes/search.db",
            ".minutes/search.db-wal",
            ".minutes/search.db-shm",
            "isolated-minutes/search.db",
            "isolated-minutes/search.db-wal",
            "isolated-minutes/search.db-shm",
        ] {
            assert!(
                !home.join(relative).exists(),
                "created durable cache {relative}"
            );
        }

        if let Some(value) = old_home {
            std::env::set_var("HOME", value);
        } else {
            std::env::remove_var("HOME");
        }
        if let Some(value) = old_minutes_home {
            std::env::set_var("MINUTES_HOME", value);
        } else {
            std::env::remove_var("MINUTES_HOME");
        }
    }

    #[test]
    fn read_to_upsert_reclassification_is_rejected_without_retained_bytes() {
        let (dir, config) = temp_config();
        let path = write_meeting(
            &config.output_dir,
            "reclassification",
            "Normal title",
            "NORMAL-ONLY-CANARY",
        );
        let index = make_index(&dir, &config);
        let document = index.read_indexable_file(&path).unwrap().unwrap();
        std::fs::write(
            &path,
            "---\ntitle: Now restricted\ndate: 2026-07-15\ntype: meeting\nsensitivity: restricted\n---\n\nRESTRICTED-RETENTION-CANARY",
        )
        .unwrap();

        assert!(index.upsert_document(document, None).is_err());
        assert_eq!(raw_counts(&index, "NORMAL"), (0, 0));
        assert_eq!(raw_counts(&index, "RESTRICTED"), (0, 0));
    }

    #[test]
    fn reclassification_after_sql_write_rolls_back_private_projection() {
        let (dir, config) = temp_config();
        let path = write_meeting(
            &config.output_dir,
            "transaction-race",
            "Normal title",
            "PRETRANSACTION-CANARY",
        );
        let index = make_index(&dir, &config);
        let document = index.read_indexable_file(&path).unwrap().unwrap();

        let result = index.upsert_document_with_hook(document, None, |source_path| {
            std::fs::write(
                source_path,
                "---\ntitle: Now restricted\ndate: 2026-07-15\ntype: meeting\nsensitivity: restricted\n---\n\nPOSTWRITE-RESTRICTED-CANARY",
            )
            .unwrap();
        });

        assert!(result.is_err());
        assert_eq!(raw_counts(&index, "PRETRANSACTION"), (0, 0));
        assert_eq!(raw_counts(&index, "POSTWRITE"), (0, 0));
    }

    #[test]
    fn reauthorization_reads_share_the_sync_resource_budget() {
        let (dir, config) = temp_config();
        let path = write_meeting(
            &config.output_dir,
            "bounded-reauthorization",
            "Normal title",
            "BOUNDED-REAUTHORIZATION-CANARY",
        );
        let index = make_index(&dir, &config);
        let document = index.read_indexable_file(&path).unwrap().unwrap();
        let budget =
            ActiveCorpusReadBudget::for_test(1, 1, 1024 * 1024, std::time::Duration::from_secs(1));

        assert!(index.upsert_document(document, Some(&budget)).is_err());
        assert_eq!(raw_counts(&index, "BOUNDED"), (0, 0));
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
    fn restricted_meeting_is_never_persisted_in_btree_or_fts() {
        let (dir, config) = temp_config();
        std::fs::write(
            config.output_dir.join("restricted.md"),
            "---\ntitle: Restricted\ndate: 2026-07-15\ntype: meeting\nsensitivity: restricted\n---\n\nRESTRICTEDINGESTCANARY",
        )
        .unwrap();
        let idx = make_index(&dir, &config);

        let stats = idx.sync(&config, SyncMode::Auto).unwrap();

        assert_eq!(stats.indexed, 0);
        let authorized = idx.search_restricted_live(
            "RESTRICTEDINGESTCANARY",
            &SearchFilters {
                include_restricted: true,
                ..Default::default()
            },
        );
        assert_eq!(authorized.len(), 1);
        assert_eq!(authorized[0].title, "Restricted");
        assert_eq!(raw_counts(&idx, "RESTRICTEDINGESTCANARY"), (0, 0));
    }

    #[test]
    fn restricted_projection_iterates_only_the_prebound_revision() {
        let (dir, config) = temp_config();
        std::fs::write(
            config.output_dir.join("bound.md"),
            "---\ntitle: Bound Restricted\ndate: 2026-07-15\ntype: meeting\nsensitivity: restricted\n---\n\nBOUND",
        )
        .unwrap();
        let revision = crate::markdown::stable_active_corpus_revision(&config.output_dir).unwrap();
        let materialization_budget = revision.budget().fresh_pass();
        let revision = revision.with_read_budget(materialization_budget);
        std::fs::write(
            config.output_dir.join("late.md"),
            "---\ntitle: Late Restricted\ndate: 2026-07-16\ntype: meeting\nsensitivity: restricted\n---\n\nLATE",
        )
        .unwrap();
        let idx = make_index(&dir, &config);

        let results = idx
            .search_restricted_live_for_active_corpus(
                "",
                &SearchFilters {
                    include_restricted: true,
                    ..Default::default()
                },
                &revision,
            )
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Bound Restricted");
    }

    #[test]
    fn malformed_and_policy_uncertain_frontmatter_never_reaches_raw_tables() {
        let cases = [
            ("missing-title", "date: 2026-07-15\ntype: meeting"),
            ("missing-date", "title: Missing date\ntype: meeting"),
            ("missing-type", "title: Missing type\ndate: 2026-07-15"),
            (
                "null-sensitivity",
                "title: Null\ndate: 2026-07-15\ntype: meeting\nsensitivity: null",
            ),
            (
                "tilde-sensitivity",
                "title: Tilde\ndate: 2026-07-15\ntype: meeting\nsensitivity: ~",
            ),
            (
                "empty-sensitivity",
                "title: Empty\ndate: 2026-07-15\ntype: meeting\nsensitivity:",
            ),
            (
                "unknown-sensitivity",
                "title: Unknown\ndate: 2026-07-15\ntype: meeting\nsensitivity: confidential",
            ),
        ];

        for (name, frontmatter) in cases {
            let (dir, config) = temp_config();
            let canary = format!("POLICYUNCERTAIN{name}").replace('-', "");
            std::fs::write(
                config.output_dir.join(format!("{name}.md")),
                format!("---\n{frontmatter}\n---\n\n{canary}"),
            )
            .unwrap();
            let idx = make_index(&dir, &config);

            idx.sync(&config, SyncMode::Auto).unwrap();

            assert_eq!(raw_counts(&idx, &canary), (0, 0), "case: {name}");
        }
    }

    #[test]
    fn normal_to_restricted_purges_raw_rows_before_mtime_fast_path() {
        let (dir, config) = temp_config();
        let path = write_meeting(
            &config.output_dir,
            "reclassified",
            "Initially normal",
            "RECLASSIFICATIONCANARY",
        );
        let idx = make_index(&dir, &config);
        idx.sync(&config, SyncMode::Auto).unwrap();
        assert_eq!(raw_counts(&idx, "RECLASSIFICATIONCANARY"), (1, 1));

        std::fs::write(
            &path,
            "---\ntitle: Now restricted\ndate: 2026-07-15\ntype: meeting\nsensitivity: restricted\n---\n\nRECLASSIFICATIONCANARY",
        )
        .unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let mtime_ns = metadata
            .modified()
            .unwrap()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        // Simulate a stale/colliding cache record: the old implementation
        // skipped content inspection when these two values matched.
        idx.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE meetings SET mtime_ns = ?, size_bytes = ? WHERE path = ?",
                params![mtime_ns, metadata.len() as i64, path.to_string_lossy()],
            )
            .unwrap();

        let stats = idx.sync(&config, SyncMode::Auto).unwrap();

        assert_eq!(stats.removed, 1);
        assert_eq!(raw_counts(&idx, "RECLASSIFICATIONCANARY"), (0, 0));
    }

    #[test]
    #[cfg(unix)]
    fn symlink_escape_is_not_followed_by_sync_or_direct_upsert() {
        use std::os::unix::fs::symlink;

        let (dir, config) = temp_config();
        let outside = dir.path().join("outside.md");
        std::fs::write(
            &outside,
            "---\ntitle: Outside\ndate: 2026-07-15\ntype: meeting\n---\n\nSYMLINKESCAPECANARY",
        )
        .unwrap();
        let link = config.output_dir.join("escape.md");
        symlink(&outside, &link).unwrap();
        let idx = make_index(&dir, &config);

        idx.sync(&config, SyncMode::Auto).unwrap();
        assert!(idx.upsert_file(&link).is_err());

        assert_eq!(raw_counts(&idx, "SYMLINKESCAPECANARY"), (0, 0));
    }

    #[test]
    fn direct_upsert_outside_bound_corpus_is_rejected() {
        let (dir, config) = temp_config();
        let outside = dir.path().join("outside.md");
        std::fs::write(
            &outside,
            "---\ntitle: Outside\ndate: 2026-07-15\ntype: meeting\n---\n\nOUTSIDECORPUSCANARY",
        )
        .unwrap();
        let idx = make_index(&dir, &config);

        assert!(idx.upsert_file(&outside).is_err());

        assert_eq!(raw_counts(&idx, "OUTSIDECORPUSCANARY"), (0, 0));
    }

    #[test]
    #[cfg(unix)]
    fn configured_corpus_symlink_retarget_cannot_change_bound_root() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let corpus_a = dir.path().join("corpus-a");
        let corpus_b = dir.path().join("corpus-b");
        std::fs::create_dir_all(&corpus_a).unwrap();
        std::fs::create_dir_all(&corpus_b).unwrap();
        let configured = dir.path().join("configured-meetings");
        symlink(&corpus_a, &configured).unwrap();
        let config = Config {
            output_dir: configured.clone(),
            ..Default::default()
        };
        let idx = make_index(&dir, &config);

        std::fs::remove_file(&configured).unwrap();
        symlink(&corpus_b, &configured).unwrap();
        write_meeting(
            &corpus_b,
            "outside-binding",
            "Retargeted",
            "RETARGETEDCORPUSCANARY",
        );

        assert!(idx.sync(&config, SyncMode::Auto).is_err());
        assert_eq!(raw_counts(&idx, "RETARGETEDCORPUSCANARY"), (0, 0));
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
        let filters = SearchFilters {
            content_type: Some("memo".into()),
            ..Default::default()
        };
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
        let filters = SearchFilters {
            attendee: Some("mat".into()), // case-insensitive substring
            ..Default::default()
        };
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
