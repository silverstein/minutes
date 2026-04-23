use crate::config::Config;
use crate::markdown::ContentType;
use crate::pid::CaptureMode;
use chrono::{DateTime, Local};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

// ──────────────────────────────────────────────────────────────
// Desktop-context sidecar store.
//
// Meetings and memos remain durable markdown artifacts under ~/meetings.
// context.db stores local, query-oriented state that links adjacent desktop
// context to those artifacts by session and timestamp.
//
//   recording/live session ──▶ context_sessions ──▶ context_events
//                                  │
//                                  └──▶ context_links (job audio, markdown, JSONL)
//
// If context.db is deleted, meeting markdown still works. Only the adjacent
// desktop-context index is lost.
// ──────────────────────────────────────────────────────────────

const SCHEMA_VERSION: i64 = 1;
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum ContextStoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid timestamp '{value}': {source}")]
    InvalidTimestamp {
        value: String,
        source: chrono::ParseError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextSessionType {
    Recording,
    LiveTranscript,
    MemoWindow,
    FocusSession,
}

impl ContextSessionType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::LiveTranscript => "live-transcript",
            Self::MemoWindow => "memo-window",
            Self::FocusSession => "focus-session",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "recording" => Self::Recording,
            "live-transcript" => Self::LiveTranscript,
            "memo-window" => Self::MemoWindow,
            "focus-session" => Self::FocusSession,
            _ => Self::Recording,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextSessionState {
    Active,
    Processing,
    Complete,
    Failed,
    Discarded,
}

impl ContextSessionState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Processing => "processing",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Discarded => "discarded",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "processing" => Self::Processing,
            "complete" => Self::Complete,
            "failed" => Self::Failed,
            "discarded" => Self::Discarded,
            _ => Self::Active,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextEventSource {
    AppFocus,
    WindowFocus,
    BrowserPage,
    Clipboard,
    ScreenshotRef,
    Accessibility,
}

impl ContextEventSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::AppFocus => "app-focus",
            Self::WindowFocus => "window-focus",
            Self::BrowserPage => "browser-page",
            Self::Clipboard => "clipboard",
            Self::ScreenshotRef => "screenshot-ref",
            Self::Accessibility => "accessibility",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "window-focus" => Self::WindowFocus,
            "browser-page" => Self::BrowserPage,
            "clipboard" => Self::Clipboard,
            "screenshot-ref" => Self::ScreenshotRef,
            "accessibility" => Self::Accessibility,
            _ => Self::AppFocus,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextPrivacyScope {
    Normal,
    Filtered,
    Redacted,
}

impl ContextPrivacyScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Filtered => "filtered",
            Self::Redacted => "redacted",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "filtered" => Self::Filtered,
            "redacted" => Self::Redacted,
            _ => Self::Normal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextLinkKind {
    Job,
    AudioCapture,
    MarkdownArtifact,
    LiveTranscriptJsonl,
    LiveTranscriptWav,
    ScreenshotDirectory,
    PreservedCapture,
}

impl ContextLinkKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Job => "job",
            Self::AudioCapture => "audio-capture",
            Self::MarkdownArtifact => "markdown-artifact",
            Self::LiveTranscriptJsonl => "live-transcript-jsonl",
            Self::LiveTranscriptWav => "live-transcript-wav",
            Self::ScreenshotDirectory => "screenshot-directory",
            Self::PreservedCapture => "preserved-capture",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "audio-capture" => Self::AudioCapture,
            "markdown-artifact" => Self::MarkdownArtifact,
            "live-transcript-jsonl" => Self::LiveTranscriptJsonl,
            "live-transcript-wav" => Self::LiveTranscriptWav,
            "screenshot-directory" => Self::ScreenshotDirectory,
            "preserved-capture" => Self::PreservedCapture,
            _ => Self::Job,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSession {
    pub id: String,
    pub session_type: ContextSessionType,
    pub state: ContextSessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_mode: Option<CaptureMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<ContentType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub started_at: DateTime<Local>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Local>>,
    #[serde(default, skip_serializing_if = "metadata_is_empty")]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextLink {
    pub session_id: String,
    pub kind: ContextLinkKind,
    pub target: String,
    pub linked_at: DateTime<Local>,
    #[serde(default, skip_serializing_if = "metadata_is_empty")]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEvent {
    pub id: i64,
    pub session_id: String,
    pub observed_at: DateTime<Local>,
    pub source: ContextEventSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<String>,
    pub privacy_scope: ContextPrivacyScope,
    #[serde(default, skip_serializing_if = "metadata_is_empty")]
    pub metadata: Value,
}

#[derive(Debug, Clone)]
pub struct NewContextSession {
    pub session_type: ContextSessionType,
    pub capture_mode: Option<CaptureMode>,
    pub content_type: Option<ContentType>,
    pub title: Option<String>,
    pub started_at: DateTime<Local>,
    pub metadata: Value,
}

#[derive(Debug, Clone)]
pub struct NewContextEvent {
    pub observed_at: DateTime<Local>,
    pub source: ContextEventSource,
    pub app_name: Option<String>,
    pub bundle_id: Option<String>,
    pub window_title: Option<String>,
    pub url: Option<String>,
    pub domain: Option<String>,
    pub artifact_path: Option<String>,
    pub privacy_scope: ContextPrivacyScope,
    pub metadata: Value,
}

fn metadata_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Object(map) => map.is_empty(),
        _ => false,
    }
}

fn metadata_to_string(value: &Value) -> Result<String, ContextStoreError> {
    Ok(if metadata_is_empty(value) {
        "{}".to_string()
    } else {
        serde_json::to_string(value)?
    })
}

fn metadata_from_db(raw: String) -> Value {
    serde_json::from_str(&raw).unwrap_or_else(|_| json!({}))
}

fn timestamp_to_db(ts: DateTime<Local>) -> String {
    ts.to_rfc3339()
}

fn timestamp_from_db(raw: String) -> Result<DateTime<Local>, ContextStoreError> {
    let parsed = DateTime::parse_from_rfc3339(&raw).map_err(|source| {
        ContextStoreError::InvalidTimestamp {
            value: raw.clone(),
            source,
        }
    })?;
    Ok(parsed.with_timezone(&Local))
}

fn next_session_id(kind: ContextSessionType, started_at: DateTime<Local>) -> String {
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "ctx-{}-{}-{}-{}",
        kind.as_str(),
        started_at.format("%Y%m%d%H%M%S%3f"),
        std::process::id(),
        counter
    )
}

/// Database path: ~/.minutes/context.db
pub fn db_path() -> PathBuf {
    let base = Config::minutes_dir();
    std::fs::create_dir_all(&base).ok();
    base.join("context.db")
}

pub fn open_db() -> Result<Connection, ContextStoreError> {
    open_db_at(&db_path())
}

pub fn open_db_at(path: &Path) -> Result<Connection, ContextStoreError> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;",
    )?;
    create_schema(&conn)?;
    set_db_permissions(path);
    Ok(conn)
}

fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

#[cfg(unix)]
fn set_db_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    for candidate in [
        path.to_path_buf(),
        sqlite_sidecar_path(path, "-wal"),
        sqlite_sidecar_path(path, "-shm"),
    ] {
        if candidate.exists() {
            std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o600)).ok();
        }
    }
}

#[cfg(not(unix))]
fn set_db_permissions(_path: &Path) {}

fn create_schema(conn: &Connection) -> Result<(), ContextStoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS context_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS context_sessions (
            id TEXT PRIMARY KEY,
            session_type TEXT NOT NULL,
            state TEXT NOT NULL,
            capture_mode TEXT,
            content_type TEXT,
            title TEXT,
            started_at TEXT NOT NULL,
            ended_at TEXT,
            metadata_json TEXT NOT NULL DEFAULT '{}'
        );
        CREATE TABLE IF NOT EXISTS context_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL REFERENCES context_sessions(id) ON DELETE CASCADE,
            observed_at TEXT NOT NULL,
            source TEXT NOT NULL,
            app_name TEXT,
            bundle_id TEXT,
            window_title TEXT,
            url TEXT,
            domain TEXT,
            artifact_path TEXT,
            privacy_scope TEXT NOT NULL DEFAULT 'normal',
            metadata_json TEXT NOT NULL DEFAULT '{}'
        );
        CREATE TABLE IF NOT EXISTS context_links (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL REFERENCES context_sessions(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            target TEXT NOT NULL,
            linked_at TEXT NOT NULL,
            metadata_json TEXT NOT NULL DEFAULT '{}',
            UNIQUE(session_id, kind, target)
        );
        CREATE INDEX IF NOT EXISTS idx_context_sessions_started_at
            ON context_sessions(started_at);
        CREATE INDEX IF NOT EXISTS idx_context_sessions_state
            ON context_sessions(state);
        CREATE INDEX IF NOT EXISTS idx_context_events_session_time
            ON context_events(session_id, observed_at);
        CREATE INDEX IF NOT EXISTS idx_context_events_time
            ON context_events(observed_at);
        CREATE INDEX IF NOT EXISTS idx_context_events_source
            ON context_events(source);
        CREATE INDEX IF NOT EXISTS idx_context_links_kind_target
            ON context_links(kind, target);
        CREATE INDEX IF NOT EXISTS idx_context_links_session
            ON context_links(session_id, linked_at);",
    )?;
    conn.execute(
        "INSERT INTO context_meta (key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

fn row_to_session(row: &rusqlite::Row<'_>) -> Result<ContextSession, ContextStoreError> {
    let capture_mode = row
        .get::<_, Option<String>>("capture_mode")?
        .and_then(|value| match value.as_str() {
            "meeting" => Some(CaptureMode::Meeting),
            "quick-thought" => Some(CaptureMode::QuickThought),
            "dictation" => Some(CaptureMode::Dictation),
            "live-transcript" => Some(CaptureMode::LiveTranscript),
            _ => None,
        });
    let content_type = row
        .get::<_, Option<String>>("content_type")?
        .and_then(|value| match value.as_str() {
            "meeting" => Some(ContentType::Meeting),
            "memo" => Some(ContentType::Memo),
            "dictation" => Some(ContentType::Dictation),
            _ => None,
        });

    Ok(ContextSession {
        id: row.get("id")?,
        session_type: ContextSessionType::from_str(&row.get::<_, String>("session_type")?),
        state: ContextSessionState::from_str(&row.get::<_, String>("state")?),
        capture_mode,
        content_type,
        title: row.get("title")?,
        started_at: timestamp_from_db(row.get("started_at")?)?,
        ended_at: row
            .get::<_, Option<String>>("ended_at")?
            .map(timestamp_from_db)
            .transpose()?,
        metadata: metadata_from_db(row.get("metadata_json")?),
    })
}

fn row_to_link(row: &rusqlite::Row<'_>) -> Result<ContextLink, ContextStoreError> {
    Ok(ContextLink {
        session_id: row.get("session_id")?,
        kind: ContextLinkKind::from_str(&row.get::<_, String>("kind")?),
        target: row.get("target")?,
        linked_at: timestamp_from_db(row.get("linked_at")?)?,
        metadata: metadata_from_db(row.get("metadata_json")?),
    })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> Result<ContextEvent, ContextStoreError> {
    Ok(ContextEvent {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        observed_at: timestamp_from_db(row.get("observed_at")?)?,
        source: ContextEventSource::from_str(&row.get::<_, String>("source")?),
        app_name: row.get("app_name")?,
        bundle_id: row.get("bundle_id")?,
        window_title: row.get("window_title")?,
        url: row.get("url")?,
        domain: row.get("domain")?,
        artifact_path: row.get("artifact_path")?,
        privacy_scope: ContextPrivacyScope::from_str(&row.get::<_, String>("privacy_scope")?),
        metadata: metadata_from_db(row.get("metadata_json")?),
    })
}

fn capture_mode_to_db(mode: CaptureMode) -> &'static str {
    match mode {
        CaptureMode::Meeting => "meeting",
        CaptureMode::QuickThought => "quick-thought",
        CaptureMode::Dictation => "dictation",
        CaptureMode::LiveTranscript => "live-transcript",
    }
}

fn content_type_to_db(kind: ContentType) -> &'static str {
    match kind {
        ContentType::Meeting => "meeting",
        ContentType::Memo => "memo",
        ContentType::Dictation => "dictation",
    }
}

pub fn session_type_for_capture_mode(mode: CaptureMode) -> ContextSessionType {
    match mode {
        CaptureMode::Meeting => ContextSessionType::Recording,
        CaptureMode::QuickThought => ContextSessionType::MemoWindow,
        CaptureMode::Dictation => ContextSessionType::FocusSession,
        CaptureMode::LiveTranscript => ContextSessionType::LiveTranscript,
    }
}

pub fn start_session(new_session: NewContextSession) -> Result<ContextSession, ContextStoreError> {
    let conn = open_db()?;
    start_session_with_conn(&conn, new_session)
}

fn start_session_with_conn(
    conn: &Connection,
    new_session: NewContextSession,
) -> Result<ContextSession, ContextStoreError> {
    let session = ContextSession {
        id: next_session_id(new_session.session_type, new_session.started_at),
        session_type: new_session.session_type,
        state: ContextSessionState::Active,
        capture_mode: new_session.capture_mode,
        content_type: new_session.content_type,
        title: new_session.title,
        started_at: new_session.started_at,
        ended_at: None,
        metadata: new_session.metadata,
    };

    conn.execute(
        "INSERT INTO context_sessions
         (id, session_type, state, capture_mode, content_type, title, started_at, ended_at, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8)",
        params![
            session.id,
            session.session_type.as_str(),
            session.state.as_str(),
            session.capture_mode.map(capture_mode_to_db),
            session.content_type.map(content_type_to_db),
            session.title,
            timestamp_to_db(session.started_at),
            metadata_to_string(&session.metadata)?,
        ],
    )?;

    get_session_with_conn(conn, &session.id)?.ok_or_else(|| {
        ContextStoreError::Io(std::io::Error::other(
            "failed to read inserted context session",
        ))
    })
}

pub fn get_session(session_id: &str) -> Result<Option<ContextSession>, ContextStoreError> {
    let conn = open_db()?;
    get_session_with_conn(&conn, session_id)
}

fn get_session_with_conn(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<ContextSession>, ContextStoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, session_type, state, capture_mode, content_type, title, started_at, ended_at, metadata_json
         FROM context_sessions
         WHERE id = ?1",
    )?;
    let mut rows = stmt.query(params![session_id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_session(row)?))
    } else {
        Ok(None)
    }
}

pub fn get_session_for_link(
    kind: ContextLinkKind,
    target: &str,
) -> Result<Option<ContextSession>, ContextStoreError> {
    let conn = open_db()?;
    let mut stmt = conn.prepare(
        "SELECT s.id, s.session_type, s.state, s.capture_mode, s.content_type, s.title, s.started_at, s.ended_at, s.metadata_json
         FROM context_links l
         JOIN context_sessions s ON s.id = l.session_id
         WHERE l.kind = ?1 AND l.target = ?2
         ORDER BY l.linked_at DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query(params![kind.as_str(), target])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_session(row)?))
    } else {
        Ok(None)
    }
}

pub fn get_session_for_artifact(target: &str) -> Result<Option<ContextSession>, ContextStoreError> {
    let conn = open_db()?;
    let mut stmt = conn.prepare(
        "SELECT s.id, s.session_type, s.state, s.capture_mode, s.content_type, s.title, s.started_at, s.ended_at, s.metadata_json
         FROM context_links l
         JOIN context_sessions s ON s.id = l.session_id
         WHERE l.target = ?1
         ORDER BY l.linked_at DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query(params![target])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_session(row)?))
    } else {
        Ok(None)
    }
}

pub fn list_links_for_session(session_id: &str) -> Result<Vec<ContextLink>, ContextStoreError> {
    let conn = open_db()?;
    let mut stmt = conn.prepare(
        "SELECT session_id, kind, target, linked_at, metadata_json
         FROM context_links
         WHERE session_id = ?1
         ORDER BY linked_at ASC, id ASC",
    )?;
    let mut rows = stmt.query(params![session_id])?;
    let mut links = Vec::new();
    while let Some(row) = rows.next()? {
        links.push(row_to_link(row)?);
    }
    Ok(links)
}

pub fn upsert_link(
    session_id: &str,
    kind: ContextLinkKind,
    target: &str,
    metadata: Value,
) -> Result<(), ContextStoreError> {
    let conn = open_db()?;
    upsert_link_with_conn(&conn, session_id, kind, target, metadata)
}

fn upsert_link_with_conn(
    conn: &Connection,
    session_id: &str,
    kind: ContextLinkKind,
    target: &str,
    metadata: Value,
) -> Result<(), ContextStoreError> {
    conn.execute(
        "INSERT INTO context_links (session_id, kind, target, linked_at, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(session_id, kind, target) DO UPDATE SET
            linked_at = excluded.linked_at,
            metadata_json = excluded.metadata_json",
        params![
            session_id,
            kind.as_str(),
            target,
            timestamp_to_db(Local::now()),
            metadata_to_string(&metadata)?,
        ],
    )?;
    Ok(())
}

pub fn append_event(
    session_id: &str,
    event: NewContextEvent,
) -> Result<ContextEvent, ContextStoreError> {
    let conn = open_db()?;
    append_event_with_conn(&conn, session_id, event)
}

fn append_event_with_conn(
    conn: &Connection,
    session_id: &str,
    event: NewContextEvent,
) -> Result<ContextEvent, ContextStoreError> {
    let metadata = metadata_to_string(&event.metadata)?;
    conn.execute(
        "INSERT INTO context_events
         (session_id, observed_at, source, app_name, bundle_id, window_title, url, domain, artifact_path, privacy_scope, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            session_id,
            timestamp_to_db(event.observed_at),
            event.source.as_str(),
            event.app_name,
            event.bundle_id,
            event.window_title,
            event.url,
            event.domain,
            event.artifact_path,
            event.privacy_scope.as_str(),
            metadata,
        ],
    )?;
    let id = conn.last_insert_rowid();
    let mut stmt = conn.prepare(
        "SELECT id, session_id, observed_at, source, app_name, bundle_id, window_title, url, domain, artifact_path, privacy_scope, metadata_json
         FROM context_events WHERE id = ?1",
    )?;
    let mut rows = stmt.query(params![id])?;
    if let Some(row) = rows.next()? {
        Ok(row_to_event(row)?)
    } else {
        Err(ContextStoreError::Io(std::io::Error::other(
            "failed to read inserted context event",
        )))
    }
}

pub fn list_events_for_session(
    session_id: &str,
    start: Option<DateTime<Local>>,
    end: Option<DateTime<Local>>,
) -> Result<Vec<ContextEvent>, ContextStoreError> {
    let conn = open_db()?;
    let start_db = start.map(timestamp_to_db);
    let end_db = end.map(timestamp_to_db);
    let mut stmt = conn.prepare(
        "SELECT id, session_id, observed_at, source, app_name, bundle_id, window_title, url, domain, artifact_path, privacy_scope, metadata_json
         FROM context_events
         WHERE session_id = ?1
           AND (?2 IS NULL OR observed_at >= ?2)
           AND (?3 IS NULL OR observed_at <= ?3)
         ORDER BY observed_at ASC, id ASC",
    )?;
    let mut rows = stmt.query(params![session_id, start_db.as_deref(), end_db.as_deref()])?;
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        events.push(row_to_event(row)?);
    }
    Ok(events)
}

pub fn list_events_in_window(
    start: DateTime<Local>,
    end: DateTime<Local>,
) -> Result<Vec<ContextEvent>, ContextStoreError> {
    let conn = open_db()?;
    let mut stmt = conn.prepare(
        "SELECT id, session_id, observed_at, source, app_name, bundle_id, window_title, url, domain, artifact_path, privacy_scope, metadata_json
         FROM context_events
         WHERE observed_at >= ?1 AND observed_at <= ?2
         ORDER BY observed_at ASC, id ASC",
    )?;
    let mut rows = stmt.query(params![timestamp_to_db(start), timestamp_to_db(end)])?;
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        events.push(row_to_event(row)?);
    }
    Ok(events)
}

pub fn search_events(query: &str, limit: usize) -> Result<Vec<ContextEvent>, ContextStoreError> {
    let conn = open_db()?;
    let pattern = format!("%{}%", query.to_ascii_lowercase());
    let mut stmt = conn.prepare(
        "SELECT id, session_id, observed_at, source, app_name, bundle_id, window_title, url, domain, artifact_path, privacy_scope, metadata_json
         FROM context_events
         WHERE lower(coalesce(app_name, '')) LIKE ?1
            OR lower(coalesce(bundle_id, '')) LIKE ?1
            OR lower(coalesce(window_title, '')) LIKE ?1
            OR lower(coalesce(url, '')) LIKE ?1
            OR lower(coalesce(domain, '')) LIKE ?1
            OR lower(coalesce(artifact_path, '')) LIKE ?1
         ORDER BY observed_at DESC, id DESC
         LIMIT ?2",
    )?;
    let mut rows = stmt.query(params![pattern, limit as i64])?;
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        events.push(row_to_event(row)?);
    }
    Ok(events)
}

pub fn get_session_covering_time(
    at: DateTime<Local>,
) -> Result<Option<ContextSession>, ContextStoreError> {
    let conn = open_db()?;
    let at_db = timestamp_to_db(at);
    let mut stmt = conn.prepare(
        "SELECT id, session_type, state, capture_mode, content_type, title, started_at, ended_at, metadata_json
         FROM context_sessions
         WHERE started_at <= ?1
           AND (ended_at IS NULL OR ended_at >= ?1)
         ORDER BY started_at DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query(params![at_db])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row_to_session(row)?))
    } else {
        Ok(None)
    }
}

pub fn update_session_state(
    session_id: &str,
    state: ContextSessionState,
    ended_at: Option<DateTime<Local>>,
    title: Option<&str>,
    content_type: Option<ContentType>,
    metadata_patch: Value,
) -> Result<(), ContextStoreError> {
    let conn = open_db()?;
    update_session_state_with_conn(
        &conn,
        session_id,
        state,
        ended_at,
        title,
        content_type,
        metadata_patch,
    )
}

fn update_session_state_with_conn(
    conn: &Connection,
    session_id: &str,
    state: ContextSessionState,
    ended_at: Option<DateTime<Local>>,
    title: Option<&str>,
    content_type: Option<ContentType>,
    metadata_patch: Value,
) -> Result<(), ContextStoreError> {
    let Some(existing) = get_session_with_conn(conn, session_id)? else {
        return Ok(());
    };

    let mut merged_metadata = existing.metadata;
    merge_metadata(&mut merged_metadata, metadata_patch);

    conn.execute(
        "UPDATE context_sessions
         SET state = ?2,
             ended_at = COALESCE(?3, ended_at),
             title = COALESCE(?4, title),
             content_type = COALESCE(?5, content_type),
             metadata_json = ?6
         WHERE id = ?1",
        params![
            session_id,
            state.as_str(),
            ended_at.map(timestamp_to_db),
            title,
            content_type.map(content_type_to_db),
            metadata_to_string(&merged_metadata)?,
        ],
    )?;
    Ok(())
}

fn merge_metadata(existing: &mut Value, patch: Value) {
    match patch {
        Value::Null => {}
        Value::Object(patch_map) => {
            if !existing.is_object() {
                *existing = json!({});
            }
            let existing_map = existing.as_object_mut().expect("object");
            for (key, value) in patch_map {
                existing_map.insert(key, value);
            }
        }
        other => {
            *existing = other;
        }
    }
}

pub fn start_capture_session(
    mode: CaptureMode,
    title: Option<String>,
    started_at: DateTime<Local>,
) -> Result<ContextSession, ContextStoreError> {
    start_session(NewContextSession {
        session_type: session_type_for_capture_mode(mode),
        capture_mode: Some(mode),
        content_type: Some(mode.content_type()),
        title,
        started_at,
        metadata: json!({}),
    })
}

pub fn mark_capture_session_processing(
    session_id: &str,
    job_id: &str,
    audio_path: &Path,
    ended_at: Option<DateTime<Local>>,
) -> Result<(), ContextStoreError> {
    let conn = open_db()?;
    update_session_state_with_conn(
        &conn,
        session_id,
        ContextSessionState::Processing,
        ended_at,
        None,
        None,
        json!({ "job_id": job_id }),
    )?;
    upsert_link_with_conn(&conn, session_id, ContextLinkKind::Job, job_id, json!({}))?;
    upsert_link_with_conn(
        &conn,
        session_id,
        ContextLinkKind::AudioCapture,
        &audio_path.display().to_string(),
        json!({}),
    )?;
    Ok(())
}

pub fn mark_capture_session_complete(
    session_id: &str,
    output_path: &Path,
    audio_path: Option<&Path>,
    content_type: ContentType,
    ended_at: Option<DateTime<Local>>,
    metadata: Value,
) -> Result<(), ContextStoreError> {
    let conn = open_db()?;
    update_session_state_with_conn(
        &conn,
        session_id,
        ContextSessionState::Complete,
        ended_at,
        None,
        Some(content_type),
        metadata,
    )?;
    upsert_link_with_conn(
        &conn,
        session_id,
        ContextLinkKind::MarkdownArtifact,
        &output_path.display().to_string(),
        json!({ "content_type": content_type_to_db(content_type) }),
    )?;
    if let Some(audio_path) = audio_path {
        upsert_link_with_conn(
            &conn,
            session_id,
            ContextLinkKind::AudioCapture,
            &audio_path.display().to_string(),
            json!({}),
        )?;
    }
    Ok(())
}

pub fn mark_capture_session_failed(
    session_id: &str,
    ended_at: Option<DateTime<Local>>,
    diagnostic: &str,
    preserved_path: Option<&Path>,
) -> Result<(), ContextStoreError> {
    let conn = open_db()?;
    update_session_state_with_conn(
        &conn,
        session_id,
        ContextSessionState::Failed,
        ended_at,
        None,
        None,
        json!({ "diagnostic": diagnostic }),
    )?;
    if let Some(path) = preserved_path {
        upsert_link_with_conn(
            &conn,
            session_id,
            ContextLinkKind::PreservedCapture,
            &path.display().to_string(),
            json!({}),
        )?;
    }
    Ok(())
}

pub fn mark_capture_session_discarded(
    session_id: &str,
    ended_at: Option<DateTime<Local>>,
) -> Result<(), ContextStoreError> {
    update_session_state(
        session_id,
        ContextSessionState::Discarded,
        ended_at,
        None,
        None,
        json!({ "discarded": true }),
    )
}

pub fn start_live_transcript_session(
    started_at: DateTime<Local>,
) -> Result<ContextSession, ContextStoreError> {
    start_session(NewContextSession {
        session_type: ContextSessionType::LiveTranscript,
        capture_mode: Some(CaptureMode::LiveTranscript),
        content_type: None,
        title: None,
        started_at,
        metadata: json!({}),
    })
}

pub fn mark_live_transcript_complete(
    session_id: &str,
    jsonl_path: &Path,
    wav_path: Option<&Path>,
    ended_at: Option<DateTime<Local>>,
    metadata: Value,
) -> Result<(), ContextStoreError> {
    let conn = open_db()?;
    update_session_state_with_conn(
        &conn,
        session_id,
        ContextSessionState::Complete,
        ended_at,
        None,
        None,
        metadata,
    )?;
    upsert_link_with_conn(
        &conn,
        session_id,
        ContextLinkKind::LiveTranscriptJsonl,
        &jsonl_path.display().to_string(),
        json!({}),
    )?;
    if let Some(wav_path) = wav_path {
        upsert_link_with_conn(
            &conn,
            session_id,
            ContextLinkKind::LiveTranscriptWav,
            &wav_path.display().to_string(),
            json!({}),
        )?;
    }
    Ok(())
}

pub fn mark_live_transcript_failed(
    session_id: &str,
    ended_at: Option<DateTime<Local>>,
    diagnostic: &str,
) -> Result<(), ContextStoreError> {
    update_session_state(
        session_id,
        ContextSessionState::Failed,
        ended_at,
        None,
        None,
        json!({ "diagnostic": diagnostic }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use tempfile::TempDir;

    fn with_temp_home<T>(f: impl FnOnce(&TempDir) -> T) -> T {
        let _lock = crate::test_home_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let original_home = std::env::var_os("HOME");
        #[cfg(windows)]
        let original_userprofile = std::env::var_os("USERPROFILE");

        std::env::set_var("HOME", dir.path());
        #[cfg(windows)]
        std::env::set_var("USERPROFILE", dir.path());

        let result = f(&dir);

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }

        #[cfg(windows)]
        if let Some(profile) = original_userprofile {
            std::env::set_var("USERPROFILE", profile);
        } else {
            std::env::remove_var("USERPROFILE");
        }

        result
    }

    #[test]
    fn capture_session_lifecycle_and_queries_round_trip() {
        with_temp_home(|_| {
            let started_at = Local::now();
            let session = start_capture_session(
                CaptureMode::Meeting,
                Some("Roadmap Review".into()),
                started_at,
            )
            .unwrap();

            append_event(
                &session.id,
                NewContextEvent {
                    observed_at: started_at + Duration::seconds(2),
                    source: ContextEventSource::AppFocus,
                    app_name: Some("Zoom".into()),
                    bundle_id: Some("us.zoom.xos".into()),
                    window_title: Some("Roadmap Review".into()),
                    url: None,
                    domain: None,
                    artifact_path: None,
                    privacy_scope: ContextPrivacyScope::Normal,
                    metadata: json!({ "confidence": 0.9 }),
                },
            )
            .unwrap();

            append_event(
                &session.id,
                NewContextEvent {
                    observed_at: started_at + Duration::seconds(6),
                    source: ContextEventSource::ScreenshotRef,
                    app_name: Some("Keynote".into()),
                    bundle_id: None,
                    window_title: Some("Q2 Plan".into()),
                    url: None,
                    domain: None,
                    artifact_path: Some("/tmp/shot-1.png".into()),
                    privacy_scope: ContextPrivacyScope::Filtered,
                    metadata: json!({}),
                },
            )
            .unwrap();

            mark_capture_session_processing(
                &session.id,
                "job-123",
                Path::new("/tmp/job-123.wav"),
                Some(started_at + Duration::seconds(30)),
            )
            .unwrap();

            mark_capture_session_complete(
                &session.id,
                Path::new("/tmp/2026-04-22-roadmap-review.md"),
                Some(Path::new("/tmp/2026-04-22-roadmap-review.wav")),
                ContentType::Meeting,
                Some(started_at + Duration::seconds(45)),
                json!({ "job_state": "complete" }),
            )
            .unwrap();

            let reloaded = get_session(&session.id).unwrap().unwrap();
            assert_eq!(reloaded.state, ContextSessionState::Complete);
            assert_eq!(reloaded.session_type, ContextSessionType::Recording);
            assert_eq!(reloaded.capture_mode, Some(CaptureMode::Meeting));

            let linked = get_session_for_link(
                ContextLinkKind::MarkdownArtifact,
                "/tmp/2026-04-22-roadmap-review.md",
            )
            .unwrap()
            .unwrap();
            assert_eq!(linked.id, session.id);

            let links = list_links_for_session(&session.id).unwrap();
            assert!(links.iter().any(|link| link.kind == ContextLinkKind::Job));
            assert!(links
                .iter()
                .any(|link| link.kind == ContextLinkKind::MarkdownArtifact));

            let events = list_events_for_session(
                &session.id,
                Some(started_at),
                Some(started_at + Duration::seconds(10)),
            )
            .unwrap();
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].source, ContextEventSource::AppFocus);

            let window_events = list_events_in_window(
                started_at + Duration::seconds(1),
                started_at + Duration::seconds(7),
            )
            .unwrap();
            assert_eq!(window_events.len(), 2);
        });
    }

    #[test]
    fn quick_thought_maps_to_memo_window_session_type() {
        assert_eq!(
            session_type_for_capture_mode(CaptureMode::QuickThought),
            ContextSessionType::MemoWindow
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_db_hardens_main_db_and_wal_sidecars_to_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("context.db");
        let conn = open_db_at(&path).unwrap();
        conn.execute_batch(
            "PRAGMA wal_autocheckpoint=0;
             INSERT INTO context_meta (key, value) VALUES ('perm-test', '1')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
        )
        .unwrap();

        let wal_path = sqlite_sidecar_path(&path, "-wal");
        let shm_path = sqlite_sidecar_path(&path, "-shm");

        assert!(path.exists());
        assert!(wal_path.exists());
        assert!(shm_path.exists());

        for candidate in [&path, &wal_path, &shm_path] {
            let mode = std::fs::metadata(candidate).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{} should be 0600", candidate.display());
        }
    }
}
