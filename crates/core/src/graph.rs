use crate::config::Config;
use crate::diarize::SpeakerAttribution;
use crate::markdown::{
    is_inactive_corpus_dir_name, read_stable_active_markdown_with_budget, split_frontmatter,
    ActiveCorpusReadBudget, ContentType, EntityRef, Frontmatter, IntentKind, Sensitivity,
    StableMarkdownSnapshot, ACTIVE_CORPUS_AUTHORIZATION_DEADLINE,
};
use crate::overlays;
use crate::person_identity::PersonCanonicalizer;
use chrono::Local;
#[cfg(all(not(windows), not(target_os = "macos")))]
use notify::{Config as NotifyConfig, RecommendedWatcher, Watcher};
#[cfg(any(test, all(not(windows), not(target_os = "macos"))))]
use notify::{EventKind as NotifyEventKind, RecursiveMode};
use rusqlite::{limits::Limit, params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
#[cfg(test)]
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "macos"))]
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard};
use thiserror::Error;
use walkdir::WalkDir;

const MAX_CORRECTION_FILE_BYTES: u64 = 16 * 1024 * 1024;
const CORRECTION_READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);
const MAX_GRAPH_DERIVED_ITEMS: usize = 1_000_000;
const MAX_GRAPH_ALIAS_PEOPLE: usize = 10_000;
const MAX_GRAPH_QUERY_ROWS: usize = 10_000;
const MAX_GRAPH_QUERY_RETAINED_BYTES: usize = 16 * 1024 * 1024;
const MAX_GRAPH_TOPIC_ASSOCIATIONS_PER_PERSON: usize = 100_000;
const MAX_GRAPH_RETAINED_PATH_BYTES: usize = 8 * 1024 * 1024;
const MAX_GRAPH_ENTITY_FIELD_BYTES: usize = 4 * 1024;
const MAX_GRAPH_ENTITY_STRING_BYTES: usize = 16 * 1024 * 1024;
const MAX_GRAPH_FRONTMATTER_BYTES: usize = 256 * 1024;
const GRAPH_SQLITE_PAGE_BYTES: i64 = 4 * 1024;
const MAX_GRAPH_SQLITE_PAGE_COUNT: i64 = 16 * 1024;
const MAX_GRAPH_SQLITE_TEMP_PAGE_COUNT: i64 = 8 * 1024;
const GRAPH_SQLITE_CACHE_KIB: i64 = 8 * 1024;
const GRAPH_SQLITE_TEMP_CACHE_KIB: i64 = 4 * 1024;
const MAX_GRAPH_SQLITE_VALUE_BYTES: i32 = 4 * 1024 * 1024;
const MAX_GRAPH_SQLITE_SQL_BYTES: i32 = 64 * 1024;
const MAX_GRAPH_SQLITE_COLUMNS: i32 = 64;
const MAX_GRAPH_SQLITE_EXPR_DEPTH: i32 = 64;
const MAX_GRAPH_SQLITE_COMPOUND_TERMS: i32 = 8;
const MAX_GRAPH_SQLITE_VDBE_OPS: i32 = 100_000;
const MAX_GRAPH_SQLITE_FUNCTION_ARGS: i32 = 32;
const MAX_GRAPH_SQLITE_LIKE_PATTERN_BYTES: i32 = 4 * 1024;
const MAX_GRAPH_SQLITE_VARIABLES: i32 = 64;
// One policy-fresh answer may use at most six independently bounded scans per
// attempt (corrections, projection build, and the four-part post-query
// attestation) and at most three attempts. Keeping each scan on the normal
// active-corpus envelope avoids shrinking the supported corpus to one third,
// while this operation envelope still bounds cumulative work.
const MAX_GRAPH_OPERATION_PASSES: usize = 18;
#[cfg(any(test, not(target_os = "macos")))]
const GRAPH_SNAPSHOT_FENCE_DIRECTORY: &str = ".minutes-graph-snapshot-fences";
#[cfg(not(target_os = "macos"))]
const GRAPH_SNAPSHOT_FENCE_BYTES: &[u8] = b"minutes-graph-snapshot-fence-v1";
#[cfg(not(target_os = "macos"))]
const GRAPH_SNAPSHOT_FENCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
static GRAPH_PROJECTION_ADMISSION: Mutex<()> = Mutex::new(());

/// One graph projection owns both the in-process slot and the shared
/// cross-process private-projection lease. Search and graph intentionally use
/// the same filesystem lease, so separate CLI/MCP/app processes cannot each
/// amplify one corpus into an independent SQLite heap.
struct GraphProjectionAdmission {
    _in_process: MutexGuard<'static, ()>,
    _cross_process: crate::policy_fs::BoundRecoveryLeaseFile,
}

fn ensure_graph_entity_field(value: &str) -> Result<(), GraphError> {
    if value.len() > MAX_GRAPH_ENTITY_FIELD_BYTES {
        return Err(GraphError::Io(std::io::Error::other(
            "graph entity field budget exceeded",
        )));
    }
    Ok(())
}

fn graph_corpus_budget_error() -> GraphError {
    GraphError::Io(std::io::Error::other(
        "graph corpus resource budget or deadline exceeded",
    ))
}

fn retain_graph_query_text(total: &mut usize, value: &str) -> Result<(), GraphError> {
    *total = total.checked_add(value.len()).ok_or_else(|| {
        GraphError::Io(std::io::Error::other(
            "graph query retained-byte budget overflowed",
        ))
    })?;
    if *total > MAX_GRAPH_QUERY_RETAINED_BYTES {
        return Err(GraphError::Io(std::io::Error::other(
            "graph query retained-byte budget exceeded",
        )));
    }
    Ok(())
}

fn consume_graph_corpus(
    budget: &ActiveCorpusReadBudget,
    files: usize,
    directories: usize,
    bytes: u64,
) -> Result<(), GraphError> {
    budget
        .consume(files, directories, bytes)
        .map_err(|_| graph_corpus_budget_error())
}

fn try_graph_projection_admission(
    admission: &'static Mutex<()>,
) -> Result<MutexGuard<'static, ()>, GraphError> {
    admission.try_lock().map_err(|error| {
        let message = match error {
            std::sync::TryLockError::Poisoned(_) => {
                "graph projection admission lock is unavailable"
            }
            std::sync::TryLockError::WouldBlock => {
                "another bounded graph projection is already active"
            }
        };
        GraphError::Io(std::io::Error::other(message))
    })
}

fn graph_projection_admission_at(
    canonical_corpus_root: &Path,
    wait_for_test_peer: bool,
) -> Result<GraphProjectionAdmission, GraphError> {
    let in_process = if wait_for_test_peer {
        GRAPH_PROJECTION_ADMISSION.lock().map_err(|_| {
            GraphError::Io(std::io::Error::other(
                "graph projection admission lock is unavailable",
            ))
        })?
    } else {
        try_graph_projection_admission(&GRAPH_PROJECTION_ADMISSION)?
    };

    let cross_process = crate::policy_fs::acquire_private_corpus_projection_lease(
        canonical_corpus_root,
        wait_for_test_peer,
    )
    .map_err(|_| {
        GraphError::Io(std::io::Error::other(
            "private graph projection capacity is unavailable",
        ))
    })?;

    Ok(GraphProjectionAdmission {
        _in_process: in_process,
        _cross_process: cross_process,
    })
}

fn graph_projection_admission(
    canonical_corpus_root: &Path,
) -> Result<GraphProjectionAdmission, GraphError> {
    // Anchor admission to the corpus itself, not configurable MINUTES_HOME.
    // Two processes pointed at the same meetings with different state roots
    // must still contend for one projection heap. Hidden directories are
    // excluded by every active-corpus walker.
    graph_projection_admission_at(canonical_corpus_root, cfg!(test))
}

fn graph_operation_deadline() -> std::time::Instant {
    std::time::Instant::now() + ACTIVE_CORPUS_AUTHORIZATION_DEADLINE
}

fn graph_scan_budget(deadline: std::time::Instant) -> ActiveCorpusReadBudget {
    ActiveCorpusReadBudget::new_until(deadline)
}

struct GraphOperationBudget {
    deadline: std::time::Instant,
    passes: usize,
}

impl GraphOperationBudget {
    fn new(deadline: std::time::Instant) -> Self {
        Self {
            deadline,
            passes: 0,
        }
    }

    fn next_pass(&mut self) -> Result<ActiveCorpusReadBudget, GraphError> {
        if std::time::Instant::now() >= self.deadline {
            return Err(graph_corpus_budget_error());
        }
        self.passes = self.passes.checked_add(1).ok_or_else(|| {
            GraphError::Io(std::io::Error::other(
                "graph operation pass budget overflowed",
            ))
        })?;
        if self.passes > MAX_GRAPH_OPERATION_PASSES {
            return Err(GraphError::Io(std::io::Error::other(
                "graph operation pass budget exceeded",
            )));
        }
        Ok(graph_scan_budget(self.deadline))
    }
}

#[derive(Debug, Default)]
struct GraphDerivedBudget {
    items: usize,
    retained_path_bytes: usize,
    entity_string_bytes: usize,
}

impl GraphDerivedBudget {
    fn consume(&mut self, items: usize, corpus: &ActiveCorpusReadBudget) -> Result<(), GraphError> {
        corpus.check_deadline().map_err(|_| {
            GraphError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "graph projection deadline elapsed",
            ))
        })?;
        self.items = self.items.checked_add(items).ok_or_else(|| {
            GraphError::Io(std::io::Error::other(
                "graph projection item budget overflowed",
            ))
        })?;
        if self.items > MAX_GRAPH_DERIVED_ITEMS {
            return Err(GraphError::Io(std::io::Error::other(
                "graph projection item budget exceeded",
            )));
        }
        Ok(())
    }

    fn consume_path(
        &mut self,
        path: &Path,
        corpus: &ActiveCorpusReadBudget,
    ) -> Result<(), GraphError> {
        corpus
            .check_deadline()
            .map_err(|_| graph_corpus_budget_error())?;
        self.retained_path_bytes = self
            .retained_path_bytes
            .checked_add(path.as_os_str().len())
            .ok_or_else(|| {
                GraphError::Io(std::io::Error::other(
                    "graph retained-path budget overflowed",
                ))
            })?;
        if self.retained_path_bytes > MAX_GRAPH_RETAINED_PATH_BYTES {
            return Err(GraphError::Io(std::io::Error::other(
                "graph retained-path budget exceeded",
            )));
        }
        Ok(())
    }

    fn consume_entity_text(
        &mut self,
        value: &str,
        corpus: &ActiveCorpusReadBudget,
    ) -> Result<(), GraphError> {
        corpus
            .check_deadline()
            .map_err(|_| graph_corpus_budget_error())?;
        ensure_graph_entity_field(value)?;
        self.entity_string_bytes = self
            .entity_string_bytes
            .checked_add(value.len())
            .ok_or_else(|| {
                GraphError::Io(std::io::Error::other(
                    "graph entity-string budget overflowed",
                ))
            })?;
        if self.entity_string_bytes > MAX_GRAPH_ENTITY_STRING_BYTES {
            return Err(GraphError::Io(std::io::Error::other(
                "graph entity-string budget exceeded",
            )));
        }
        Ok(())
    }

    fn consume_entity_ref(
        &mut self,
        entity: &EntityRef,
        corpus: &ActiveCorpusReadBudget,
    ) -> Result<(), GraphError> {
        self.consume(2usize.saturating_add(entity.aliases.len()), corpus)?;
        self.consume_entity_text(&entity.slug, corpus)?;
        self.consume_entity_text(&entity.label, corpus)?;
        for alias in &entity.aliases {
            self.consume_entity_text(alias, corpus)?;
        }
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────
// Conversation graph: process-private SQLite projection derived from stable,
// policy-authorized Markdown plus an attested correction snapshot.
//
// Markdown remains the source of truth. Every public graph answer builds a
// disposable projection, materializes the answer, then re-attests both source
// and correction revisions before returning it. No graph.db is retained.
// ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum GraphError {
    #[error("meetings directory does not exist: {0}")]
    DirNotFound(String),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphStats {
    pub people_count: usize,
    pub meeting_count: usize,
    pub commitment_count: usize,
    pub topic_count: usize,
    pub alias_suggestions: Vec<AliasSuggestion>,
    pub alias_clusters: Vec<AliasCluster>,
    pub rebuild_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonSummary {
    pub slug: String,
    pub name: String,
    pub meeting_count: i64,
    pub last_seen: String,
    pub days_since: f64,
    pub open_commitments: i64,
    pub top_topics: Vec<String>,
    pub score: f64,
    pub losing_touch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commitment {
    pub text: String,
    pub status: String,
    pub due_date: Option<String>,
    pub created_at: String,
    pub commitment_type: String,
    pub meeting_title: String,
    pub meeting_date: String,
    pub person_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipContext {
    pub people: Vec<PersonSummary>,
    pub commitments: Vec<Commitment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyPeopleProjection {
    pub stats: Option<GraphStats>,
    pub people: Vec<PersonSummary>,
    pub commitments: Vec<Commitment>,
}

/// An exact-identity profile projected from the same policy-filtered graph
/// snapshot as relationship maps and commitments. The shape intentionally
/// matches the historical CLI profile contract while eliminating substring
/// identity matching and independently reopened correction state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyPersonProfile {
    pub name: String,
    pub recent_meetings: Vec<PolicyMeetingReference>,
    pub open_intents: Vec<PolicyIntentReference>,
    pub recent_decisions: Vec<PolicyDecisionReference>,
    pub top_topics: Vec<PolicyTopicSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyMeetingReference {
    pub path: PathBuf,
    pub title: String,
    pub date: String,
    pub content_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyIntentReference {
    pub path: PathBuf,
    pub title: String,
    pub date: String,
    pub content_type: String,
    pub kind: IntentKind,
    pub what: String,
    pub who: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who_original: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who_provenance: Option<String>,
    pub status: String,
    pub by_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecisionReference {
    pub path: PathBuf,
    pub title: String,
    pub date: String,
    pub what: String,
    pub who: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who_original: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who_provenance: Option<String>,
    pub by_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyTopicSummary {
    pub topic: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum PolicyProjectionRequest {
    RebuildStats,
    People {
        limit: usize,
        include_commitments: bool,
        include_stats: bool,
    },
    RelationshipMap {
        limit: usize,
    },
    RelationshipContext {
        limit: usize,
    },
    PersonProfile {
        selector: String,
    },
    Commitments {
        selector: Option<String>,
        limit: usize,
    },
    LosingTouch {
        limit: usize,
    },
    ParakeetBoostPhrases {
        limit: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", content = "value", rename_all = "snake_case")]
pub enum PolicyProjectionResponse {
    RebuildStats(GraphStats),
    People(PolicyPeopleProjection),
    RelationshipMap(Vec<PersonSummary>),
    RelationshipContext(RelationshipContext),
    PersonProfile(PolicyPersonProfile),
    Commitments(Vec<Commitment>),
    LosingTouch(Vec<PersonSummary>),
    ParakeetBoostPhrases(Vec<String>),
}

struct PendingGraphCommitment {
    person_id: Option<i64>,
    text: String,
    status: String,
    due_date: Option<String>,
    commitment_type: &'static str,
}

fn normalized_graph_commitment_field(value: Option<&str>) -> String {
    value
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn graph_commitment_key(
    text: &str,
    person_id: Option<i64>,
    owner: Option<&str>,
    due_date: Option<&str>,
) -> String {
    format!(
        "{}\0{}\0{}",
        normalized_graph_commitment_field(Some(text)),
        person_id
            .map(|id| format!("id:{id}"))
            .unwrap_or_else(|| normalized_graph_commitment_field(owner)),
        normalized_graph_commitment_field(due_date),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasSuggestion {
    pub name_a: String,
    pub name_b: String,
    pub shared_meetings: usize,
}

/// A group of people who are plausibly the same person (issue #385, class 3
/// name-variant fragmentation). Suggestion only — nothing is merged. Members form
/// a clique under the separator / same-first-letter edit predicate in
/// [`crate::entity_cluster`] (every pair directly matches), so transitive drift
/// chains never bridge distinct people. Prefix/last-name matches are surfaced
/// separately as pairwise `AliasSuggestion`s.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasCluster {
    /// Display names of the cluster members, sorted for deterministic output.
    pub members: Vec<String>,
    /// Slugs of the members, aligned with `members`.
    pub slugs: Vec<String>,
    /// Largest shared-meeting count among any pair in the cluster. Evidence for
    /// display only, never a gate: spelling-drift variants of one person often
    /// appear in *different* meetings, so `0` is common and expected.
    pub max_shared_meetings: usize,
}

/// Set 0600 permissions on the database file (meeting data is sensitive).
#[cfg(all(test, unix))]
fn set_db_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
    }
}

fn merge_person_aliases(existing: &mut Vec<String>, incoming: &[String]) {
    let mut seen: HashSet<String> = existing
        .iter()
        .map(|alias| alias.to_ascii_lowercase())
        .collect();
    for alias in incoming {
        let trimmed = alias.trim();
        if trimmed.is_empty() {
            continue;
        }

        let key = trimmed.to_ascii_lowercase();
        if seen.insert(key) {
            existing.push(trimmed.to_string());
        }
    }
}

fn person_role_priority(role: &str) -> u8 {
    match role {
        "attendee" => 3,
        "speaker" => 2,
        "mentioned" => 1,
        _ => 0,
    }
}

fn push_file_person(
    file_people: &mut Vec<(String, String, Vec<String>, &'static str)>,
    slug: String,
    name: String,
    aliases: Vec<String>,
    role: &'static str,
) {
    if slug.is_empty() {
        return;
    }

    if let Some((_, existing_name, existing_aliases, existing_role)) = file_people
        .iter_mut()
        .find(|(existing_slug, _, _, _)| *existing_slug == slug)
    {
        if name.trim().len() > existing_name.trim().len() {
            *existing_name = name;
        }
        merge_person_aliases(existing_aliases, &aliases);
        if person_role_priority(role) > person_role_priority(existing_role) {
            *existing_role = role;
        }
        return;
    }

    file_people.push((slug, name, aliases, role));
}

#[cfg(all(test, not(unix)))]
fn set_db_permissions(_path: &Path) {}

/// Calculate relationship score from meeting count, recency, and topic depth.
#[cfg(test)]
fn relationship_score(meeting_count: i64, days_since: f64, topic_count: usize) -> f64 {
    let recency_weight = 1.0 / (1.0 + days_since / 30.0);
    let topic_depth = (topic_count as f64 / 3.0).min(1.0);
    meeting_count as f64 * recency_weight * topic_depth
}

fn actionable_commitment_status(
    status: &str,
    due_date: Option<&str>,
    now: chrono::DateTime<Local>,
) -> Option<&'static str> {
    if status == "stale" {
        return Some("stale");
    }
    if status != "open" {
        return None;
    }
    let Some(due_date) = due_date else {
        return Some("open");
    };
    let overdue = chrono::NaiveDate::parse_from_str(due_date, "%Y-%m-%d")
        .map(|due| now.date_naive() > due)
        .or_else(|_| {
            chrono::DateTime::parse_from_rfc3339(due_date)
                .map(|due| now > due.with_timezone(&Local))
        })
        .unwrap_or(false);
    Some(if overdue { "stale" } else { "open" })
}

/// Open or create the SQLite database with schema.
#[cfg(test)]
fn open_db(path: &Path) -> Result<Connection, GraphError> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA temp_store=MEMORY;
         PRAGMA page_size=4096;
         PRAGMA max_page_count=16384;
         PRAGMA cache_size=-8192;
         PRAGMA temp.page_size=4096;
         PRAGMA temp.max_page_count=8192;
         PRAGMA temp.cache_size=-4096;
         PRAGMA threads=0;
         PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;",
    )?;
    configure_and_verify_sqlite_bounds(&conn)?;
    create_schema(&conn)?;
    Ok(conn)
}

fn open_memory_db() -> Result<Connection, GraphError> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(
        "PRAGMA temp_store=MEMORY;
         PRAGMA page_size=4096;
         PRAGMA max_page_count=16384;
         PRAGMA cache_size=-8192;
         PRAGMA temp.page_size=4096;
         PRAGMA temp.max_page_count=8192;
         PRAGMA temp.cache_size=-4096;
         PRAGMA threads=0;
         PRAGMA journal_mode=MEMORY;
         PRAGMA synchronous=OFF;",
    )?;
    configure_and_verify_sqlite_bounds(&conn)?;
    create_schema(&conn)?;
    Ok(conn)
}

fn configure_and_verify_sqlite_bounds(conn: &Connection) -> Result<(), GraphError> {
    let limits = [
        (Limit::SQLITE_LIMIT_LENGTH, MAX_GRAPH_SQLITE_VALUE_BYTES),
        (Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_GRAPH_SQLITE_SQL_BYTES),
        (Limit::SQLITE_LIMIT_COLUMN, MAX_GRAPH_SQLITE_COLUMNS),
        (Limit::SQLITE_LIMIT_EXPR_DEPTH, MAX_GRAPH_SQLITE_EXPR_DEPTH),
        (
            Limit::SQLITE_LIMIT_COMPOUND_SELECT,
            MAX_GRAPH_SQLITE_COMPOUND_TERMS,
        ),
        (Limit::SQLITE_LIMIT_VDBE_OP, MAX_GRAPH_SQLITE_VDBE_OPS),
        (
            Limit::SQLITE_LIMIT_FUNCTION_ARG,
            MAX_GRAPH_SQLITE_FUNCTION_ARGS,
        ),
        (Limit::SQLITE_LIMIT_ATTACHED, 0),
        (
            Limit::SQLITE_LIMIT_LIKE_PATTERN_LENGTH,
            MAX_GRAPH_SQLITE_LIKE_PATTERN_BYTES,
        ),
        (
            Limit::SQLITE_LIMIT_VARIABLE_NUMBER,
            MAX_GRAPH_SQLITE_VARIABLES,
        ),
        (Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 0),
        (Limit::SQLITE_LIMIT_WORKER_THREADS, 0),
    ];
    for (limit, value) in limits {
        conn.set_limit(limit, value)?;
        if conn.limit(limit)? != value {
            return Err(GraphError::Io(std::io::Error::other(
                "SQLite refused the bounded graph connection policy",
            )));
        }
    }

    let mode: i64 = conn.query_row("PRAGMA temp_store", [], |row| row.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    let max_page_count: i64 = conn.query_row("PRAGMA max_page_count", [], |row| row.get(0))?;
    let cache_size: i64 = conn.query_row("PRAGMA cache_size", [], |row| row.get(0))?;
    let temp_page_size: i64 = conn.query_row("PRAGMA temp.page_size", [], |row| row.get(0))?;
    let temp_max_page_count: i64 =
        conn.query_row("PRAGMA temp.max_page_count", [], |row| row.get(0))?;
    let temp_cache_size: i64 = conn.query_row("PRAGMA temp.cache_size", [], |row| row.get(0))?;
    let worker_threads: i64 = conn.query_row("PRAGMA threads", [], |row| row.get(0))?;
    if mode != 2
        || page_size != GRAPH_SQLITE_PAGE_BYTES
        || max_page_count != MAX_GRAPH_SQLITE_PAGE_COUNT
        || cache_size != -GRAPH_SQLITE_CACHE_KIB
        || temp_page_size != GRAPH_SQLITE_PAGE_BYTES
        || temp_max_page_count != MAX_GRAPH_SQLITE_TEMP_PAGE_COUNT
        || temp_cache_size != -GRAPH_SQLITE_TEMP_CACHE_KIB
        || worker_threads != 0
    {
        return Err(GraphError::Io(std::io::Error::other(
            "SQLite refused the bounded memory-only graph policy",
        )));
    }
    Ok(())
}

fn create_schema(conn: &Connection) -> Result<(), GraphError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS graph_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS people (
            id INTEGER PRIMARY KEY,
            slug TEXT UNIQUE NOT NULL,
            name TEXT NOT NULL,
            aliases TEXT DEFAULT '[]',
            first_seen TEXT NOT NULL,
            last_seen TEXT NOT NULL,
            meeting_count INTEGER DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS meetings (
            id INTEGER PRIMARY KEY,
            path TEXT UNIQUE NOT NULL,
            title TEXT NOT NULL,
            date TEXT NOT NULL,
            duration_secs INTEGER,
            content_type TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS people_meetings (
            person_id INTEGER REFERENCES people(id),
            meeting_id INTEGER REFERENCES meetings(id),
            role TEXT DEFAULT 'attendee',
            PRIMARY KEY (person_id, meeting_id)
        );
        CREATE TABLE IF NOT EXISTS commitments (
            id INTEGER PRIMARY KEY,
            meeting_id INTEGER REFERENCES meetings(id),
            person_id INTEGER,
            text TEXT NOT NULL,
            status TEXT DEFAULT 'open',
            due_date TEXT,
            created_at TEXT NOT NULL,
            commitment_type TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS decisions (
            id INTEGER PRIMARY KEY,
            meeting_id INTEGER REFERENCES meetings(id),
            text TEXT NOT NULL,
            topic TEXT,
            authority TEXT
        );
        CREATE TABLE IF NOT EXISTS topics (
            id INTEGER PRIMARY KEY,
            name TEXT UNIQUE NOT NULL
        );
        CREATE TABLE IF NOT EXISTS meeting_topics (
            meeting_id INTEGER REFERENCES meetings(id),
            topic_id INTEGER REFERENCES topics(id),
            PRIMARY KEY (meeting_id, topic_id)
        );
        CREATE INDEX IF NOT EXISTS idx_people_slug ON people(slug);
        CREATE INDEX IF NOT EXISTS idx_people_last_seen ON people(last_seen);
        CREATE INDEX IF NOT EXISTS idx_meetings_date ON meetings(date);
        CREATE INDEX IF NOT EXISTS idx_commitments_status ON commitments(status);
        CREATE INDEX IF NOT EXISTS idx_commitments_person ON commitments(person_id);
        CREATE INDEX IF NOT EXISTS idx_decisions_meeting ON decisions(meeting_id);",
    )?;
    Ok(())
}

const GRAPH_CORPUS_ROOT_KEY: &str = "corpus_root";
const GRAPH_CORPUS_REVISION_KEY: &str = "corpus_revision_sha256_v1";
const GRAPH_CORPUS_REVISION_DOMAIN: &[u8] = b"minutes.graph.corpus-revision.v1\0";
const GRAPH_CORRECTION_REVISION_KEY: &str = "correction_revision_sha256_v1";
const GRAPH_CORRECTION_REVISION_DOMAIN: &[u8] = b"minutes.graph.corrections.v1\0";
const GRAPH_VOCABULARY_REVISION_DOMAIN: &[u8] = b"minutes.graph.vocabulary.v1\0";

#[derive(Debug, Clone)]
struct GraphCorrectionPaths {
    vocabulary: PathBuf,
    overlays: PathBuf,
}

impl GraphCorrectionPaths {
    fn production() -> Self {
        Self {
            vocabulary: crate::vocabulary::default_path(),
            overlays: overlays::default_db_path(),
        }
    }

    #[cfg(test)]
    fn beside_graph(path: &Path) -> Self {
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        Self {
            vocabulary: parent.join("vocabulary.toml"),
            overlays: parent.join("overlays.db"),
        }
    }
}

/// One ordered event stream spanning every input that can change graph
/// meaning. Independent before/after hashes cannot prove that corpus revision
/// N and correction revision K ever coexisted: an attacker can alternate the
/// two namespaces between reads. This journal establishes one ordering
/// boundary before materialization and retains it until the answer has been
/// serialized by the supervised worker.
#[cfg(any(not(target_os = "macos"), test))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
enum GraphJournalEvent {
    Paths(Vec<PathBuf>),
    Overflow,
}

#[cfg(all(not(windows), not(target_os = "macos")))]
struct GraphNativeWatchers {
    _watcher: RecommendedWatcher,
}

#[cfg(target_os = "macos")]
struct GraphNativeWatchers {
    journal: macos_graph_journal::MacGraphJournal,
}

#[cfg(windows)]
struct GraphNativeWatchers {
    _watchers: Vec<windows_graph_journal::WindowsDirectoryJournal>,
}

struct GraphSnapshotJournal {
    _watchers: GraphNativeWatchers,
    #[cfg(not(target_os = "macos"))]
    events: mpsc::Receiver<GraphJournalEvent>,
    #[cfg(not(target_os = "macos"))]
    fence_directories: Vec<crate::policy_fs::BoundRecoveryDirectory>,
    #[cfg(not(target_os = "macos"))]
    corpus_root: PathBuf,
    #[cfg(not(target_os = "macos"))]
    correction_root: PathBuf,
    #[cfg(not(target_os = "macos"))]
    vocabulary_path: PathBuf,
    #[cfg(not(target_os = "macos"))]
    overlays_path: PathBuf,
    #[cfg(not(target_os = "macos"))]
    dirty: bool,
    deadline: std::time::Instant,
}

impl GraphSnapshotJournal {
    fn begin(
        corpus_root: &Path,
        corrections: &GraphCorrectionPaths,
        deadline: std::time::Instant,
    ) -> Result<Self, GraphError> {
        let correction_root = corrections
            .vocabulary
            .parent()
            .filter(|parent| corrections.overlays.parent() == Some(*parent))
            .ok_or_else(|| {
                GraphError::Io(std::io::Error::other(
                    "graph corrections do not share one ordered namespace",
                ))
            })?
            .to_path_buf();
        overlays::secure_private_parent(&correction_root).map_err(|_| {
            GraphError::Io(std::io::Error::other(
                "graph correction namespace could not be bound privately",
            ))
        })?;
        #[cfg(not(target_os = "macos"))]
        let fence_directories = {
            let corpus_fence_root = corpus_root
                .join(".minutes-private-projection")
                .join(GRAPH_SNAPSHOT_FENCE_DIRECTORY);
            let correction_fence_root = correction_root.join(GRAPH_SNAPSHOT_FENCE_DIRECTORY);
            let mut directories = Vec::with_capacity(2);
            for fence_root in [&corpus_fence_root, &correction_fence_root] {
                directories.push(
                    crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(fence_root)
                        .map_err(|_| {
                            GraphError::Io(std::io::Error::other(
                                "graph snapshot fence namespace could not be prepared",
                            ))
                        })?,
                );
            }
            directories
        };

        #[cfg(not(target_os = "macos"))]
        let (event_tx, events) = mpsc::channel();
        #[cfg(not(target_os = "macos"))]
        let watchers = install_graph_snapshot_watches(event_tx, corpus_root, &correction_root)?;
        #[cfg(target_os = "macos")]
        let watchers = install_graph_snapshot_watches(
            corpus_root,
            &correction_root,
            &corrections.vocabulary,
            &corrections.overlays,
        )?;

        let mut journal = Self {
            _watchers: watchers,
            #[cfg(not(target_os = "macos"))]
            events,
            #[cfg(not(target_os = "macos"))]
            fence_directories,
            #[cfg(not(target_os = "macos"))]
            corpus_root: lexical_absolute_path(corpus_root),
            #[cfg(not(target_os = "macos"))]
            correction_root: lexical_absolute_path(&correction_root),
            #[cfg(not(target_os = "macos"))]
            vocabulary_path: lexical_absolute_path(&corrections.vocabulary),
            #[cfg(not(target_os = "macos"))]
            overlays_path: lexical_absolute_path(&corrections.overlays),
            #[cfg(not(target_os = "macos"))]
            dirty: false,
            deadline,
        };
        journal.checkpoint("ready")?;
        Ok(journal)
    }

    /// Drain the single native event stream through an exact, unpredictable
    /// capability-backed fence. Rescan/overflow and any corpus/correction
    /// mutation fail closed; the worker may retry from a brand-new journal.
    fn checkpoint(&mut self, label: &str) -> Result<(), GraphError> {
        #[cfg(target_os = "macos")]
        {
            if std::time::Instant::now() >= self.deadline {
                return Err(graph_corpus_budget_error());
            }
            if self._watchers.journal.changed().map_err(GraphError::Io)? {
                return Err(GraphError::Io(std::io::Error::other(
                    "graph corpus or corrections changed during one ordered snapshot",
                )));
            }
            let _ = label;
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            if std::time::Instant::now() >= self.deadline {
                return Err(graph_corpus_budget_error());
            }
            let mut fences = Vec::with_capacity(self.fence_directories.len());
            for (domain, directory) in self.fence_directories.iter().enumerate() {
                let fence = directory
                    .create_random_private_control_file(
                        &format!("graph-{domain}-{label}-{}", std::process::id()),
                        GRAPH_SNAPSHOT_FENCE_BYTES,
                    )
                    .map_err(|_| {
                        GraphError::Io(std::io::Error::other(
                            "graph snapshot fence could not be published",
                        ))
                    })?;
                let path = lexical_absolute_path(fence.display_path());
                fences.push((directory, fence, path));
            }
            let wait_deadline = self
                .deadline
                .min(std::time::Instant::now() + GRAPH_SNAPSHOT_FENCE_TIMEOUT);
            let mut observed = vec![false; fences.len()];
            while observed.iter().any(|seen| !seen) {
                let remaining = wait_deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    self.dirty = true;
                    break;
                }
                match self.events.recv_timeout(remaining) {
                    Ok(GraphJournalEvent::Paths(paths)) => {
                        for path in &paths {
                            let path = lexical_absolute_path(path);
                            for (index, (_, _, fence_path)) in fences.iter().enumerate() {
                                if path == *fence_path {
                                    observed[index] = true;
                                }
                            }
                        }
                        if graph_snapshot_event_affects_inputs(
                            &paths,
                            &self.corpus_root,
                            &self.correction_root,
                            &self.vocabulary_path,
                            &self.overlays_path,
                            &self
                                .fence_directories
                                .iter()
                                .map(|directory| directory.display_path())
                                .collect::<Vec<_>>(),
                        ) {
                            #[cfg(test)]
                            eprintln!(
                                "graph snapshot journal marked {label:?} dirty from paths: {paths:?}"
                            );
                            self.dirty = true;
                        }
                    }
                    Ok(GraphJournalEvent::Overflow) | Err(_) => {
                        #[cfg(test)]
                        eprintln!(
                            "graph snapshot journal marked {label:?} dirty from overflow/disconnect"
                        );
                        self.dirty = true;
                        break;
                    }
                }
            }
            let mut retired = true;
            for (directory, fence, _) in fences {
                if directory.remove_owned_private_file(fence).is_err() {
                    retired = false;
                }
            }
            #[cfg(test)]
            if observed.iter().any(|seen| !seen) || !retired || self.dirty {
                eprintln!(
                    "graph snapshot journal checkpoint {label:?} failed: observed={observed:?} retired={retired} dirty={}",
                    self.dirty
                );
            }
            if observed.iter().any(|seen| !seen) || !retired || self.dirty {
                return Err(GraphError::Io(std::io::Error::other(
                    "graph corpus or corrections changed during one ordered snapshot",
                )));
            }
            Ok(())
        }
    }
}

#[cfg(any(not(target_os = "macos"), test))]
fn lexical_absolute_path(path: &Path) -> PathBuf {
    use std::path::Component;

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

#[cfg(any(not(target_os = "macos"), test))]
fn path_touches_namespace(path: &Path, namespace: &Path) -> bool {
    path == namespace || path.starts_with(namespace) || namespace.starts_with(path)
}

#[cfg(any(not(target_os = "macos"), test))]
fn graph_snapshot_event_affects_inputs(
    paths: &[PathBuf],
    corpus_root: &Path,
    correction_root: &Path,
    vocabulary_path: &Path,
    overlays_path: &Path,
    fence_roots: &[&Path],
) -> bool {
    let fence_roots = fence_roots
        .iter()
        .map(|root| lexical_absolute_path(root))
        .collect::<Vec<_>>();
    paths.iter().any(|path| {
        let path = lexical_absolute_path(path);
        if fence_roots
            .iter()
            .any(|root| path == *root || path.starts_with(root))
        {
            return false;
        }
        path_touches_namespace(&path, corpus_root)
            || path == correction_root
            || path_touches_namespace(&path, vocabulary_path)
            || path_touches_sqlite_correction(&path, overlays_path)
    })
}

#[cfg(any(not(target_os = "macos"), test))]
fn path_touches_sqlite_correction(path: &Path, database: &Path) -> bool {
    if path_touches_namespace(path, database) {
        return true;
    }
    let Some(database_name) = database.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    path.parent() == database.parent()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                matches!(
                    name.strip_prefix(database_name),
                    Some("-wal" | "-shm" | "-journal")
                )
            })
}

#[cfg(any(test, all(not(windows), not(target_os = "macos"))))]
fn classify_graph_notify_event(event: notify::Result<notify::Event>) -> Option<GraphJournalEvent> {
    match event {
        Ok(event) if event.need_rescan() => Some(GraphJournalEvent::Overflow),
        Ok(event)
            if matches!(
                event.kind,
                NotifyEventKind::Access(_)
                    | NotifyEventKind::Modify(notify::event::ModifyKind::Metadata(_))
            ) =>
        {
            None
        }
        Ok(event) => Some(GraphJournalEvent::Paths(event.paths)),
        Err(_) => Some(GraphJournalEvent::Overflow),
    }
}

#[cfg(any(test, all(not(windows), not(target_os = "macos"))))]
fn non_windows_graph_watch_specs(
    corpus_root: &Path,
    correction_root: &Path,
) -> Vec<(PathBuf, RecursiveMode)> {
    let corpus_root = lexical_absolute_path(corpus_root);
    let correction_root = lexical_absolute_path(correction_root);
    let mut watches = vec![
        (corpus_root.clone(), RecursiveMode::Recursive),
        (correction_root.clone(), RecursiveMode::Recursive),
    ];
    for root in [&corpus_root, &correction_root] {
        for ancestor in root.ancestors().skip(1) {
            watches.push((ancestor.to_path_buf(), RecursiveMode::NonRecursive));
        }
    }
    watches
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn install_graph_snapshot_watches(
    event_tx: mpsc::Sender<GraphJournalEvent>,
    corpus_root: &Path,
    correction_root: &Path,
) -> Result<GraphNativeWatchers, GraphError> {
    let mut watcher = RecommendedWatcher::new(
        move |event: notify::Result<notify::Event>| {
            if let Some(event) = classify_graph_notify_event(event) {
                let _ = event_tx.send(event);
            }
        },
        NotifyConfig::default(),
    )
    .map_err(|_| {
        GraphError::Io(std::io::Error::other(
            "graph snapshot journal could not be started",
        ))
    })?;
    let watches = non_windows_graph_watch_specs(corpus_root, correction_root);
    // A recursive watch stays attached to the original inode when any higher
    // ancestor is renamed. Cover every ancestor through the filesystem anchor
    // so a nested configurable corpus/correction root cannot be swapped out,
    // read through its replacement, and restored before the final fence.
    // Windows does not need these watches: the retained fence-directory
    // capability chains deny FILE_SHARE_DELETE on every ancestor there.
    let mut installed = HashSet::new();
    for (path, mode) in watches {
        let key = (path.clone(), matches!(mode, RecursiveMode::Recursive));
        if !installed.insert(key) {
            continue;
        }
        watcher.watch(&path, mode).map_err(|_| {
            GraphError::Io(std::io::Error::other(
                "graph snapshot journal could not cover every input namespace",
            ))
        })?;
    }
    Ok(GraphNativeWatchers { _watcher: watcher })
}

#[cfg(target_os = "macos")]
fn install_graph_snapshot_watches(
    corpus_root: &Path,
    correction_root: &Path,
    vocabulary_path: &Path,
    overlays_path: &Path,
) -> Result<GraphNativeWatchers, GraphError> {
    let journal = macos_graph_journal::MacGraphJournal::start(
        corpus_root,
        correction_root,
        vocabulary_path,
        overlays_path,
    )
    .map_err(GraphError::Io)?;
    Ok(GraphNativeWatchers { journal })
}

#[cfg(target_os = "macos")]
#[path = "graph/macos_kqueue.rs"]
mod macos_graph_journal;

#[cfg(windows)]
fn install_graph_snapshot_watches(
    event_tx: mpsc::Sender<GraphJournalEvent>,
    corpus_root: &Path,
    correction_root: &Path,
) -> Result<GraphNativeWatchers, GraphError> {
    let mut watchers = Vec::with_capacity(2);
    for root in [corpus_root, correction_root] {
        watchers.push(
            windows_graph_journal::WindowsDirectoryJournal::start(
                &lexical_absolute_path(root),
                event_tx.clone(),
            )
            .map_err(|_| {
                GraphError::Io(std::io::Error::other(
                    "loss-aware Windows graph journal could not be established",
                ))
            })?,
        );
    }
    Ok(GraphNativeWatchers {
        _watchers: watchers,
    })
}

#[cfg(windows)]
mod windows_graph_journal {
    use super::GraphJournalEvent;
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Component, Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, INVALID_HANDLE_VALUE,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
        FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
        FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

    const JOURNAL_BUFFER_BYTES: usize = 64 * 1024;
    const FILE_NOTIFY_HEADER_BYTES: usize = 12;
    const POLL_MILLIS: u32 = 100;

    pub(super) struct WindowsDirectoryJournal {
        stop: Arc<AtomicBool>,
        directory: usize,
        event: usize,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl WindowsDirectoryJournal {
        pub(super) fn start(
            root: &Path,
            events: mpsc::Sender<GraphJournalEvent>,
        ) -> std::io::Result<Self> {
            let mut root_wide = root.as_os_str().encode_wide().collect::<Vec<_>>();
            root_wide.push(0);
            let directory = unsafe {
                CreateFileW(
                    root_wide.as_ptr(),
                    FILE_LIST_DIRECTORY,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                    std::ptr::null_mut(),
                )
            };
            if directory == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
            if event.is_null() {
                unsafe {
                    CloseHandle(directory);
                }
                return Err(std::io::Error::last_os_error());
            }

            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = Arc::clone(&stop);
            let worker_root = root.to_path_buf();
            let directory_bits = directory as usize;
            let event_bits = event as usize;
            let worker = std::thread::Builder::new()
                .name("minutes-graph-journal".to_string())
                .spawn(move || {
                    run_directory_journal(
                        &worker_root,
                        directory_bits,
                        event_bits,
                        &worker_stop,
                        &events,
                    );
                });
            let worker = match worker {
                Ok(worker) => worker,
                Err(error) => {
                    unsafe {
                        CloseHandle(event);
                        CloseHandle(directory);
                    }
                    return Err(error);
                }
            };
            Ok(Self {
                stop,
                directory: directory_bits,
                event: event_bits,
                worker: Some(worker),
            })
        }
    }

    impl Drop for WindowsDirectoryJournal {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            unsafe {
                CancelIoEx(self.directory as _, std::ptr::null());
            }
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            unsafe {
                CloseHandle(self.event as _);
                CloseHandle(self.directory as _);
            }
        }
    }

    fn run_directory_journal(
        root: &Path,
        directory_bits: usize,
        event_bits: usize,
        stop: &AtomicBool,
        events: &mpsc::Sender<GraphJournalEvent>,
    ) {
        let directory = directory_bits as _;
        let event = event_bits as _;
        // Graph meaning is derived from namespace and byte changes. Match the
        // non-Windows classifier, which deliberately ignores access and
        // metadata-only events: retained capability checks independently
        // enforce identity and reachability, while subscribing to Windows
        // security/attribute/creation metadata makes our own handle/DACL
        // attestations look like source-byte mutations.
        let notify_filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_SIZE
            | FILE_NOTIFY_CHANGE_LAST_WRITE;
        let mut buffer = vec![0u8; JOURNAL_BUFFER_BYTES];
        while !stop.load(Ordering::Acquire) {
            let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
            overlapped.hEvent = event;
            let started = unsafe {
                ReadDirectoryChangesW(
                    directory,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    1,
                    notify_filter,
                    std::ptr::null_mut(),
                    &mut overlapped,
                    None,
                )
            };
            if started == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
                let _ = events.send(GraphJournalEvent::Overflow);
                return;
            }

            loop {
                if stop.load(Ordering::Acquire) {
                    unsafe {
                        CancelIoEx(directory, &overlapped);
                    }
                }
                match unsafe { WaitForSingleObject(event, POLL_MILLIS) } {
                    WAIT_OBJECT_0 => break,
                    WAIT_TIMEOUT => continue,
                    _ => {
                        let _ = events.send(GraphJournalEvent::Overflow);
                        return;
                    }
                }
            }

            let mut transferred = 0u32;
            if unsafe { GetOverlappedResult(directory, &overlapped, &mut transferred, 0) } == 0 {
                let error = unsafe { GetLastError() };
                if stop.load(Ordering::Acquire) && error == ERROR_OPERATION_ABORTED {
                    return;
                }
                let _ = events.send(GraphJournalEvent::Overflow);
                return;
            }
            if stop.load(Ordering::Acquire) {
                return;
            }
            let Some(paths) = parse_notifications(root, &buffer, transferred as usize) else {
                let _ = events.send(GraphJournalEvent::Overflow);
                return;
            };
            if events.send(GraphJournalEvent::Paths(paths)).is_err() {
                return;
            }
        }
    }

    fn parse_notifications(root: &Path, buffer: &[u8], transferred: usize) -> Option<Vec<PathBuf>> {
        if transferred == 0 || transferred > buffer.len() {
            return None;
        }
        let mut paths = Vec::new();
        let mut offset = 0usize;
        loop {
            let header_end = offset.checked_add(FILE_NOTIFY_HEADER_BYTES)?;
            if header_end > transferred {
                return None;
            }
            let next = u32::from_le_bytes(buffer[offset..offset + 4].try_into().ok()?) as usize;
            let name_bytes =
                u32::from_le_bytes(buffer[offset + 8..offset + 12].try_into().ok()?) as usize;
            if name_bytes == 0 || !name_bytes.is_multiple_of(2) {
                return None;
            }
            let name_end = header_end.checked_add(name_bytes)?;
            if name_end > transferred {
                return None;
            }
            let wide = buffer[header_end..name_end]
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            let relative = PathBuf::from(OsString::from_wide(&wide));
            if relative.components().any(|component| {
                matches!(
                    component,
                    Component::Prefix(_) | Component::RootDir | Component::ParentDir
                )
            }) {
                return None;
            }
            paths.push(root.join(relative));
            if next == 0 {
                break;
            }
            if next < name_end - offset || !next.is_multiple_of(4) {
                return None;
            }
            offset = offset.checked_add(next)?;
            if offset >= transferred {
                return None;
            }
        }
        Some(paths)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn notification_parser_rejects_overflow_and_truncation() {
            assert!(parse_notifications(Path::new(r"C:\meetings"), &[], 0).is_none());
            let mut truncated = vec![0u8; FILE_NOTIFY_HEADER_BYTES];
            truncated[8..12].copy_from_slice(&8u32.to_le_bytes());
            assert!(
                parse_notifications(Path::new(r"C:\meetings"), &truncated, truncated.len())
                    .is_none()
            );
        }

        #[test]
        fn notification_parser_returns_exact_relative_paths() {
            let name = "person.md".encode_utf16().collect::<Vec<_>>();
            let mut bytes = vec![0u8; FILE_NOTIFY_HEADER_BYTES + name.len() * 2];
            bytes[8..12].copy_from_slice(&((name.len() * 2) as u32).to_le_bytes());
            for (index, value) in name.into_iter().enumerate() {
                let start = FILE_NOTIFY_HEADER_BYTES + index * 2;
                bytes[start..start + 2].copy_from_slice(&value.to_le_bytes());
            }
            assert_eq!(
                parse_notifications(Path::new(r"C:\meetings"), &bytes, bytes.len()).unwrap(),
                vec![PathBuf::from(r"C:\meetings\person.md")]
            );
        }
    }
}

#[derive(Debug, Clone)]
struct GraphCorrectionSnapshot {
    vocabulary_people: Vec<EntityRef>,
    speaker_overlays: overlays::StableSpeakerOverlaySnapshot,
    revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PolicyGraphSpeakerCorrection {
    pub(crate) speaker_label: String,
    pub(crate) name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PolicyGraphStreamSource {
    /// Request-local opaque path. The parent maps this back to the exact
    /// authorized live path only after the final ordered-snapshot fence.
    pub(crate) opaque_path: PathBuf,
    pub(crate) content: String,
    pub(crate) content_sha256: [u8; 32],
    pub(crate) speaker_corrections: Vec<PolicyGraphSpeakerCorrection>,
}

pub(crate) struct PolicyGraphSnapshotPayload {
    pub(crate) sources: Vec<PolicyGraphStreamSource>,
    pub(crate) vocabulary_people: Vec<EntityRef>,
    pub(crate) correction_revision: String,
    pub(crate) opaque_to_live_paths: HashMap<PathBuf, PathBuf>,
}

pub(crate) struct PolicyGraphSnapshotAuthority {
    _admission: GraphProjectionAdmission,
    journal: GraphSnapshotJournal,
    canonical_root: PathBuf,
    correction_paths: GraphCorrectionPaths,
    corpus_revision: String,
    correction_revision: String,
    operation: GraphOperationBudget,
    derived: GraphDerivedBudget,
    deadline: std::time::Instant,
}

fn hash_revision_field(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn correction_aggregate_revision(vocabulary: &str, overlays: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(GRAPH_CORRECTION_REVISION_DOMAIN);
    hash_revision_field(&mut hasher, vocabulary);
    hash_revision_field(&mut hasher, overlays);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_stable_correction_file(
    path: &Path,
    budget: &ActiveCorpusReadBudget,
    deadline: std::time::Instant,
) -> Result<Option<Vec<u8>>, GraphError> {
    let read_deadline = deadline.min(std::time::Instant::now() + CORRECTION_READ_DEADLINE);
    budget
        .check_deadline()
        .map_err(|_| graph_corpus_budget_error())?;
    if std::time::Instant::now() >= read_deadline {
        return Err(graph_corpus_budget_error());
    }
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(GraphError::Io(std::io::Error::other(
                "correction store could not be verified",
            )))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(GraphError::Io(std::io::Error::other(
            "correction store is not a bounded regular file",
        )));
    }
    if metadata.len() > MAX_CORRECTION_FILE_BYTES {
        return Err(GraphError::Io(std::io::Error::other(
            "correction store exceeded its byte budget",
        )));
    }
    let parent = path.parent().ok_or_else(|| {
        GraphError::Io(std::io::Error::other(
            "correction store has no capability root",
        ))
    })?;
    overlays::secure_private_parent(parent).map_err(|_| {
        GraphError::Io(std::io::Error::other(
            "correction store parent could not be verified",
        ))
    })?;
    let snapshot = crate::policy_fs::read_bound_utf8_file(parent, path).map_err(|_| {
        GraphError::Io(std::io::Error::other(
            "correction store could not be read through its retained capability",
        ))
    })?;
    budget
        .check_deadline()
        .map_err(|_| graph_corpus_budget_error())?;
    if std::time::Instant::now() >= read_deadline {
        return Err(graph_corpus_budget_error());
    }
    consume_graph_corpus(budget, 1, 0, snapshot.content.len() as u64)?;
    Ok(Some(snapshot.content.into_bytes()))
}

fn stable_vocabulary_people(
    path: &Path,
    budget: &ActiveCorpusReadBudget,
    deadline: std::time::Instant,
) -> Result<(Vec<EntityRef>, String), GraphError> {
    let bytes = read_stable_correction_file(path, budget, deadline)?;
    let mut hasher = Sha256::new();
    hasher.update(GRAPH_VOCABULARY_REVISION_DOMAIN);
    let store = match bytes {
        None => {
            hasher.update([0]);
            crate::vocabulary::VocabularyStore::empty()
        }
        Some(bytes) => {
            hasher.update([1]);
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
            if bytes.iter().all(u8::is_ascii_whitespace) {
                crate::vocabulary::VocabularyStore::empty()
            } else {
                let raw = std::str::from_utf8(&bytes).map_err(|_| {
                    GraphError::Io(std::io::Error::other(
                        "vocabulary correction store is invalid",
                    ))
                })?;
                toml::from_str::<crate::vocabulary::VocabularyStore>(raw)
                    .map_err(|_| {
                        GraphError::Io(std::io::Error::other(
                            "vocabulary correction store is invalid",
                        ))
                    })?
                    .normalized()
                    .map_err(|_| {
                        GraphError::Io(std::io::Error::other(
                            "vocabulary correction store is invalid",
                        ))
                    })?
            }
        }
    };
    let people = store
        .entries
        .into_iter()
        .filter(|entry| entry.kind == crate::vocabulary::VocabularyKind::Person)
        .filter_map(|entry| {
            let label = entry.canonical.trim();
            let slug = slugify(label);
            (!label.is_empty() && !slug.is_empty()).then(|| EntityRef {
                slug,
                label: label.to_string(),
                aliases: entry.aliases,
            })
        })
        .collect();
    let revision = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((people, revision))
}

fn graph_correction_snapshot(
    paths: &GraphCorrectionPaths,
    budget: &ActiveCorpusReadBudget,
    derived: &mut GraphDerivedBudget,
    deadline: std::time::Instant,
) -> Result<GraphCorrectionSnapshot, GraphError> {
    derived.consume_path(&paths.vocabulary, budget)?;
    derived.consume_path(&paths.overlays, budget)?;
    let (vocabulary_people, vocabulary_revision) =
        stable_vocabulary_people(&paths.vocabulary, budget, deadline)?;
    for entity in &vocabulary_people {
        derived.consume_entity_ref(entity, budget)?;
    }
    if let Ok(metadata) = std::fs::symlink_metadata(&paths.overlays) {
        consume_graph_corpus(budget, 1, 0, metadata.len())?;
    }
    let speaker_overlays =
        overlays::stable_speaker_overlay_snapshot_at_until(&paths.overlays, deadline).map_err(
            |_| {
                GraphError::Io(std::io::Error::other(
                    "speaker corrections could not be verified",
                ))
            },
        )?;
    for confirmation in speaker_overlays.confirmations() {
        derived.consume(2, budget)?;
        derived.consume_entity_text(&confirmation.speaker_label, budget)?;
        derived.consume_entity_text(&confirmation.name, budget)?;
    }
    let revision = correction_aggregate_revision(&vocabulary_revision, speaker_overlays.revision());
    Ok(GraphCorrectionSnapshot {
        vocabulary_people,
        speaker_overlays,
        revision,
    })
}

/// Re-attest source and correction inputs as one aggregate. Bracketing both
/// reads prevents a source flip during correction verification (or vice
/// versa) from producing a pair that never coexisted across the check.
fn graph_inputs_still_attested(
    canonical_root: &Path,
    correction_paths: &GraphCorrectionPaths,
    expected_corpus: &str,
    expected_corrections: &str,
    operation: &mut GraphOperationBudget,
    derived: &mut GraphDerivedBudget,
    deadline: std::time::Instant,
) -> Result<bool, GraphError> {
    let corpus_before_budget = operation.next_pass()?;
    let corpus_before = graph_corpus_revision(canonical_root, &corpus_before_budget, derived)?;
    let corrections_before_budget = operation.next_pass()?;
    let corrections_before = graph_correction_snapshot(
        correction_paths,
        &corrections_before_budget,
        derived,
        deadline,
    );
    let corpus_after_budget = operation.next_pass()?;
    let corpus_after = graph_corpus_revision(canonical_root, &corpus_after_budget, derived)?;
    let corrections_after_budget = operation.next_pass()?;
    let corrections_after = graph_correction_snapshot(
        correction_paths,
        &corrections_after_budget,
        derived,
        deadline,
    );
    Ok(corpus_before == expected_corpus
        && corpus_after == expected_corpus
        && corrections_before
            .as_ref()
            .is_ok_and(|snapshot| snapshot.revision == expected_corrections)
        && corrections_after
            .as_ref()
            .is_ok_and(|snapshot| snapshot.revision == expected_corrections))
}

pub(crate) fn capture_policy_graph_snapshot(
    config: &Config,
) -> Result<(PolicyGraphSnapshotPayload, PolicyGraphSnapshotAuthority), GraphError> {
    let deadline = graph_operation_deadline();
    let canonical_root = config
        .output_dir
        .canonicalize()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                GraphError::DirNotFound(config.output_dir.display().to_string())
            }
            _ => GraphError::Io(error),
        })?;
    let admission = graph_projection_admission(&canonical_root)?;
    let correction_paths = GraphCorrectionPaths::production();
    let mut operation = GraphOperationBudget::new(deadline);
    let mut derived = GraphDerivedBudget::default();

    for attempt in 0..3 {
        let mut journal =
            match GraphSnapshotJournal::begin(&canonical_root, &correction_paths, deadline) {
                Ok(journal) => journal,
                Err(_) if attempt < 2 => continue,
                Err(error) => return Err(error),
            };
        let correction_budget = operation.next_pass()?;
        let corrections = match graph_correction_snapshot(
            &correction_paths,
            &correction_budget,
            &mut derived,
            deadline,
        ) {
            Ok(corrections) => corrections,
            Err(_) if attempt < 2 => continue,
            Err(error) => return Err(error),
        };
        let source_budget = operation.next_pass()?;
        let mut sources = match collect_policy_graph_sources(
            &config.output_dir,
            &canonical_root,
            &corrections,
            &source_budget,
            &mut derived,
            &mut |_| {},
        ) {
            Ok(sources) => sources,
            Err(_) if attempt < 2 => continue,
            Err(error) => return Err(error),
        };

        let mut revision_entries = sources
            .iter()
            .map(|source| (source.opaque_path.clone(), source.content_sha256))
            .collect::<Vec<_>>();
        let corpus_revision = graph_revision_from_entries(&mut revision_entries);
        if journal.checkpoint("captured").is_err() {
            if attempt < 2 {
                continue;
            }
            return Err(GraphError::Io(std::io::Error::other(
                "graph sources or corrections changed during capture",
            )));
        }

        let mut request_nonce = [0_u8; 16];
        getrandom::fill(&mut request_nonce)
            .map_err(|error| GraphError::Io(std::io::Error::other(error.to_string())))?;
        let request_prefix = request_nonce
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut opaque_to_live_paths = HashMap::with_capacity(sources.len());
        for (index, source) in sources.iter_mut().enumerate() {
            let live_path = source.opaque_path.clone();
            let opaque_path = PathBuf::from(format!(
                "/__minutes_graph_source/{request_prefix}/{index:08}.md"
            ));
            source.opaque_path = opaque_path.clone();
            opaque_to_live_paths.insert(opaque_path, live_path);
        }

        let correction_revision = corrections.revision.clone();
        let payload = PolicyGraphSnapshotPayload {
            sources,
            vocabulary_people: corrections.vocabulary_people,
            correction_revision: correction_revision.clone(),
            opaque_to_live_paths,
        };
        let authority = PolicyGraphSnapshotAuthority {
            _admission: admission,
            journal,
            canonical_root,
            correction_paths,
            corpus_revision,
            correction_revision,
            operation,
            derived,
            deadline,
        };
        return Ok((payload, authority));
    }

    Err(GraphError::Io(std::io::Error::other(
        "graph sources or corrections could not be captured",
    )))
}

impl PolicyGraphSnapshotAuthority {
    pub(crate) fn remaining(&self) -> std::time::Duration {
        self.deadline
            .saturating_duration_since(std::time::Instant::now())
    }

    pub(crate) fn finalize(mut self) -> Result<(), GraphError> {
        if !graph_inputs_still_attested(
            &self.canonical_root,
            &self.correction_paths,
            &self.corpus_revision,
            &self.correction_revision,
            &mut self.operation,
            &mut self.derived,
            self.deadline,
        )? {
            return Err(GraphError::Io(std::io::Error::other(
                "graph sources or corrections changed before publication",
            )));
        }
        self.journal.checkpoint("published")
    }
}

pub(crate) fn rehydrate_policy_projection_paths(
    response: &mut PolicyProjectionResponse,
    opaque_to_live_paths: &HashMap<PathBuf, PathBuf>,
) -> Result<(), GraphError> {
    let rehydrate = |path: &mut PathBuf| {
        let live = opaque_to_live_paths.get(path).ok_or_else(|| {
            GraphError::Io(std::io::Error::other(
                "graph worker returned an unknown source identifier",
            ))
        })?;
        *path = live.clone();
        Ok::<(), GraphError>(())
    };
    if let PolicyProjectionResponse::PersonProfile(profile) = response {
        for meeting in &mut profile.recent_meetings {
            rehydrate(&mut meeting.path)?;
        }
        for intent in &mut profile.open_intents {
            rehydrate(&mut intent.path)?;
        }
        for decision in &mut profile.recent_decisions {
            rehydrate(&mut decision.path)?;
        }
    }
    Ok(())
}

/// Read one source as an immutable policy snapshot. The returned bytes and
/// revision come from the same descriptor, and the live pathname must still
/// identify that descriptor after the read. Invalid, unreadable, symlinked,
/// outside-root, restricted, and policy-uncertain files are all absent from
/// the eligible graph corpus.
fn read_policy_graph_source(
    path: &Path,
    canonical_root: &Path,
    budget: &ActiveCorpusReadBudget,
) -> Option<StableMarkdownSnapshot> {
    let source = read_stable_active_markdown_with_budget(path, canonical_root, budget)?;
    let (frontmatter_yaml, _) = split_frontmatter(&source.content);
    if frontmatter_yaml.is_empty() {
        return None;
    }
    let frontmatter = parse_graph_frontmatter(frontmatter_yaml)?;
    if matches!(frontmatter.sensitivity, Some(Sensitivity::Restricted)) {
        return None;
    }

    Some(source)
}

fn graph_revision_from_entries(entries: &mut [(PathBuf, [u8; 32])]) -> String {
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    hasher.update(GRAPH_CORPUS_REVISION_DOMAIN);
    for (path, content_sha256) in entries {
        let path = path
            .to_str()
            .expect("policy graph sources reject non-UTF-8 paths");
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update(content_sha256);
    }
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn graph_corpus_revision_with_budget_and_hook(
    canonical_root: &Path,
    budget: &ActiveCorpusReadBudget,
    derived: &mut GraphDerivedBudget,
    after_source_verified: &mut impl FnMut(&Path),
) -> Result<String, GraphError> {
    let walker = WalkDir::new(canonical_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !is_inactive_corpus_dir_name(entry.file_name())
        });
    let mut entries = Vec::new();
    let mut retained_path_bytes = 0usize;
    for entry in walker {
        budget
            .check_deadline()
            .map_err(|_| graph_corpus_budget_error())?;
        let entry = entry.map_err(|_| {
            GraphError::Io(std::io::Error::other(
                "graph corpus traversal could not be verified",
            ))
        })?;
        derived.consume(1, budget)?;
        derived.consume_path(entry.path(), budget)?;
        if entry.file_type().is_dir() {
            consume_graph_corpus(budget, 0, 1, 0)?;
            continue;
        }
        consume_graph_corpus(budget, 1, 0, 0)?;
        if !entry.file_type().is_file()
            || entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("md")
        {
            continue;
        }
        let source = read_policy_graph_source(entry.path(), canonical_root, budget);
        budget
            .check_deadline()
            .map_err(|_| graph_corpus_budget_error())?;
        if let Some(source) = source {
            consume_graph_corpus(budget, 0, 0, source.content.len() as u64)?;
            retained_path_bytes = retained_path_bytes
                .checked_add(source.path.as_os_str().len())
                .ok_or_else(|| {
                    GraphError::Io(std::io::Error::other(
                        "graph revision retained-path budget overflowed",
                    ))
                })?;
            if retained_path_bytes > MAX_GRAPH_RETAINED_PATH_BYTES {
                return Err(GraphError::Io(std::io::Error::other(
                    "graph revision retained-path budget exceeded",
                )));
            }
            entries.push((source.path.clone(), source.content_sha256));
            after_source_verified(&source.path);
        }
    }
    budget
        .check_deadline()
        .map_err(|_| graph_corpus_budget_error())?;
    Ok(graph_revision_from_entries(&mut entries))
}

fn graph_corpus_revision(
    canonical_root: &Path,
    budget: &ActiveCorpusReadBudget,
    derived: &mut GraphDerivedBudget,
) -> Result<String, GraphError> {
    graph_corpus_revision_with_budget_and_hook(canonical_root, budget, derived, &mut |_| {})
}

#[cfg(test)]
fn graph_metadata_value(conn: &Connection, key: &str) -> Result<Option<String>, GraphError> {
    match conn.query_row(
        "SELECT value FROM graph_metadata WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    ) {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Materialize one graph answer only while the derived database and two live
/// corpus snapshots agree on the exact eligible path+byte set. The pre-query
/// snapshot prevents a stale cache read; the post-query snapshot prevents a
/// normal-to-restricted flip during SQL materialization from escaping. No raw
/// `Connection` leaves this boundary.
#[cfg(test)]
fn query_policy_fresh_graph_at_with_publication<T>(
    config: &Config,
    path: &Path,
    mut query: impl FnMut(&Connection) -> Result<T, GraphError>,
    mut after_source_verified: impl FnMut(&Path),
    mut before_query: impl FnMut(),
    mut publish_while_attested: impl FnMut(&T, &mut GraphSnapshotJournal) -> Result<(), GraphError>,
) -> Result<T, GraphError> {
    let deadline = graph_operation_deadline();
    let mut operation = GraphOperationBudget::new(deadline);
    let mut derived = GraphDerivedBudget::default();
    // The retirement transaction has its own focused tests. Graph unit tests
    // use per-test correction paths and must not race through the process-wide
    // HOME namespace when the Rust harness runs them in parallel.
    #[cfg(not(test))]
    crate::policy_fs::retire_legacy_policy_caches()?;
    let canonical_root = config
        .output_dir
        .canonicalize()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                GraphError::DirNotFound(config.output_dir.display().to_string())
            }
            _ => GraphError::Io(error),
        })?;
    let _admission = graph_projection_admission(&canonical_root)?;

    #[cfg(test)]
    let correction_paths = if path.as_os_str().is_empty() {
        GraphCorrectionPaths::production()
    } else {
        GraphCorrectionPaths::beside_graph(path)
    };
    #[cfg(not(test))]
    let correction_paths = {
        let _ = path;
        GraphCorrectionPaths::production()
    };
    for attempt in 0..3 {
        let mut journal =
            match GraphSnapshotJournal::begin(&canonical_root, &correction_paths, deadline) {
                Ok(journal) => journal,
                Err(_) if attempt < 2 => continue,
                Err(error) => return Err(error),
            };
        // Build a private one-query projection from stable live Markdown plus
        // one explicit, immutable correction snapshot. The builder never
        // rediscovers correction paths beside its temporary SQLite file.
        let correction_budget = operation.next_pass()?;
        let corrections = match graph_correction_snapshot(
            &correction_paths,
            &correction_budget,
            &mut derived,
            deadline,
        ) {
            Ok(corrections) => corrections,
            Err(_) if attempt < 2 => continue,
            Err(error) => return Err(error),
        };
        let projection_budget = operation.next_pass()?;
        let rebuilt = rebuild_in_memory_projection_with_hook(
            config,
            &corrections,
            &projection_budget,
            &mut derived,
            &mut after_source_verified,
            false,
        );
        let (conn, _) = match rebuilt {
            Ok(rebuilt) => rebuilt,
            Err(_) if attempt < 2 => continue,
            Err(error) => return Err(error),
        };
        conn.execute_batch("BEGIN")?;

        let trusted_revision =
            graph_metadata_value(&conn, GRAPH_CORPUS_REVISION_KEY)?.ok_or_else(|| {
                GraphError::Io(std::io::Error::other("graph projection is unattested"))
            })?;
        let trusted_correction_revision =
            graph_metadata_value(&conn, GRAPH_CORRECTION_REVISION_KEY)?.ok_or_else(|| {
                GraphError::Io(std::io::Error::other("graph projection is unattested"))
            })?;
        before_query();
        let value = query(&conn)?;
        projection_budget
            .check_deadline()
            .map_err(|_| graph_corpus_budget_error())?;
        if graph_inputs_still_attested(
            &canonical_root,
            &correction_paths,
            &trusted_revision,
            &trusted_correction_revision,
            &mut operation,
            &mut derived,
            deadline,
        )? && journal.checkpoint("materialized").is_ok()
        {
            publish_while_attested(&value, &mut journal)?;
            return Ok(value);
        }

        conn.execute_batch("ROLLBACK")?;
    }

    Err(GraphError::Io(std::io::Error::other(
        "graph sources or corrections changed while materializing a policy-fresh read",
    )))
}

#[cfg(test)]
fn query_policy_fresh_graph_at_with_hooks<T>(
    config: &Config,
    path: &Path,
    query: impl FnMut(&Connection) -> Result<T, GraphError>,
    after_source_verified: impl FnMut(&Path),
    before_query: impl FnMut(),
) -> Result<T, GraphError> {
    query_policy_fresh_graph_at_with_publication(
        config,
        path,
        query,
        after_source_verified,
        before_query,
        |_, _| Ok(()),
    )
}

#[cfg(test)]
fn query_policy_fresh_graph_at<T>(
    config: &Config,
    path: &Path,
    query: impl FnMut(&Connection) -> Result<T, GraphError>,
) -> Result<T, GraphError> {
    query_policy_fresh_graph_at_with_hooks(config, path, query, |_| {}, || {})
}

/// Materialize and serialize one worker response while the ordered snapshot
/// journal is still alive. The first checkpoint occurs before bytes enter the
/// worker's captured stdout pipe; the publication checkpoint occurs after the
/// pipe flush. The parent never exposes those bytes unless the worker exits
/// successfully, so a mutation during serialization/publication discards the
/// whole answer rather than leaking a stale projection.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn write_policy_projection_response(
    config: &Config,
    request: &PolicyProjectionRequest,
    writer: &mut dyn Write,
) -> Result<(), GraphError> {
    let requested_limit = match request {
        PolicyProjectionRequest::People { limit, .. }
        | PolicyProjectionRequest::RelationshipMap { limit }
        | PolicyProjectionRequest::RelationshipContext { limit }
        | PolicyProjectionRequest::Commitments { limit, .. }
        | PolicyProjectionRequest::LosingTouch { limit }
        | PolicyProjectionRequest::ParakeetBoostPhrases { limit } => Some(*limit),
        PolicyProjectionRequest::RebuildStats | PolicyProjectionRequest::PersonProfile { .. } => {
            None
        }
    };
    if requested_limit.is_some_and(|limit| limit > MAX_GRAPH_QUERY_ROWS) {
        return Err(GraphError::Io(std::io::Error::other(
            "graph projection result limit exceeded",
        )));
    }
    if matches!(
        request,
        PolicyProjectionRequest::RebuildStats
            | PolicyProjectionRequest::People {
                include_stats: true,
                ..
            }
    ) {
        rebuild_index_with_publication(config, |conn, stats, journal| {
            let response = match request {
                PolicyProjectionRequest::RebuildStats => {
                    PolicyProjectionResponse::RebuildStats(stats.clone())
                }
                PolicyProjectionRequest::People {
                    limit,
                    include_commitments,
                    include_stats: true,
                } => PolicyProjectionResponse::People(people_projection_from_connection(
                    conn,
                    *limit,
                    *include_commitments,
                    Some(stats.clone()),
                    graph_operation_deadline(),
                )?),
                _ => unreachable!("statistics publication request was prefiltered"),
            };
            serde_json::to_writer(&mut *writer, &response).map_err(|_| {
                GraphError::Io(std::io::Error::other(
                    "graph worker response could not be serialized",
                ))
            })?;
            writer.flush().map_err(GraphError::Io)?;
            journal.checkpoint("published")
        })?;
        return Ok(());
    }
    match request {
        PolicyProjectionRequest::PersonProfile { selector }
        | PolicyProjectionRequest::Commitments {
            selector: Some(selector),
            ..
        } => ensure_graph_entity_field(selector)?,
        _ => {}
    }
    let query_deadline = graph_operation_deadline();
    query_policy_fresh_graph_at_with_publication(
        config,
        Path::new(""),
        |conn| match request {
            PolicyProjectionRequest::RebuildStats
            | PolicyProjectionRequest::People {
                include_stats: true,
                ..
            } => unreachable!("handled above"),
            PolicyProjectionRequest::People {
                limit,
                include_commitments,
                include_stats: false,
            } => Ok(PolicyProjectionResponse::People(
                people_projection_from_connection(
                    conn,
                    *limit,
                    *include_commitments,
                    None,
                    query_deadline,
                )?,
            )),
            PolicyProjectionRequest::RelationshipMap { limit } => {
                Ok(PolicyProjectionResponse::RelationshipMap(
                    relationship_map_from_connection(conn, query_deadline, false, Some(*limit))?,
                ))
            }
            PolicyProjectionRequest::RelationshipContext { limit } => {
                let projection =
                    people_projection_from_connection(conn, *limit, true, None, query_deadline)?;
                Ok(PolicyProjectionResponse::RelationshipContext(
                    RelationshipContext {
                        people: projection.people,
                        commitments: projection.commitments,
                    },
                ))
            }
            PolicyProjectionRequest::PersonProfile { selector } => {
                Ok(PolicyProjectionResponse::PersonProfile(
                    policy_person_profile_from_connection(conn, selector, query_deadline)?,
                ))
            }
            PolicyProjectionRequest::Commitments { selector, limit } => Ok(
                PolicyProjectionResponse::Commitments(query_commitments_from_connection(
                    conn,
                    selector.as_deref(),
                    *limit,
                    query_deadline,
                )?),
            ),
            PolicyProjectionRequest::LosingTouch { limit } => {
                Ok(PolicyProjectionResponse::LosingTouch(
                    relationship_map_from_connection(conn, query_deadline, true, Some(*limit))?,
                ))
            }
            PolicyProjectionRequest::ParakeetBoostPhrases { limit } => {
                if *limit > MAX_GRAPH_QUERY_ROWS {
                    return Err(GraphError::Io(std::io::Error::other(
                        "graph boost-phrase result budget exceeded",
                    )));
                }
                Ok(PolicyProjectionResponse::ParakeetBoostPhrases(
                    parakeet_boost_phrases_from_connection(conn, *limit, query_deadline)?,
                ))
            }
        },
        |_| {},
        || {},
        |response, journal| {
            serde_json::to_writer(&mut *writer, response).map_err(|_| {
                GraphError::Io(std::io::Error::other(
                    "graph worker response could not be serialized",
                ))
            })?;
            writer.flush().map_err(GraphError::Io)?;
            journal.checkpoint("published")
        },
    )?;
    Ok(())
}

pub(crate) fn policy_projection_response_from_stream(
    sources: Vec<PolicyGraphStreamSource>,
    vocabulary_people: Vec<EntityRef>,
    correction_revision: String,
    request: &PolicyProjectionRequest,
) -> Result<PolicyProjectionResponse, GraphError> {
    let requested_limit = match request {
        PolicyProjectionRequest::People { limit, .. }
        | PolicyProjectionRequest::RelationshipMap { limit }
        | PolicyProjectionRequest::RelationshipContext { limit }
        | PolicyProjectionRequest::Commitments { limit, .. }
        | PolicyProjectionRequest::LosingTouch { limit }
        | PolicyProjectionRequest::ParakeetBoostPhrases { limit } => Some(*limit),
        PolicyProjectionRequest::RebuildStats | PolicyProjectionRequest::PersonProfile { .. } => {
            None
        }
    };
    if requested_limit.is_some_and(|limit| limit > MAX_GRAPH_QUERY_ROWS) {
        return Err(GraphError::Io(std::io::Error::other(
            "graph projection result limit exceeded",
        )));
    }
    match request {
        PolicyProjectionRequest::PersonProfile { selector }
        | PolicyProjectionRequest::Commitments {
            selector: Some(selector),
            ..
        } => ensure_graph_entity_field(selector)?,
        _ => {}
    }

    let include_alias_analysis = matches!(
        request,
        PolicyProjectionRequest::RebuildStats
            | PolicyProjectionRequest::People {
                include_stats: true,
                ..
            }
    );
    let (conn, stats) = populate_policy_projection_from_stream(
        sources,
        vocabulary_people,
        correction_revision,
        include_alias_analysis,
    )?;
    let query_deadline = graph_operation_deadline();
    match request {
        PolicyProjectionRequest::RebuildStats => Ok(PolicyProjectionResponse::RebuildStats(stats)),
        PolicyProjectionRequest::People {
            limit,
            include_commitments,
            include_stats,
        } => Ok(PolicyProjectionResponse::People(
            people_projection_from_connection(
                &conn,
                *limit,
                *include_commitments,
                include_stats.then_some(stats),
                query_deadline,
            )?,
        )),
        PolicyProjectionRequest::RelationshipMap { limit } => {
            Ok(PolicyProjectionResponse::RelationshipMap(
                relationship_map_from_connection(&conn, query_deadline, false, Some(*limit))?,
            ))
        }
        PolicyProjectionRequest::RelationshipContext { limit } => {
            let projection =
                people_projection_from_connection(&conn, *limit, true, None, query_deadline)?;
            Ok(PolicyProjectionResponse::RelationshipContext(
                RelationshipContext {
                    people: projection.people,
                    commitments: projection.commitments,
                },
            ))
        }
        PolicyProjectionRequest::PersonProfile { selector } => {
            Ok(PolicyProjectionResponse::PersonProfile(
                policy_person_profile_from_connection(&conn, selector, query_deadline)?,
            ))
        }
        PolicyProjectionRequest::Commitments { selector, limit } => Ok(
            PolicyProjectionResponse::Commitments(query_commitments_from_connection(
                &conn,
                selector.as_deref(),
                *limit,
                query_deadline,
            )?),
        ),
        PolicyProjectionRequest::LosingTouch { limit } => {
            Ok(PolicyProjectionResponse::LosingTouch(
                relationship_map_from_connection(&conn, query_deadline, true, Some(*limit))?,
            ))
        }
        PolicyProjectionRequest::ParakeetBoostPhrases { limit } => {
            Ok(PolicyProjectionResponse::ParakeetBoostPhrases(
                parakeet_boost_phrases_from_connection(&conn, *limit, query_deadline)?,
            ))
        }
    }
}

#[cfg(test)]
pub(crate) fn parakeet_boost_phrases(
    config: &Config,
    limit: usize,
) -> Result<Vec<String>, GraphError> {
    parakeet_boost_phrases_at(config, limit, Path::new(""))
}

#[cfg(test)]
fn parakeet_boost_phrases_at(
    config: &Config,
    limit: usize,
    path: &Path,
) -> Result<Vec<String>, GraphError> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let query_deadline = graph_operation_deadline();
    query_policy_fresh_graph_at(config, path, |conn| {
        parakeet_boost_phrases_from_connection(conn, limit, query_deadline)
    })
}

fn parakeet_boost_phrases_from_connection(
    conn: &Connection,
    limit: usize,
    query_deadline: std::time::Instant,
) -> Result<Vec<String>, GraphError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut phrases = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut people_stmt =
        conn.prepare("SELECT slug, name, meeting_count, last_seen FROM people")?;
    let people_rows = people_stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut people = Vec::new();
    let mut retained_bytes = 0usize;
    for row in people_rows {
        if people.len() >= MAX_GRAPH_QUERY_ROWS || std::time::Instant::now() >= query_deadline {
            return Err(graph_corpus_budget_error());
        }
        let row = row?;
        retain_graph_query_text(&mut retained_bytes, &row.0)?;
        retain_graph_query_text(&mut retained_bytes, &row.1)?;
        retain_graph_query_text(&mut retained_bytes, &row.3)?;
        people.push(row);
    }
    people.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| right.3.cmp(&left.3)));
    for (slug, name, _, _) in people.into_iter().take(200) {
        if let Some(phrase) = normalize_boost_phrase(&name, Some(&slug)) {
            push_unique_phrase(&mut phrases, &mut seen, phrase, limit);
        }
        if phrases.len() >= limit {
            return Ok(phrases);
        }
    }

    let mut meeting_stmt = conn.prepare("SELECT title, date FROM meetings")?;
    let meeting_rows = meeting_stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut meetings = Vec::new();
    let mut retained_bytes = 0usize;
    for row in meeting_rows {
        if meetings.len() >= MAX_GRAPH_QUERY_ROWS || std::time::Instant::now() >= query_deadline {
            return Err(graph_corpus_budget_error());
        }
        let row = row?;
        retain_graph_query_text(&mut retained_bytes, &row.0)?;
        retain_graph_query_text(&mut retained_bytes, &row.1)?;
        meetings.push(row);
    }
    meetings.sort_by(|left, right| right.1.cmp(&left.1));
    for (title, _) in meetings.into_iter().take(200) {
        for fragment in split_boost_title_fragments(&title) {
            if let Some(phrase) = normalize_boost_phrase(&fragment, None) {
                push_unique_phrase(&mut phrases, &mut seen, phrase, limit);
            }
            if phrases.len() >= limit {
                return Ok(phrases);
            }
        }
    }

    Ok(phrases)
}

fn push_unique_phrase(
    phrases: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
    phrase: String,
    limit: usize,
) {
    if phrases.len() >= limit {
        return;
    }
    let key = phrase.to_lowercase();
    if seen.insert(key) {
        phrases.push(phrase);
    }
}

fn normalize_boost_phrase(phrase: &str, slug: Option<&str>) -> Option<String> {
    let phrase = phrase.trim().trim_matches(|c: char| c == '"' || c == '\'');
    if phrase.len() < 3 {
        return None;
    }

    if let Some(slug) = slug {
        if slug == "unknown"
            || slug == "unknown-speaker"
            || slug.starts_with("speaker-")
            || slug.starts_with("unknown-")
        {
            return None;
        }
    }

    let lower = phrase.to_lowercase();
    if matches!(
        lower.as_str(),
        "unknown" | "unknown speaker" | "speaker 0" | "speaker 1" | "speaker 2" | "speaker 3"
    ) {
        return None;
    }

    let has_signal = phrase
        .chars()
        .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    if !has_signal {
        return None;
    }

    Some(phrase.to_string())
}

fn split_boost_title_fragments(title: &str) -> Vec<String> {
    title
        .replace(['—', '&', ','], "|")
        .split('|')
        .flat_map(|part| part.split(" with "))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
}

// ── Rebuild ───────────────────────────────────────────────────

/// Rebuild the entire graph index from markdown files.
#[cfg(test)]
pub(crate) fn rebuild_index(config: &Config) -> Result<GraphStats, GraphError> {
    rebuild_index_with_publication(config, |_, _, _| Ok(()))
}

#[cfg(test)]
fn rebuild_index_with_publication(
    config: &Config,
    mut publish_while_attested: impl FnMut(
        &Connection,
        &GraphStats,
        &mut GraphSnapshotJournal,
    ) -> Result<(), GraphError>,
) -> Result<GraphStats, GraphError> {
    let deadline = graph_operation_deadline();
    let mut operation = GraphOperationBudget::new(deadline);
    let mut derived = GraphDerivedBudget::default();
    crate::policy_fs::retire_legacy_policy_caches()?;
    let correction_paths = GraphCorrectionPaths::production();
    let canonical_root = config.output_dir.canonicalize()?;
    let _admission = graph_projection_admission(&canonical_root)?;
    for attempt in 0..3 {
        let mut journal =
            match GraphSnapshotJournal::begin(&canonical_root, &correction_paths, deadline) {
                Ok(journal) => journal,
                Err(_) if attempt < 2 => continue,
                Err(error) => return Err(error),
            };
        let correction_budget = operation.next_pass()?;
        let corrections = match graph_correction_snapshot(
            &correction_paths,
            &correction_budget,
            &mut derived,
            deadline,
        ) {
            Ok(corrections) => corrections,
            Err(_) if attempt < 2 => continue,
            Err(error) => return Err(error),
        };
        let projection_budget = operation.next_pass()?;
        let (conn, stats) = match rebuild_in_memory_projection_with_hook(
            config,
            &corrections,
            &projection_budget,
            &mut derived,
            |_| {},
            true,
        ) {
            Ok(rebuilt) => rebuilt,
            Err(_) if attempt < 2 => continue,
            Err(error) => return Err(error),
        };
        let trusted_corpus =
            graph_metadata_value(&conn, GRAPH_CORPUS_REVISION_KEY)?.ok_or_else(|| {
                GraphError::Io(std::io::Error::other("graph projection is unattested"))
            })?;
        if graph_inputs_still_attested(
            &canonical_root,
            &correction_paths,
            &trusted_corpus,
            &corrections.revision,
            &mut operation,
            &mut derived,
            deadline,
        )? && journal.checkpoint("statistics").is_ok()
        {
            publish_while_attested(&conn, &stats, &mut journal)?;
            return Ok(stats);
        }
    }
    Err(GraphError::Io(std::io::Error::other(
        "graph corrections changed while publishing rebuilt statistics",
    )))
}

/// Rebuild the graph index at a specific database path (for testing).
#[cfg(test)]
pub(crate) fn rebuild_index_at(config: &Config, path: &Path) -> Result<GraphStats, GraphError> {
    let deadline = graph_operation_deadline();
    let budget = graph_scan_budget(deadline);
    let mut derived = GraphDerivedBudget::default();
    let corrections = graph_correction_snapshot(
        &GraphCorrectionPaths::beside_graph(path),
        &budget,
        &mut derived,
        deadline,
    )?;
    rebuild_projection_with_hook(config, path, &corrections, |_| {})
}

#[cfg(test)]
fn rebuild_index_at_with_vocabulary_entities(
    config: &Config,
    path: &Path,
    vocabulary_people: Vec<EntityRef>,
) -> Result<GraphStats, GraphError> {
    rebuild_index_at_with_vocabulary_entities_and_hook(config, path, vocabulary_people, |_| {})
}

#[cfg(test)]
fn rebuild_index_at_with_vocabulary_entities_and_hook(
    config: &Config,
    path: &Path,
    vocabulary_people: Vec<EntityRef>,
    after_source_snapshot: impl FnMut(&Path),
) -> Result<GraphStats, GraphError> {
    let speaker_overlays =
        overlays::stable_speaker_overlay_snapshot_at(&overlays::db_path_for_graph_path(path))
            .map_err(|_| {
                GraphError::Io(std::io::Error::other(
                    "speaker corrections could not be verified",
                ))
            })?;
    let vocabulary_revision = {
        let mut hasher = Sha256::new();
        hasher.update(GRAPH_VOCABULARY_REVISION_DOMAIN);
        for entity in &vocabulary_people {
            hash_revision_field(&mut hasher, &entity.slug);
            hash_revision_field(&mut hasher, &entity.label);
            for alias in &entity.aliases {
                hash_revision_field(&mut hasher, alias);
            }
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let corrections = GraphCorrectionSnapshot {
        revision: correction_aggregate_revision(&vocabulary_revision, speaker_overlays.revision()),
        vocabulary_people,
        speaker_overlays,
    };
    rebuild_projection_with_hook(config, path, &corrections, after_source_snapshot)
}

#[cfg(test)]
fn rebuild_projection_with_hook(
    config: &Config,
    path: &Path,
    corrections: &GraphCorrectionSnapshot,
    after_source_snapshot: impl FnMut(&Path),
) -> Result<GraphStats, GraphError> {
    if path.exists()
        && Connection::open(path)
            .and_then(|connection| connection.execute_batch("SELECT 1 FROM people LIMIT 1"))
            .is_err()
    {
        tracing::warn!("Corrupted test graph projection detected, rebuilding from scratch");
        std::fs::remove_file(path).ok();
    }
    let conn = open_db(path)?;
    let mut derived = GraphDerivedBudget::default();
    let (conn, stats) = populate_projection_with_hook(
        config,
        conn,
        corrections,
        &ActiveCorpusReadBudget::new(),
        &mut derived,
        after_source_snapshot,
        true,
    )?;
    let trusted_revision = graph_metadata_value(&conn, GRAPH_CORPUS_REVISION_KEY)?
        .ok_or_else(|| GraphError::Io(std::io::Error::other("graph projection is unattested")))?;
    if graph_corpus_revision(
        &config.output_dir.canonicalize()?,
        &ActiveCorpusReadBudget::new(),
        &mut derived,
    )? != trusted_revision
    {
        return Err(GraphError::Io(std::io::Error::other(
            "graph corpus changed while publishing the rebuilt index",
        )));
    }
    set_db_permissions(path);
    Ok(stats)
}

#[cfg(test)]
fn rebuild_in_memory_projection_with_hook(
    config: &Config,
    corrections: &GraphCorrectionSnapshot,
    budget: &ActiveCorpusReadBudget,
    derived: &mut GraphDerivedBudget,
    after_source_snapshot: impl FnMut(&Path),
    include_alias_analysis: bool,
) -> Result<(Connection, GraphStats), GraphError> {
    let conn = open_memory_db()?;
    let progress_budget = budget.clone();
    conn.progress_handler(
        1_000,
        Some(move || progress_budget.check_deadline().is_err()),
    );
    populate_projection_with_hook(
        config,
        conn,
        corrections,
        budget,
        derived,
        after_source_snapshot,
        include_alias_analysis,
    )
}

#[cfg(test)]
fn populate_projection_with_hook(
    config: &Config,
    conn: Connection,
    corrections: &GraphCorrectionSnapshot,
    budget: &ActiveCorpusReadBudget,
    derived_budget: &mut GraphDerivedBudget,
    mut after_source_snapshot: impl FnMut(&Path),
    include_alias_analysis: bool,
) -> Result<(Connection, GraphStats), GraphError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(GraphError::DirNotFound(dir.display().to_string()));
    }
    let canonical_root = dir.canonicalize()?;
    let sources = collect_policy_graph_sources(
        dir,
        &canonical_root,
        corrections,
        budget,
        derived_budget,
        &mut after_source_snapshot,
    )?;
    populate_projection_from_sources(
        conn,
        corrections,
        budget,
        derived_budget,
        sources,
        include_alias_analysis,
    )
}

fn collect_policy_graph_sources(
    dir: &Path,
    canonical_root: &Path,
    corrections: &GraphCorrectionSnapshot,
    budget: &ActiveCorpusReadBudget,
    derived_budget: &mut GraphDerivedBudget,
    after_source_snapshot: &mut impl FnMut(&Path),
) -> Result<Vec<PolicyGraphStreamSource>, GraphError> {
    let mut sources = Vec::new();
    let walker = WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !is_inactive_corpus_dir_name(entry.file_name())
        });
    for entry in walker {
        budget
            .check_deadline()
            .map_err(|_| graph_corpus_budget_error())?;
        let entry = entry.map_err(|_| {
            GraphError::Io(std::io::Error::other(
                "graph corpus traversal could not be verified",
            ))
        })?;
        derived_budget.consume(1, budget)?;
        derived_budget.consume_path(entry.path(), budget)?;
        if entry.file_type().is_dir() {
            consume_graph_corpus(budget, 0, 1, 0)?;
            continue;
        }
        consume_graph_corpus(budget, 1, 0, 0)?;
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|ext| ext.to_str()) != Some("md")
        {
            continue;
        }
        let source = read_policy_graph_source(entry.path(), canonical_root, budget);
        budget
            .check_deadline()
            .map_err(|_| graph_corpus_budget_error())?;
        let Some(source) = source else {
            tracing::debug!("skipping policy-ineligible meeting during graph rebuild");
            continue;
        };
        consume_graph_corpus(budget, 0, 0, source.content.len() as u64)?;
        derived_budget.consume(1, budget)?;
        let speaker_corrections = corrections
            .speaker_overlays
            .confirmations_for_source(&source.path, &source.content_sha256)
            .into_iter()
            .map(|confirmation| PolicyGraphSpeakerCorrection {
                speaker_label: confirmation.speaker_label,
                name: confirmation.name,
            })
            .collect();
        after_source_snapshot(&source.path);
        sources.push(PolicyGraphStreamSource {
            opaque_path: source.path,
            content: source.content,
            content_sha256: source.content_sha256,
            speaker_corrections,
        });
    }
    Ok(sources)
}

pub(crate) fn populate_policy_projection_from_stream(
    sources: Vec<PolicyGraphStreamSource>,
    vocabulary_people: Vec<EntityRef>,
    correction_revision: String,
    include_alias_analysis: bool,
) -> Result<(Connection, GraphStats), GraphError> {
    let conn = open_memory_db()?;
    let budget = ActiveCorpusReadBudget::new();
    let progress_budget = budget.clone();
    conn.progress_handler(
        1_000,
        Some(move || progress_budget.check_deadline().is_err()),
    );
    let corrections = GraphCorrectionSnapshot {
        vocabulary_people,
        speaker_overlays: overlays::StableSpeakerOverlaySnapshot::empty(),
        revision: correction_revision,
    };
    let mut derived = GraphDerivedBudget::default();
    populate_projection_from_sources(
        conn,
        &corrections,
        &budget,
        &mut derived,
        sources,
        include_alias_analysis,
    )
}

fn populate_projection_from_sources(
    conn: Connection,
    corrections: &GraphCorrectionSnapshot,
    budget: &ActiveCorpusReadBudget,
    derived_budget: &mut GraphDerivedBudget,
    sources: Vec<PolicyGraphStreamSource>,
    include_alias_analysis: bool,
) -> Result<(Connection, GraphStats), GraphError> {
    let start = std::time::Instant::now();
    let projection_now = Local::now();

    // Wrap the private in-memory projection in one transaction so a failed
    // rebuild cannot expose a partially materialized answer.
    conn.execute_batch("BEGIN IMMEDIATE")?;

    // Clear existing data for full rebuild
    conn.execute_batch(
        "DELETE FROM meeting_topics;
         DELETE FROM people_meetings;
         DELETE FROM commitments;
         DELETE FROM decisions;
         DELETE FROM meetings;
         DELETE FROM topics;
         DELETE FROM people;
         DELETE FROM graph_metadata;",
    )?;

    // Walk all markdown files
    let mut people_map: HashMap<String, (String, Vec<String>)> = HashMap::new(); // slug -> (stable name, aliases)
    let mut meeting_count = 0usize;
    let mut commitment_count = 0usize;
    let mut topic_set: HashMap<String, i64> = HashMap::new(); // name -> id
    let mut source_revision_entries: Vec<(PathBuf, [u8; 32])> = Vec::new();
    for source in sources {
        budget
            .check_deadline()
            .map_err(|_| graph_corpus_budget_error())?;
        let file_path = source.opaque_path;
        let (fm_str, body) = split_frontmatter(&source.content);
        let Some(frontmatter) = parse_graph_frontmatter(fm_str) else {
            continue;
        };
        for value in frontmatter
            .attendees
            .iter()
            .chain(frontmatter.people.iter())
            .chain(frontmatter.action_items.iter().map(|item| &item.assignee))
            .chain(
                frontmatter
                    .intents
                    .iter()
                    .filter_map(|intent| intent.who.as_ref()),
            )
            .chain(
                frontmatter
                    .speaker_map
                    .iter()
                    .map(|item| &item.speaker_label),
            )
            .chain(frontmatter.speaker_map.iter().map(|item| &item.name))
        {
            ensure_graph_entity_field(value)?;
        }
        for entity in &frontmatter.entities.people {
            ensure_graph_entity_field(&entity.slug)?;
            ensure_graph_entity_field(&entity.label)?;
            for alias in &entity.aliases {
                ensure_graph_entity_field(alias)?;
            }
        }
        source_revision_entries.push((file_path.clone(), source.content_sha256));

        let content_type_str = match frontmatter.r#type {
            ContentType::Meeting => "meeting",
            ContentType::Memo => "memo",
            ContentType::Dictation => "dictation",
        };
        let date_str = frontmatter.date.to_rfc3339();
        let duration_secs = parse_duration_secs(&frontmatter.duration);
        let speakers = extract_speakers_from_transcript(body, budget, derived_budget)?;
        // Correction rows are enrichments only: lookup happens after this
        // exact source descriptor has parsed as normal-authorized.
        let speaker_map = speaker_map_with_confirmations(
            &frontmatter.speaker_map,
            &source.speaker_corrections,
            &speakers,
        );
        derived_budget.consume(
            [
                frontmatter.attendees.len(),
                frontmatter.people.len(),
                frontmatter.entities.people.len(),
                frontmatter.action_items.len(),
                frontmatter.intents.len(),
                frontmatter.decisions.len(),
                frontmatter.tags.len(),
                speakers.len(),
                speaker_map.len(),
            ]
            .into_iter()
            .fold(0usize, usize::saturating_add),
            budget,
        )?;

        // Insert meeting
        conn.execute(
            "INSERT OR IGNORE INTO meetings (path, title, date, duration_secs, content_type) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![file_path.to_string_lossy().as_ref(), frontmatter.title, date_str, duration_secs, content_type_str],
        )?;
        let meeting_id: i64 = conn.query_row(
            "SELECT id FROM meetings WHERE path = ?1",
            params![file_path.to_string_lossy().as_ref()],
            |row| row.get(0),
        )?;
        meeting_count += 1;

        for decision in &frontmatter.decisions {
            ensure_graph_entity_field(&decision.text)?;
            if let Some(topic) = &decision.topic {
                ensure_graph_entity_field(topic)?;
            }
            if let Some(authority) = &decision.authority {
                ensure_graph_entity_field(authority)?;
            }
            derived_budget.consume_entity_text(&decision.text, budget)?;
            if let Some(topic) = &decision.topic {
                derived_budget.consume_entity_text(topic, budget)?;
            }
            conn.execute(
                "INSERT INTO decisions (meeting_id, text, topic, authority) VALUES (?1, ?2, ?3, ?4)",
                params![meeting_id, decision.text, decision.topic, decision.authority],
            )?;
        }

        let normalized_attendees = frontmatter.normalized_attendees();
        let context_names: Vec<&str> = normalized_attendees
            .iter()
            .map(String::as_str)
            .chain(frontmatter.people.iter().map(String::as_str))
            .chain(speakers.iter().map(String::as_str))
            .chain(
                speaker_map
                    .iter()
                    .filter(|attr| attr.confidence == crate::diarize::Confidence::High)
                    .map(|attr| attr.name.as_str()),
            )
            .chain(
                frontmatter
                    .action_items
                    .iter()
                    .map(|item| item.assignee.as_str()),
            )
            .chain(
                frontmatter
                    .intents
                    .iter()
                    .filter_map(|intent| intent.who.as_deref()),
            )
            .collect();
        for name in &context_names {
            derived_budget.consume_entity_text(name, budget)?;
        }
        for attribution in &speaker_map {
            derived_budget.consume_entity_text(&attribution.speaker_label, budget)?;
        }
        for entity in frontmatter
            .entities
            .people
            .iter()
            .chain(corrections.vocabulary_people.iter())
        {
            // Corrections are intentionally charged on every per-meeting
            // canonicalizer construction. This accounts the actual repeated
            // clone/map work rather than treating one correction snapshot as
            // free after its first use.
            derived_budget.consume_entity_ref(entity, budget)?;
        }
        let mut canonical_people = frontmatter.entities.people.clone();
        canonical_people.extend(corrections.vocabulary_people.iter().cloned());
        let canonicalizer = PersonCanonicalizer::new(&canonical_people, context_names);

        // Extract people from multiple sources
        let mut file_people: Vec<(String, String, Vec<String>, &'static str)> = Vec::new(); // (slug, name, aliases, role)

        // Source 1: frontmatter.attendees
        for attendee in normalized_attendees {
            if let Some(identity) = canonicalizer.resolve(&attendee) {
                push_file_person(
                    &mut file_people,
                    identity.slug,
                    identity.name,
                    identity.aliases,
                    "attendee",
                );
            }
        }

        // Source 2: frontmatter.people
        for person in &frontmatter.people {
            if let Some(identity) = canonicalizer.resolve(person) {
                push_file_person(
                    &mut file_people,
                    identity.slug,
                    identity.name,
                    identity.aliases,
                    "mentioned",
                );
            }
        }

        // Source 3: entities.people (richest identity data, but an entity is a
        // mention unless attendee/speaker evidence above already promoted it).
        for entity in &frontmatter.entities.people {
            if let Some(identity) = canonicalizer.resolve_entity(entity) {
                push_file_person(
                    &mut file_people,
                    identity.slug,
                    identity.name,
                    identity.aliases,
                    "mentioned",
                );
            }
        }

        // Source 4: transcript speaker labels [NAME HH:MM] or [NAME M:SS]
        let confirmed_speaker_label_slugs: HashSet<String> = speaker_map
            .iter()
            .filter(|attr| attr.confidence == crate::diarize::Confidence::High)
            .map(|attr| slugify(&attr.speaker_label))
            .collect();
        for speaker in &speakers {
            if confirmed_speaker_label_slugs.contains(&slugify(speaker)) {
                continue;
            }
            if let Some(identity) = canonicalizer.resolve(speaker) {
                push_file_person(
                    &mut file_people,
                    identity.slug,
                    identity.name,
                    identity.aliases,
                    "speaker",
                );
            }
        }

        // Source 5: speaker_map (confirmed speaker attributions)
        for attr in &speaker_map {
            if attr.confidence == crate::diarize::Confidence::High {
                if let Some(identity) = canonicalizer.resolve(&attr.name) {
                    push_file_person(
                        &mut file_people,
                        identity.slug,
                        identity.name,
                        identity.aliases,
                        "speaker",
                    );
                }
            }
        }

        // Source 6: explicit actionable owners. They remain mentioned-only
        // unless independent attendee/speaker evidence promoted them, but the
        // commitment must still retain an exact canonical owner.
        for item in &frontmatter.action_items {
            if matches!(item.status.as_str(), "open" | "stale") {
                let owner = resolved_graph_owner(&item.assignee, &speaker_map);
                if let Some(identity) = canonicalizer.resolve(owner) {
                    push_file_person(
                        &mut file_people,
                        identity.slug,
                        identity.name,
                        identity.aliases,
                        "mentioned",
                    );
                }
            }
        }
        for intent in &frontmatter.intents {
            if matches!(intent.kind, IntentKind::ActionItem | IntentKind::Commitment)
                && matches!(intent.status.as_str(), "open" | "stale")
            {
                if let Some(identity) = intent
                    .who
                    .as_deref()
                    .map(|who| resolved_graph_owner(who, &speaker_map))
                    .and_then(|who| canonicalizer.resolve(who))
                {
                    push_file_person(
                        &mut file_people,
                        identity.slug,
                        identity.name,
                        identity.aliases,
                        "mentioned",
                    );
                }
            }
        }

        derived_budget.consume(file_people.len(), budget)?;

        // Insert/update people and link to meeting
        for (slug, name, aliases, role) in &file_people {
            let person = people_map
                .entry(slug.clone())
                .or_insert_with(|| (name.clone(), Vec::new()));
            let stable_name = preferred_person_display_name(slug, &person.0, name);
            for alias in aliases
                .iter()
                .chain(std::iter::once(&person.0))
                .chain(std::iter::once(name))
            {
                if !alias.eq_ignore_ascii_case(&stable_name)
                    && !person
                        .1
                        .iter()
                        .any(|existing| existing.eq_ignore_ascii_case(alias))
                {
                    person.1.push(alias.clone());
                }
            }
            person.0 = stable_name;
            person.1.sort_by_key(|alias| alias.to_lowercase());
            let aliases_json = serde_json::to_string(&person.1).unwrap_or_else(|_| "[]".into());
            let contact_increment = i64::from(matches!(*role, "attendee" | "speaker"));

            // Upsert person
            conn.execute(
                "INSERT INTO people (slug, name, aliases, first_seen, last_seen, meeting_count)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5)
                 ON CONFLICT(slug) DO UPDATE SET
                   last_seen = CASE
                     WHEN ?5 = 1 AND meeting_count = 0 THEN ?4
                     WHEN ?5 = 1 AND ?4 > last_seen THEN ?4
                     ELSE last_seen END,
                   first_seen = CASE
                     WHEN ?5 = 1 AND meeting_count = 0 THEN ?4
                     WHEN ?5 = 1 AND ?4 < first_seen THEN ?4
                     ELSE first_seen END,
                   meeting_count = meeting_count + ?5,
                   name = ?2,
                   aliases = ?3",
                params![slug, &person.0, aliases_json, date_str, contact_increment],
            )?;

            let person_id: i64 = conn.query_row(
                "SELECT id FROM people WHERE slug = ?1",
                params![slug],
                |row| row.get(0),
            )?;

            // Link person to meeting
            conn.execute(
                "INSERT OR IGNORE INTO people_meetings (person_id, meeting_id, role) VALUES (?1, ?2, ?3)",
                params![person_id, meeting_id, role],
            )?;
        }

        // Build one canonical actionable set per meeting. Pipelines may emit
        // the same promise in both action_items and intents; stale wins while
        // duplicate representations never inflate counts or output.
        let mut pending_commitments: BTreeMap<String, PendingGraphCommitment> = BTreeMap::new();
        for item in &frontmatter.action_items {
            let Some(status) =
                actionable_commitment_status(&item.status, item.due.as_deref(), projection_now)
            else {
                continue;
            };
            let owner = resolved_graph_owner(&item.assignee, &speaker_map);
            let person_id = canonicalizer.resolve(owner).and_then(|identity| {
                conn.query_row(
                    "SELECT id FROM people WHERE slug = ?1",
                    params![identity.slug],
                    |row| row.get::<_, i64>(0),
                )
                .ok()
            });
            let key = graph_commitment_key(&item.task, person_id, Some(owner), item.due.as_deref());
            match pending_commitments.entry(key) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(PendingGraphCommitment {
                        person_id,
                        text: item.task.clone(),
                        status: status.to_string(),
                        due_date: item.due.clone(),
                        commitment_type: "action_item",
                    });
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if status == "stale" {
                        entry.get_mut().status = "stale".to_string();
                    }
                }
            }
        }

        for intent in &frontmatter.intents {
            if !matches!(intent.kind, IntentKind::ActionItem | IntentKind::Commitment) {
                continue;
            }
            let Some(status) = actionable_commitment_status(
                &intent.status,
                intent.by_date.as_deref(),
                projection_now,
            ) else {
                continue;
            };
            let person_id = intent.who.as_ref().and_then(|who| {
                let identity = canonicalizer.resolve(resolved_graph_owner(who, &speaker_map))?;
                conn.query_row(
                    "SELECT id FROM people WHERE slug = ?1",
                    params![identity.slug],
                    |row| row.get::<_, i64>(0),
                )
                .ok()
            });
            let key = graph_commitment_key(
                &intent.what,
                person_id,
                intent
                    .who
                    .as_deref()
                    .map(|who| resolved_graph_owner(who, &speaker_map)),
                intent.by_date.as_deref(),
            );
            match pending_commitments.entry(key) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(PendingGraphCommitment {
                        person_id,
                        text: intent.what.clone(),
                        status: status.to_string(),
                        due_date: intent.by_date.clone(),
                        commitment_type: if intent.kind == IntentKind::ActionItem {
                            "action_item"
                        } else {
                            "intent"
                        },
                    });
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if status == "stale" {
                        entry.get_mut().status = "stale".to_string();
                    }
                }
            }
        }
        derived_budget.consume(pending_commitments.len(), budget)?;
        for commitment in pending_commitments.into_values() {
            conn.execute(
                "INSERT INTO commitments (meeting_id, person_id, text, status, due_date, created_at, commitment_type)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    meeting_id,
                    commitment.person_id,
                    commitment.text,
                    commitment.status,
                    commitment.due_date,
                    date_str,
                    commitment.commitment_type,
                ],
            )?;
            commitment_count += 1;
        }

        // Extract topics from tags, decisions, and title
        let mut file_topics: Vec<String> = Vec::new();
        for tag in &frontmatter.tags {
            file_topics.push(tag.to_lowercase());
        }
        for decision in &frontmatter.decisions {
            if let Some(topic) = &decision.topic {
                file_topics.push(topic.to_lowercase());
            }
        }
        // Title keywords (words > 3 chars, skip common words)
        for word in extract_title_keywords(&frontmatter.title) {
            file_topics.push(word);
        }
        if let Some(cal) = &frontmatter.calendar_event {
            for word in extract_title_keywords(cal) {
                file_topics.push(word);
            }
        }

        file_topics.sort();
        file_topics.dedup();
        derived_budget.consume(file_topics.len(), budget)?;
        for topic in &file_topics {
            derived_budget.consume_entity_text(topic, budget)?;
        }

        for topic_name in &file_topics {
            if !topic_set.contains_key(topic_name) {
                conn.execute(
                    "INSERT OR IGNORE INTO topics (name) VALUES (?1)",
                    params![topic_name],
                )?;
                let tid: i64 = conn.query_row(
                    "SELECT id FROM topics WHERE name = ?1",
                    params![topic_name],
                    |row| row.get(0),
                )?;
                topic_set.insert(topic_name.clone(), tid);
            }
            let tid = topic_set[topic_name];
            conn.execute(
                "INSERT OR IGNORE INTO meeting_topics (meeting_id, topic_id) VALUES (?1, ?2)",
                params![meeting_id, tid],
            )?;
        }
    }

    // Pairwise alias analysis is an explicit rebuild/merge diagnostic, not a
    // prerequisite for relationship and commitment answers. Keeping it out of
    // ordinary reads avoids two O(P²) passes on every consumer query.
    let (alias_suggestions, alias_clusters) = if include_alias_analysis {
        (
            detect_aliases(&conn, budget, derived_budget)?,
            detect_alias_clusters(&conn, budget, derived_budget)?,
        )
    } else {
        (Vec::new(), Vec::new())
    };

    // Bind this derived cache to the exact configured corpus. Public reads
    // verify this value before exposing any graph-derived facts, so a process
    // switching output_dir cannot accidentally consume another corpus.
    let corpus_revision = graph_revision_from_entries(&mut source_revision_entries);
    conn.execute(
        "INSERT INTO graph_metadata (key, value) VALUES (?1, ?2)",
        params![GRAPH_CORPUS_ROOT_KEY, "/__minutes_graph_source"],
    )?;
    conn.execute(
        "INSERT INTO graph_metadata (key, value) VALUES (?1, ?2)",
        params![GRAPH_CORPUS_REVISION_KEY, corpus_revision],
    )?;
    conn.execute(
        "INSERT INTO graph_metadata (key, value) VALUES (?1, ?2)",
        params![GRAPH_CORRECTION_REVISION_KEY, corrections.revision],
    )?;

    // Commit the transaction — all or nothing
    conn.execute_batch("COMMIT")?;

    let elapsed = start.elapsed().as_millis() as u64;
    tracing::info!(
        people = people_map.len(),
        meetings = meeting_count,
        commitments = commitment_count,
        topics = topic_set.len(),
        aliases = alias_suggestions.len(),
        elapsed_ms = elapsed,
        "Index rebuilt"
    );

    let stats = GraphStats {
        people_count: people_map.len(),
        meeting_count,
        commitment_count,
        topic_count: topic_set.len(),
        alias_suggestions,
        alias_clusters,
        rebuild_ms: elapsed,
    };
    Ok((conn, stats))
}

fn speaker_map_with_confirmations(
    speaker_map: &[SpeakerAttribution],
    confirmations: &[PolicyGraphSpeakerCorrection],
    transcript_speakers: &[String],
) -> Vec<SpeakerAttribution> {
    let mut combined = speaker_map.to_vec();
    for confirmation in confirmations {
        // A bound row enriches an attribution already evidenced by these exact
        // source bytes. It can never append a person to unrelated content.
        if let Some(existing) = combined
            .iter_mut()
            .find(|attr| attr.speaker_label == confirmation.speaker_label)
        {
            existing.name = confirmation.name.clone();
            existing.confidence = crate::diarize::Confidence::High;
            existing.source = crate::diarize::AttributionSource::Manual;
        } else if transcript_speakers
            .iter()
            .any(|speaker| speaker.eq_ignore_ascii_case(&confirmation.speaker_label))
        {
            combined.push(SpeakerAttribution {
                speaker_label: confirmation.speaker_label.clone(),
                name: confirmation.name.clone(),
                confidence: crate::diarize::Confidence::High,
                source: crate::diarize::AttributionSource::Manual,
            });
        }
    }
    combined
}

fn resolved_graph_owner<'a>(owner: &'a str, speaker_map: &'a [SpeakerAttribution]) -> &'a str {
    speaker_map
        .iter()
        .find(|attribution| {
            attribution.confidence == crate::diarize::Confidence::High
                && attribution.speaker_label.eq_ignore_ascii_case(owner)
        })
        .map_or(owner, |attribution| attribution.name.as_str())
}

// ── Queries ───────────────────────────────────────────────────

fn top_topics_for_person(
    conn: &Connection,
    person_id: i64,
    limit: usize,
    deadline: std::time::Instant,
) -> Result<Vec<String>, GraphError> {
    let mut stmt = conn.prepare(
        "SELECT t.name FROM meeting_topics mt
         JOIN topics t ON mt.topic_id = t.id
         JOIN people_meetings pm ON pm.meeting_id = mt.meeting_id
         WHERE pm.person_id = ?1 AND pm.role IN ('attendee', 'speaker')",
    )?;
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut retained_bytes = 0usize;
    let mut associations = 0usize;
    let rows = stmt.query_map(params![person_id], |row| row.get::<_, String>(0))?;
    for row in rows {
        if std::time::Instant::now() >= deadline {
            return Err(graph_corpus_budget_error());
        }
        associations = associations.checked_add(1).ok_or_else(|| {
            GraphError::Io(std::io::Error::other(
                "graph topic-association budget overflowed",
            ))
        })?;
        if associations > MAX_GRAPH_TOPIC_ASSOCIATIONS_PER_PERSON {
            return Err(GraphError::Io(std::io::Error::other(
                "graph topic-association budget exceeded",
            )));
        }
        let topic = row?;
        if let Some(count) = counts.get_mut(&topic) {
            *count = count.saturating_add(1);
        } else {
            retain_graph_query_text(&mut retained_bytes, &topic)?;
            counts.insert(topic, 1);
        }
    }
    let mut ranked: Vec<_> = counts.into_iter().collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    Ok(ranked
        .into_iter()
        .take(limit)
        .map(|(topic, _)| topic)
        .collect())
}

/// Build an exact person profile from the same correction-aware,
/// policy-filtered projection used by relationship maps and commitments.
/// Selectors match only canonical slug, exact normalized display name, or an
/// exact confirmed alias; ambiguous selectors fail closed.
fn policy_person_profile_from_connection(
    conn: &Connection,
    selector: &str,
    query_deadline: std::time::Instant,
) -> Result<PolicyPersonProfile, GraphError> {
    let Some(person_id) = resolve_commitment_person_selector(conn, selector, query_deadline)?
    else {
        return Ok(PolicyPersonProfile {
            name: selector.trim().to_string(),
            recent_meetings: Vec::new(),
            open_intents: Vec::new(),
            recent_decisions: Vec::new(),
            top_topics: Vec::new(),
        });
    };
    let person_name: String = conn.query_row(
        "SELECT name FROM people WHERE id = ?1",
        params![person_id],
        |row| row.get(0),
    )?;
    let mut retained_bytes = 0usize;
    retain_graph_query_text(&mut retained_bytes, &person_name)?;

    let mut recent_meetings = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT m.path, m.title, m.date, m.content_type
             FROM meetings m
             JOIN people_meetings pm ON pm.meeting_id = m.id
             WHERE pm.person_id = ?1 AND pm.role IN ('attendee', 'speaker')",
        )?;
        let rows = stmt.query_map(params![person_id], |row| {
            Ok(PolicyMeetingReference {
                path: PathBuf::from(row.get::<_, String>(0)?),
                title: row.get(1)?,
                date: row.get(2)?,
                content_type: row.get(3)?,
            })
        })?;
        for row in rows {
            if std::time::Instant::now() >= query_deadline {
                return Err(graph_corpus_budget_error());
            }
            if recent_meetings.len() >= MAX_GRAPH_QUERY_ROWS {
                return Err(GraphError::Io(std::io::Error::other(
                    "graph profile meeting result budget exceeded",
                )));
            }
            let meeting = row?;
            retain_graph_query_text(&mut retained_bytes, &meeting.path.to_string_lossy())?;
            retain_graph_query_text(&mut retained_bytes, &meeting.title)?;
            retain_graph_query_text(&mut retained_bytes, &meeting.date)?;
            retain_graph_query_text(&mut retained_bytes, &meeting.content_type)?;
            recent_meetings.push(meeting);
        }
    }
    recent_meetings.sort_by(|left, right| {
        right
            .date
            .cmp(&left.date)
            .then_with(|| left.path.cmp(&right.path))
    });
    recent_meetings.truncate(5);

    let mut open_intents = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT m.path, m.title, m.date, m.content_type,
                    c.commitment_type, c.text, p.name, c.status, c.due_date
             FROM commitments c
             JOIN meetings m ON m.id = c.meeting_id
             LEFT JOIN people p ON p.id = c.person_id
             WHERE c.person_id = ?1 AND c.status IN ('open', 'stale')",
        )?;
        let rows = stmt.query_map(params![person_id], |row| {
            let commitment_type: String = row.get(4)?;
            Ok(PolicyIntentReference {
                path: PathBuf::from(row.get::<_, String>(0)?),
                title: row.get(1)?,
                date: row.get(2)?,
                content_type: row.get(3)?,
                kind: if commitment_type == "action_item" {
                    IntentKind::ActionItem
                } else {
                    IntentKind::Commitment
                },
                what: row.get(5)?,
                who: row.get(6)?,
                who_original: None,
                who_provenance: None,
                status: row.get(7)?,
                by_date: row.get(8)?,
            })
        })?;
        for row in rows {
            if std::time::Instant::now() >= query_deadline {
                return Err(graph_corpus_budget_error());
            }
            if open_intents.len() >= MAX_GRAPH_QUERY_ROWS {
                return Err(GraphError::Io(std::io::Error::other(
                    "graph profile commitment result budget exceeded",
                )));
            }
            let intent = row?;
            for value in [
                intent.path.to_string_lossy().as_ref(),
                &intent.title,
                &intent.date,
                &intent.content_type,
                &intent.what,
                &intent.status,
            ] {
                retain_graph_query_text(&mut retained_bytes, value)?;
            }
            if let Some(value) = &intent.who {
                retain_graph_query_text(&mut retained_bytes, value)?;
            }
            if let Some(value) = &intent.by_date {
                retain_graph_query_text(&mut retained_bytes, value)?;
            }
            open_intents.push(intent);
        }
    }
    open_intents.sort_by(|left, right| {
        right
            .date
            .cmp(&left.date)
            .then_with(|| left.what.cmp(&right.what))
    });
    open_intents.truncate(10);

    let mut recent_decisions = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT m.path, m.title, m.date, d.text, d.authority
             FROM decisions d
             JOIN meetings m ON m.id = d.meeting_id
             JOIN people_meetings pm ON pm.meeting_id = m.id
             WHERE pm.person_id = ?1 AND pm.role IN ('attendee', 'speaker')",
        )?;
        let rows = stmt.query_map(params![person_id], |row| {
            Ok(PolicyDecisionReference {
                path: PathBuf::from(row.get::<_, String>(0)?),
                title: row.get(1)?,
                date: row.get(2)?,
                what: row.get(3)?,
                who: None,
                who_original: None,
                who_provenance: None,
                by_date: None,
                authority: row.get(4)?,
            })
        })?;
        for row in rows {
            if std::time::Instant::now() >= query_deadline {
                return Err(graph_corpus_budget_error());
            }
            if recent_decisions.len() >= MAX_GRAPH_QUERY_ROWS {
                return Err(GraphError::Io(std::io::Error::other(
                    "graph profile decision result budget exceeded",
                )));
            }
            let decision = row?;
            retain_graph_query_text(&mut retained_bytes, &decision.path.to_string_lossy())?;
            retain_graph_query_text(&mut retained_bytes, &decision.title)?;
            retain_graph_query_text(&mut retained_bytes, &decision.date)?;
            retain_graph_query_text(&mut retained_bytes, &decision.what)?;
            if let Some(value) = &decision.authority {
                retain_graph_query_text(&mut retained_bytes, value)?;
            }
            recent_decisions.push(decision);
        }
    }
    recent_decisions.sort_by(|left, right| {
        right
            .date
            .cmp(&left.date)
            .then_with(|| left.what.cmp(&right.what))
    });
    recent_decisions.truncate(5);

    let mut topic_counts = HashMap::<String, usize>::new();
    {
        let mut stmt = conn.prepare(
            "SELECT t.name
             FROM meeting_topics mt
             JOIN topics t ON t.id = mt.topic_id
             JOIN people_meetings pm ON pm.meeting_id = mt.meeting_id
             WHERE pm.person_id = ?1 AND pm.role IN ('attendee', 'speaker')",
        )?;
        let rows = stmt.query_map(params![person_id], |row| row.get::<_, String>(0))?;
        let mut associations = 0usize;
        for row in rows {
            if std::time::Instant::now() >= query_deadline {
                return Err(graph_corpus_budget_error());
            }
            associations = associations.checked_add(1).ok_or_else(|| {
                GraphError::Io(std::io::Error::other(
                    "graph profile topic budget overflowed",
                ))
            })?;
            if associations > MAX_GRAPH_TOPIC_ASSOCIATIONS_PER_PERSON {
                return Err(GraphError::Io(std::io::Error::other(
                    "graph profile topic budget exceeded",
                )));
            }
            let topic = row?;
            if let Some(count) = topic_counts.get_mut(&topic) {
                *count = count.saturating_add(1);
            } else {
                retain_graph_query_text(&mut retained_bytes, &topic)?;
                topic_counts.insert(topic, 1);
            }
        }
    }
    let mut top_topics = topic_counts
        .into_iter()
        .map(|(topic, count)| PolicyTopicSummary { topic, count })
        .collect::<Vec<_>>();
    top_topics.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.topic.cmp(&right.topic))
    });
    top_topics.truncate(5);

    Ok(PolicyPersonProfile {
        name: person_name,
        recent_meetings,
        open_intents,
        recent_decisions,
        top_topics,
    })
}

#[cfg(test)]
fn query_person_at(
    config: &Config,
    name: &str,
    path: &Path,
) -> Result<Option<PersonSummary>, GraphError> {
    let slug = slugify(name);
    let query_deadline = graph_operation_deadline();
    query_policy_fresh_graph_at(config, path, |conn| {
        let result = conn.query_row(
            "SELECT slug, name, meeting_count, last_seen FROM people WHERE slug = ?1",
            params![slug],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        );

        let (person_slug, person_name, meeting_count, last_seen) = match result {
            Ok(result) => result,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => return Err(error.into()),
        };

        let person_id: i64 = conn.query_row(
            "SELECT id FROM people WHERE slug = ?1",
            params![person_slug],
            |row| row.get(0),
        )?;

        let top_topics = top_topics_for_person(conn, person_id, 5, query_deadline)?;

        let open_commitments: i64 = conn.query_row(
            "SELECT COUNT(*) FROM commitments WHERE person_id = ?1 AND status IN ('open', 'stale')",
            params![person_id],
            |row| row.get(0),
        )?;

        let days_since = days_since_date(&last_seen);
        let score = relationship_score(meeting_count, days_since, top_topics.len());
        let losing_touch = meeting_count >= 3 && days_since > 21.0;

        Ok(Some(PersonSummary {
            slug: person_slug,
            name: person_name,
            meeting_count,
            last_seen,
            days_since,
            open_commitments,
            top_topics,
            score,
            losing_touch,
        }))
    })
}

#[cfg(test)]
fn query_commitments_at(
    config: &Config,
    person_slug: Option<&str>,
    path: &Path,
) -> Result<Vec<Commitment>, GraphError> {
    let query_deadline = graph_operation_deadline();
    query_policy_fresh_graph_at(config, path, |conn| {
        query_commitments_from_connection(conn, person_slug, MAX_GRAPH_QUERY_ROWS, query_deadline)
    })
}

fn resolve_commitment_person_selector(
    conn: &Connection,
    selector: &str,
    deadline: std::time::Instant,
) -> Result<Option<i64>, GraphError> {
    let normalized = normalized_graph_commitment_field(Some(selector));
    if normalized.is_empty() {
        return Ok(None);
    }
    let selector_slug = selector.trim();
    let mut stmt = conn.prepare("SELECT id, slug, name, aliases FROM people")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut matches = HashSet::new();
    let mut retained_bytes = 0usize;
    let mut scanned = 0usize;
    for row in rows {
        if std::time::Instant::now() >= deadline {
            return Err(graph_corpus_budget_error());
        }
        scanned += 1;
        if scanned > MAX_GRAPH_QUERY_ROWS {
            return Err(GraphError::Io(std::io::Error::other(
                "graph person-selector row budget exceeded",
            )));
        }
        let (id, slug, name, aliases_json) = row?;
        retain_graph_query_text(&mut retained_bytes, &slug)?;
        retain_graph_query_text(&mut retained_bytes, &name)?;
        retain_graph_query_text(&mut retained_bytes, &aliases_json)?;
        let aliases: Vec<String> = serde_json::from_str(&aliases_json).map_err(|_| {
            GraphError::Io(std::io::Error::other(
                "graph person aliases could not be verified",
            ))
        })?;
        if slug.eq_ignore_ascii_case(selector_slug)
            || normalized_graph_commitment_field(Some(&name)) == normalized
            || aliases
                .iter()
                .any(|alias| normalized_graph_commitment_field(Some(alias)) == normalized)
        {
            matches.insert(id);
        }
    }
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.into_iter().next()),
        _ => Err(GraphError::Io(std::io::Error::other(
            "graph person selector is ambiguous",
        ))),
    }
}

fn query_commitments_from_connection(
    conn: &Connection,
    person_slug: Option<&str>,
    limit: usize,
    query_deadline: std::time::Instant,
) -> Result<Vec<Commitment>, GraphError> {
    if limit > MAX_GRAPH_QUERY_ROWS {
        return Err(GraphError::Io(std::io::Error::other(
            "graph commitment result limit exceeded",
        )));
    }
    let person_id = match person_slug {
        Some(selector) => match resolve_commitment_person_selector(conn, selector, query_deadline)?
        {
            Some(id) => Some(id),
            None => return Ok(Vec::new()),
        },
        None => None,
    };
    let sql = if person_slug.is_some() {
        "SELECT c.text, c.status, c.due_date, c.created_at, c.commitment_type,
                    m.title, m.date, p.name
             FROM commitments c
             JOIN meetings m ON c.meeting_id = m.id
             LEFT JOIN people p ON c.person_id = p.id
             WHERE c.status IN ('open', 'stale') AND c.person_id = ?1
             ORDER BY m.date DESC, c.text ASC, p.name ASC, c.commitment_type ASC
             LIMIT ?2"
    } else {
        "SELECT c.text, c.status, c.due_date, c.created_at, c.commitment_type,
                    m.title, m.date, p.name
             FROM commitments c
             JOIN meetings m ON c.meeting_id = m.id
             LEFT JOIN people p ON c.person_id = p.id
             WHERE c.status IN ('open', 'stale')
             ORDER BY m.date DESC, c.text ASC, p.name ASC, c.commitment_type ASC
             LIMIT ?1"
    };

    let mut stmt = conn.prepare(sql)?;
    let rows = if let Some(person_id) = person_id {
        stmt.query_map(params![person_id, limit as i64], map_commitment)?
    } else {
        stmt.query_map(params![limit as i64], map_commitment)?
    };

    let mut commitments = Vec::new();
    let mut retained_bytes = 0usize;
    for row in rows {
        if std::time::Instant::now() >= query_deadline {
            return Err(graph_corpus_budget_error());
        }
        let commitment = row?;
        if commitments.len() >= MAX_GRAPH_QUERY_ROWS {
            return Err(GraphError::Io(std::io::Error::other(
                "graph commitment result budget exceeded",
            )));
        }
        retain_graph_query_text(&mut retained_bytes, &commitment.text)?;
        retain_graph_query_text(&mut retained_bytes, &commitment.status)?;
        retain_graph_query_text(&mut retained_bytes, &commitment.created_at)?;
        retain_graph_query_text(&mut retained_bytes, &commitment.commitment_type)?;
        retain_graph_query_text(&mut retained_bytes, &commitment.meeting_title)?;
        retain_graph_query_text(&mut retained_bytes, &commitment.meeting_date)?;
        if let Some(value) = &commitment.due_date {
            retain_graph_query_text(&mut retained_bytes, value)?;
        }
        if let Some(value) = &commitment.person_name {
            retain_graph_query_text(&mut retained_bytes, value)?;
        }
        commitments.push(commitment);
    }
    Ok(commitments)
}

fn query_commitments_for_person_slugs(
    conn: &Connection,
    selected: &HashSet<&str>,
    limit: usize,
    query_deadline: std::time::Instant,
) -> Result<Vec<Commitment>, GraphError> {
    if selected.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    if limit > MAX_GRAPH_QUERY_ROWS {
        return Err(GraphError::Io(std::io::Error::other(
            "graph selected commitment result limit exceeded",
        )));
    }
    let mut stmt = conn.prepare(
        "SELECT c.text, c.status, c.due_date, c.created_at, c.commitment_type,
                m.title, m.date, p.name, p.slug
         FROM commitments c
         JOIN meetings m ON c.meeting_id = m.id
         JOIN people p ON c.person_id = p.id
         WHERE c.status IN ('open', 'stale')",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((map_commitment(row)?, row.get::<_, String>(8)?))
    })?;
    let mut scanned = 0usize;
    let mut commitments = Vec::new();
    for row in rows {
        if std::time::Instant::now() >= query_deadline {
            return Err(graph_corpus_budget_error());
        }
        scanned = scanned
            .checked_add(1)
            .ok_or_else(graph_corpus_budget_error)?;
        if scanned > MAX_GRAPH_QUERY_ROWS {
            return Err(GraphError::Io(std::io::Error::other(
                "graph commitment scan budget exceeded",
            )));
        }
        let (commitment, slug) = row?;
        if !selected.contains(slug.as_str()) {
            continue;
        }
        retain_commitment_top_k(&mut commitments, commitment, limit);
    }
    let mut retained_bytes = 0usize;
    for commitment in &commitments {
        for value in [
            commitment.text.as_str(),
            commitment.status.as_str(),
            commitment.created_at.as_str(),
            commitment.commitment_type.as_str(),
            commitment.meeting_title.as_str(),
            commitment.meeting_date.as_str(),
        ] {
            retain_graph_query_text(&mut retained_bytes, value)?;
        }
        if let Some(value) = &commitment.due_date {
            retain_graph_query_text(&mut retained_bytes, value)?;
        }
        if let Some(value) = &commitment.person_name {
            retain_graph_query_text(&mut retained_bytes, value)?;
        }
    }
    Ok(commitments)
}

fn compare_commitments(left: &Commitment, right: &Commitment) -> std::cmp::Ordering {
    right
        .meeting_date
        .cmp(&left.meeting_date)
        .then_with(|| left.text.cmp(&right.text))
        .then_with(|| left.person_name.cmp(&right.person_name))
        .then_with(|| left.commitment_type.cmp(&right.commitment_type))
}

fn retain_commitment_top_k(retained: &mut Vec<Commitment>, candidate: Commitment, limit: usize) {
    let insertion = retained
        .binary_search_by(|existing| compare_commitments(existing, &candidate))
        .unwrap_or_else(|index| index);
    if insertion < limit {
        retained.insert(insertion, candidate);
        if retained.len() > limit {
            retained.pop();
        }
    }
}

fn map_commitment(row: &rusqlite::Row) -> rusqlite::Result<Commitment> {
    Ok(Commitment {
        text: row.get(0)?,
        status: row.get(1)?,
        due_date: row.get(2)?,
        created_at: row.get(3)?,
        commitment_type: row.get(4)?,
        meeting_title: row.get(5)?,
        meeting_date: row.get(6)?,
        person_name: row.get(7)?,
    })
}

/// Get all people with relationship scores — the relationship map.
#[cfg(test)]
pub(crate) fn relationship_map(config: &Config) -> Result<Vec<PersonSummary>, GraphError> {
    relationship_map_at(config, Path::new(""))
}

#[cfg(test)]
fn relationship_map_at(config: &Config, path: &Path) -> Result<Vec<PersonSummary>, GraphError> {
    let query_deadline = graph_operation_deadline();
    query_policy_fresh_graph_at(config, path, |conn| {
        relationship_map_from_connection(conn, query_deadline, false, None)
    })
}

fn relationship_map_from_connection(
    conn: &Connection,
    query_deadline: std::time::Instant,
    losing_only: bool,
    limit: Option<usize>,
) -> Result<Vec<PersonSummary>, GraphError> {
    let retained_limit = limit.unwrap_or(MAX_GRAPH_QUERY_ROWS);
    if retained_limit > MAX_GRAPH_QUERY_ROWS {
        return Err(GraphError::Io(std::io::Error::other(
            "graph relationship result limit exceeded",
        )));
    }
    if retained_limit == 0 {
        return Ok(Vec::new());
    }
    let sql = if losing_only {
        "SELECT p.id, p.slug, p.name, p.meeting_count, p.last_seen
         FROM people p WHERE p.meeting_count >= 3"
    } else {
        "SELECT p.id, p.slug, p.name, p.meeting_count, p.last_seen FROM people p WHERE p.meeting_count > 0"
    };
    let mut stmt = conn.prepare(sql)?;

    let mut people: Vec<PersonSummary> = Vec::with_capacity(retained_limit.min(256));
    let mut scanned = 0usize;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        if std::time::Instant::now() >= query_deadline {
            return Err(graph_corpus_budget_error());
        }
        scanned = scanned
            .checked_add(1)
            .ok_or_else(graph_corpus_budget_error)?;
        if scanned > MAX_GRAPH_QUERY_ROWS {
            return Err(GraphError::Io(std::io::Error::other(
                "graph relationship scan budget exceeded",
            )));
        }
        let person_id: i64 = row.get(0)?;
        let slug: String = row.get(1)?;
        let name: String = row.get(2)?;
        let meeting_count: i64 = row.get(3)?;
        let last_seen: String = row.get(4)?;
        let days_since = days_since_date(&last_seen);
        let losing_touch = meeting_count >= 3 && days_since > 21.0;
        if losing_only && !losing_touch {
            continue;
        }

        let top_topics = top_topics_for_person(conn, person_id, 3, query_deadline)?;

        let open_commitments: i64 = conn.query_row(
            "SELECT COUNT(*) FROM commitments WHERE person_id = ?1 AND status IN ('open', 'stale')",
            params![person_id],
            |row| row.get(0),
        )?;

        let topic_depth = (top_topics.len() as f64 / 3.0).min(1.0);
        let recency_weight = 1.0 / (1.0 + days_since / 30.0);
        let score = meeting_count as f64 * recency_weight * topic_depth;

        let candidate = PersonSummary {
            slug,
            name,
            meeting_count,
            last_seen,
            days_since,
            open_commitments,
            top_topics,
            score,
            losing_touch,
        };
        retain_person_top_k(&mut people, candidate, retained_limit);
    }
    let mut retained_bytes = 0usize;
    for person in &people {
        retain_graph_query_text(&mut retained_bytes, &person.slug)?;
        retain_graph_query_text(&mut retained_bytes, &person.name)?;
        retain_graph_query_text(&mut retained_bytes, &person.last_seen)?;
        for topic in &person.top_topics {
            retain_graph_query_text(&mut retained_bytes, topic)?;
        }
    }
    Ok(people)
}

fn compare_person_summaries(left: &PersonSummary, right: &PersonSummary) -> std::cmp::Ordering {
    right
        .score
        .partial_cmp(&left.score)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| right.meeting_count.cmp(&left.meeting_count))
        .then_with(|| left.slug.cmp(&right.slug))
        .then_with(|| left.name.cmp(&right.name))
}

fn retain_person_top_k(retained: &mut Vec<PersonSummary>, candidate: PersonSummary, limit: usize) {
    let insertion = retained
        .binary_search_by(|existing| compare_person_summaries(existing, &candidate))
        .unwrap_or_else(|index| index);
    if insertion < limit {
        retained.insert(insertion, candidate);
        if retained.len() > limit {
            retained.pop();
        }
    }
}

fn people_projection_from_connection(
    conn: &Connection,
    limit: usize,
    include_commitments: bool,
    stats: Option<GraphStats>,
    query_deadline: std::time::Instant,
) -> Result<PolicyPeopleProjection, GraphError> {
    if limit > MAX_GRAPH_QUERY_ROWS {
        return Err(GraphError::Io(std::io::Error::other(
            "graph people projection limit exceeded",
        )));
    }
    let people = relationship_map_from_connection(conn, query_deadline, false, Some(limit))?;
    let commitments = if include_commitments {
        let selected = people
            .iter()
            .map(|person| person.slug.as_str())
            .collect::<HashSet<_>>();
        query_commitments_for_person_slugs(conn, &selected, limit, query_deadline)?
    } else {
        Vec::new()
    };
    Ok(PolicyPeopleProjection {
        stats,
        people,
        commitments,
    })
}

/// Materialize the relationship map and open commitments from one attested
/// process-private projection. Multi-panel consumers should prefer this over
/// issuing two full-corpus graph reads.
#[cfg(test)]
pub(crate) fn relationship_context(config: &Config) -> Result<RelationshipContext, GraphError> {
    let query_deadline = graph_operation_deadline();
    query_policy_fresh_graph_at(config, Path::new(""), |conn| {
        Ok(RelationshipContext {
            people: relationship_map_from_connection(conn, query_deadline, false, None)?,
            commitments: query_commitments_from_connection(
                conn,
                None,
                MAX_GRAPH_QUERY_ROWS,
                query_deadline,
            )?,
        })
    })
}

/// Count meetings two people (by slug) both attended. Shared evidence for alias
/// suggestions/clusters.
fn shared_meeting_count(conn: &Connection, slug_a: &str, slug_b: &str) -> Result<i64, GraphError> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM people_meetings pm1
         JOIN people p1 ON pm1.person_id = p1.id
         JOIN people_meetings pm2 ON pm1.meeting_id = pm2.meeting_id
         JOIN people p2 ON pm2.person_id = p2.id
         WHERE p1.slug = ?1 AND p2.slug = ?2
           AND pm1.role IN ('attendee', 'speaker')
           AND pm2.role IN ('attendee', 'speaker')",
        params![slug_a, slug_b],
        |row| row.get(0),
    )?)
}

/// Detect people who might be the same person (fuzzy name matching).
fn detect_aliases(
    conn: &Connection,
    budget: &ActiveCorpusReadBudget,
    derived: &mut GraphDerivedBudget,
) -> Result<Vec<AliasSuggestion>, GraphError> {
    let mut stmt = conn.prepare("SELECT slug, name FROM people ORDER BY slug")?;
    let mut people = Vec::new();
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        if people.len() >= MAX_GRAPH_ALIAS_PEOPLE {
            return Err(GraphError::Io(std::io::Error::other(
                "graph alias candidate budget exceeded",
            )));
        }
        people.push(row?);
    }
    derived.consume(people.len(), budget)?;

    let mut suggestions = Vec::new();

    for i in 0..people.len() {
        for j in (i + 1)..people.len() {
            // Charge every attempted comparison, not only matches. Otherwise
            // a corpus of unrelated names can force quadratic CPU while
            // consuming almost none of the advertised derived-work budget.
            derived.consume(1, budget)?;
            let (slug_a, name_a) = &people[i];
            let (slug_b, name_b) = &people[j];

            if names_likely_same(name_a, name_b) {
                let shared = shared_meeting_count(conn, slug_a, slug_b)?;
                derived.consume_entity_text(name_a, budget)?;
                derived.consume_entity_text(name_b, budget)?;
                suggestions.push(AliasSuggestion {
                    name_a: name_a.clone(),
                    name_b: name_b.clone(),
                    shared_meetings: shared as usize,
                });
                derived.consume(1, budget)?;
            }
        }
    }

    Ok(suggestions)
}

/// Detect clusters of people who are plausibly the same person (issue #385).
///
/// Edges come from two predicates: the existing prefix/last-name
/// [`names_likely_same`] and the phonetic/edit variant predicate
/// [`crate::entity_cluster::names_plausibly_same_person`] (which finally links
/// spelling drift like `junrei`/`junlei`/`jun-rei` that the first cannot). Edges
/// are unioned transitively so a fragmented person surfaces as one cluster.
/// Suggestion only — nothing is merged.
fn detect_alias_clusters(
    conn: &Connection,
    budget: &ActiveCorpusReadBudget,
    derived: &mut GraphDerivedBudget,
) -> Result<Vec<AliasCluster>, GraphError> {
    // ORDER BY slug so cluster membership and ordering are stable across rebuilds
    // (SQLite row order otherwise follows insertion history). meeting_count is
    // carried so each cluster can put its highest-evidence spelling first — that
    // is the sensible default canonical for a suggested merge, rather than an
    // arbitrary alphabetical spelling that might be a typo (#385).
    let mut stmt = conn.prepare("SELECT slug, name, meeting_count FROM people ORDER BY slug")?;
    let mut people = Vec::new();
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        if people.len() >= MAX_GRAPH_ALIAS_PEOPLE {
            return Err(GraphError::Io(std::io::Error::other(
                "graph alias-cluster candidate budget exceeded",
            )));
        }
        people.push(row?);
    }
    derived.consume(people.len(), budget)?;

    // Clusters use ONLY the variant predicate (separator / same-first-letter
    // single-edit). The prefix/last-name predicate (`names_likely_same`) stays a
    // separate pairwise suggestion: mixing the two edge types in one transitive
    // union bridges distinct people (e.g. `Jon`~`Jan` by edit + `Jan`~`Jan Smith`
    // by prefix would fuse `Jon` and `Jan Smith`), violating "a wrong merge is
    // worse than a split" even at the suggestion level (#385).
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for i in 0..people.len() {
        for j in (i + 1)..people.len() {
            derived.consume(1, budget)?;
            if crate::entity_cluster::names_plausibly_same_person(&people[i].1, &people[j].1)
                .is_some()
            {
                edges.push((i, j));
                derived.consume(1, budget)?;
            }
        }
    }

    let clusters = crate::entity_cluster::cluster_indices_with_check(people.len(), &edges, || {
        budget.check_deadline().is_ok()
    })
    .ok_or_else(graph_corpus_budget_error)?;

    let mut result = Vec::with_capacity(clusters.len());
    for cluster in clusters {
        derived.consume(cluster.len().saturating_mul(2), budget)?;
        let mut max_shared = 0usize;
        for a in 0..cluster.len() {
            for b in (a + 1)..cluster.len() {
                derived.consume(1, budget)?;
                let shared =
                    shared_meeting_count(conn, &people[cluster[a]].0, &people[cluster[b]].0)?;
                max_shared = max_shared.max(shared as usize);
            }
        }
        // Order members by evidence (meeting_count desc, then slug for stability)
        // so members[0]/slugs[0] is the suggested canonical spelling.
        let mut ordered: Vec<usize> = cluster.clone();
        ordered.sort_by(|&a, &b| {
            people[b]
                .2
                .cmp(&people[a].2)
                .then_with(|| people[a].0.cmp(&people[b].0))
        });
        for &index in &ordered {
            derived.consume_entity_text(&people[index].1, budget)?;
            derived.consume_entity_text(&people[index].0, budget)?;
        }
        result.push(AliasCluster {
            members: ordered.iter().map(|&i| people[i].1.clone()).collect(),
            slugs: ordered.iter().map(|&i| people[i].0.clone()).collect(),
            max_shared_meetings: max_shared,
        });
    }

    Ok(result)
}

// ── Helpers ───────────────────────────────────────────────────

/// Fix common frontmatter issues before YAML parsing:
/// 1. Bare ISO dates without timezone offsets (e.g., `date: 2026-03-17T14:00:00`)
/// 2. Wikilink syntax in people field (e.g., `people: [[alex-chen], [mat]]`)
/// 3. Non-date strings in `due` fields (e.g., `due: Friday`)
fn fix_frontmatter(fm_str: &str) -> String {
    let offset = Local::now().format("%:z").to_string();
    fm_str
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            // Fix bare ISO dates
            if trimmed.starts_with("date:") && trimmed.len() > 5 {
                let value = trimmed[5..].trim();
                if value.contains('T')
                    && !value.contains('+')
                    && !value.contains('Z')
                    && value.chars().filter(|c| *c == '-').count() <= 2
                {
                    return format!("date: {}{}", value, offset);
                }
            }
            // Fix wikilinks in people field:
            // people: [[alex-chen], [mat]] → people: [alex-chen, mat]
            if trimmed.starts_with("people:") && trimmed.contains('[') {
                let colon_pos = line.find(':').unwrap_or(0);
                let key = &line[..=colon_pos];
                let value = line[colon_pos + 1..].replace(['[', ']'], "");
                let items: Vec<String> = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                return format!("{} [{}]", key, items.join(", "));
            }
            // Fix non-date due fields: quote them so they parse as strings
            if trimmed.starts_with("due:") && !trimmed.contains('"') {
                let value = trimmed[4..].trim();
                if !value.is_empty()
                    && !value.starts_with('"')
                    && !value
                        .chars()
                        .next()
                        .map(|c| c.is_ascii_digit())
                        .unwrap_or(false)
                {
                    let indent = line.len() - line.trim_start().len();
                    return format!("{}due: \"{}\"", " ".repeat(indent), value);
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Try parsing frontmatter with fixes applied for real-world format issues.
fn try_parse_with_fixed_date(fm_str: &str) -> Option<Frontmatter> {
    let fixed = fix_frontmatter(fm_str);
    serde_yaml::from_str(&fixed).ok()
}

fn graph_frontmatter_contains_yaml_alias(fm_str: &str) -> bool {
    for line in fm_str.lines() {
        let mut single_quoted = false;
        let mut double_quoted = false;
        let mut escaped = false;
        let mut skip_single_quote = false;
        let chars: Vec<char> = line.chars().collect();
        for (index, ch) in chars.iter().copied().enumerate() {
            if skip_single_quote {
                skip_single_quote = false;
                continue;
            }
            if double_quoted {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    double_quoted = false;
                }
                continue;
            }
            if single_quoted {
                if ch == '\'' {
                    if chars.get(index + 1) == Some(&'\'') {
                        skip_single_quote = true;
                        continue;
                    }
                    single_quoted = false;
                }
                continue;
            }
            match ch {
                '#' => break,
                '"' => double_quoted = true,
                '\'' => single_quoted = true,
                '*' => {
                    let token_boundary = index == 0
                        || chars[index - 1].is_whitespace()
                        || matches!(chars[index - 1], '[' | '{' | ',' | ':');
                    let has_alias_name = chars.get(index + 1).is_some_and(|next| {
                        !next.is_whitespace() && !matches!(next, ',' | ']' | '}' | '#')
                    });
                    if token_boundary && has_alias_name {
                        return true;
                    }
                }
                _ => {}
            }
        }
    }
    false
}

/// Parse frontmatter exactly as graph ingestion does. The fallback only repairs
/// legacy formatting; strict fields such as `sensitivity` retain their normal
/// serde validation in both passes.
fn parse_graph_frontmatter(fm_str: &str) -> Option<Frontmatter> {
    if fm_str.len() > MAX_GRAPH_FRONTMATTER_BYTES || graph_frontmatter_contains_yaml_alias(fm_str) {
        return None;
    }
    serde_yaml::from_str(fm_str)
        .ok()
        .or_else(|| try_parse_with_fixed_date(fm_str))
}

/// Slugify a name: "Sarah Chen" -> "sarah-chen"
fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn preferred_person_display_name(slug: &str, left: &str, right: &str) -> String {
    fn rank(slug: &str, value: &str) -> (bool, usize, usize) {
        (
            slugify(value) == slug,
            value.split_whitespace().count(),
            value.chars().count(),
        )
    }

    let left_rank = rank(slug, left);
    let right_rank = rank(slug, right);
    match left_rank.cmp(&right_rank) {
        std::cmp::Ordering::Greater => left.to_string(),
        std::cmp::Ordering::Less => right.to_string(),
        std::cmp::Ordering::Equal => {
            let left_key = left.to_lowercase();
            let right_key = right.to_lowercase();
            if (left_key, left) <= (right_key, right) {
                left.to_string()
            } else {
                right.to_string()
            }
        }
    }
}

/// Parse duration string like "5m 30s" or "1h 2m" into seconds.
fn parse_duration_secs(duration: &str) -> Option<i64> {
    let mut total = 0i64;
    let mut num_buf = String::new();
    for c in duration.chars() {
        if c.is_ascii_digit() {
            num_buf.push(c);
        } else if !num_buf.is_empty() {
            let n: i64 = num_buf.parse().unwrap_or(0);
            match c {
                'h' => total += n * 3600,
                'm' => total += n * 60,
                's' => total += n,
                _ => {}
            }
            num_buf.clear();
        }
    }
    if total > 0 {
        Some(total)
    } else {
        None
    }
}

/// Extract speaker names from transcript lines like "[SARAH 0:45]" or "[MAT 1:20]"
fn extract_speakers_from_transcript(
    body: &str,
    budget: &ActiveCorpusReadBudget,
    derived: &mut GraphDerivedBudget,
) -> Result<Vec<String>, GraphError> {
    let mut speakers: Vec<String> = Vec::new();
    for line in body.lines() {
        budget
            .check_deadline()
            .map_err(|_| graph_corpus_budget_error())?;
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('[') {
            if let Some(bracket_end) = rest.find(']') {
                let inside = &rest[..bracket_end];
                // Pattern: NAME followed by timestamp (H:MM or M:SS)
                if let Some(space_pos) = inside.rfind(' ') {
                    let name_part = inside[..space_pos].trim();
                    let time_part = inside[space_pos + 1..].trim();
                    if time_part.contains(':')
                        && time_part.chars().all(|c| c.is_ascii_digit() || c == ':')
                        && !name_part.is_empty()
                        && name_part.len() <= MAX_GRAPH_ENTITY_FIELD_BYTES
                    {
                        // Capitalize first letter of each word
                        let name = name_part
                            .split_whitespace()
                            .map(|w| {
                                let mut chars = w.chars();
                                match chars.next() {
                                    Some(first) => {
                                        first.to_uppercase().collect::<String>()
                                            + &chars.as_str().to_lowercase()
                                    }
                                    None => String::new(),
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" ");
                        if !speakers.contains(&name) {
                            derived.consume(1, budget)?;
                            derived.consume_entity_text(&name, budget)?;
                            speakers.push(name);
                        }
                    }
                }
            }
        }
    }
    Ok(speakers)
}

/// Extract lightweight commitments from transcript text patterns.
#[cfg(test)]
fn extract_commitments_from_transcript(
    body: &str,
    budget: &ActiveCorpusReadBudget,
    derived: &mut GraphDerivedBudget,
) -> Result<Vec<(String, String)>, GraphError> {
    let patterns = [
        "i'll send",
        "i will send",
        "let me follow up",
        "i'll follow up",
        "action item:",
        "todo:",
        "i'll get",
        "i will get",
        "let me check",
        "i'll look into",
    ];

    let mut commitments = Vec::new();
    for line in body.lines() {
        budget
            .check_deadline()
            .map_err(|_| graph_corpus_budget_error())?;
        if line.len() > MAX_GRAPH_ENTITY_FIELD_BYTES {
            continue;
        }
        let lower = line.trim().to_lowercase();
        for pattern in &patterns {
            if lower.contains(pattern) {
                // Clean up the line — remove speaker labels and timestamps
                let clean = line
                    .trim()
                    .trim_start_matches('[')
                    .split(']')
                    .next_back()
                    .unwrap_or(line.trim())
                    .trim();
                if clean.len() > 10 {
                    derived.consume(1, budget)?;
                    derived.consume_entity_text(clean, budget)?;
                    commitments.push((clean.to_string(), pattern.to_string()));
                    break;
                }
            }
        }
    }
    Ok(commitments)
}

/// Extract meaningful keywords from a meeting title.
fn extract_title_keywords(title: &str) -> Vec<String> {
    let stopwords = [
        "a",
        "an",
        "and",
        "as",
        "at",
        "by",
        "for",
        "from",
        "in",
        "of",
        "on",
        "or",
        "the",
        "to",
        "with",
        "we",
        "should",
        "will",
        "be",
        "is",
        "are",
        "use",
        "using",
        "meeting",
        "call",
        "sync",
        "chat",
        "discussion",
        "review",
        "update",
        "weekly",
        "daily",
        "standup",
    ];
    title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 3 && !stopwords.contains(w))
        .map(|w| w.to_string())
        .collect()
}

/// Check if two names likely refer to the same person.
/// "Sarah Chen" and "Sarah" → true (one is prefix of the other)
/// "Sarah" and "Sam" → false
fn names_likely_same(a: &str, b: &str) -> bool {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    if a_lower == b_lower {
        return false; // Same slug would have been deduped already
    }
    let a_parts: Vec<&str> = a_lower.split_whitespace().collect();
    let b_parts: Vec<&str> = b_lower.split_whitespace().collect();
    let a_first = a_parts.first().copied().unwrap_or("");
    let b_first = b_parts.first().copied().unwrap_or("");
    if a_first.is_empty() || b_first.is_empty() {
        return false;
    }
    // First names must match
    if a_first != b_first {
        return false;
    }
    // If BOTH have last names and they differ → different people
    // "Alex Chen" vs "Alex Kumar" → false (different last names)
    // "Alex Chen" vs "Alex" → true (one is a shortened form)
    if a_parts.len() >= 2 && b_parts.len() >= 2 {
        return a_parts[1] == b_parts[1]; // Same last name = likely same person
    }
    // One has a last name, the other doesn't → likely same person
    a_parts.len() != b_parts.len()
}

/// Calculate days since an RFC3339 date string.
fn days_since_date(date_str: &str) -> f64 {
    chrono::DateTime::parse_from_rfc3339(date_str)
        .map(|dt| {
            let now = Local::now();
            ((now.signed_duration_since(dt)).num_hours() as f64 / 24.0).max(0.0)
        })
        .unwrap_or(999.0)
}

// ── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn path_bearing_profile(path: PathBuf) -> PolicyProjectionResponse {
        PolicyProjectionResponse::PersonProfile(PolicyPersonProfile {
            name: "Synthetic Person".into(),
            recent_meetings: vec![PolicyMeetingReference {
                path: path.clone(),
                title: "Synthetic Meeting".into(),
                date: "2026-01-01".into(),
                content_type: "meeting".into(),
            }],
            open_intents: vec![PolicyIntentReference {
                path: path.clone(),
                title: "Synthetic Meeting".into(),
                date: "2026-01-01".into(),
                content_type: "meeting".into(),
                kind: IntentKind::Commitment,
                what: "Synthetic follow-up".into(),
                who: None,
                who_original: None,
                who_provenance: None,
                status: "open".into(),
                by_date: None,
            }],
            recent_decisions: vec![PolicyDecisionReference {
                path,
                title: "Synthetic Meeting".into(),
                date: "2026-01-01".into(),
                what: "Synthetic decision".into(),
                who: None,
                who_original: None,
                who_provenance: None,
                by_date: None,
                authority: None,
            }],
            top_topics: Vec::new(),
        })
    }

    #[test]
    fn profile_rehydration_rejects_unknown_or_replayed_source_ids_in_every_collection() {
        let opaque = PathBuf::from("/__minutes_graph_source/current/00000001.md");
        let replayed = PathBuf::from("/__minutes_graph_source/prior/00000001.md");
        let live = PathBuf::from("/live/synthetic.md");
        let allow = HashMap::from([(opaque.clone(), live.clone())]);

        let mut valid = path_bearing_profile(opaque.clone());
        rehydrate_policy_projection_paths(&mut valid, &allow).unwrap();
        let PolicyProjectionResponse::PersonProfile(valid) = valid else {
            unreachable!();
        };
        assert_eq!(valid.recent_meetings[0].path, live);
        assert_eq!(valid.open_intents[0].path, live);
        assert_eq!(valid.recent_decisions[0].path, live);

        for collection in 0..3 {
            let mut response = path_bearing_profile(opaque.clone());
            let PolicyProjectionResponse::PersonProfile(profile) = &mut response else {
                unreachable!();
            };
            match collection {
                0 => profile.recent_meetings[0].path = replayed.clone(),
                1 => profile.open_intents[0].path = replayed.clone(),
                2 => profile.recent_decisions[0].path = replayed.clone(),
                _ => unreachable!(),
            }
            let error = rehydrate_policy_projection_paths(&mut response, &allow).unwrap_err();
            assert!(error.to_string().contains("unknown source identifier"));
        }
    }

    #[test]
    fn private_graph_connections_force_memory_only_temporary_storage() {
        let conn = open_memory_db().unwrap();
        let mode: i64 = conn
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, 2);
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap();
        let max_page_count: i64 = conn
            .query_row("PRAGMA max_page_count", [], |row| row.get(0))
            .unwrap();
        let cache_size: i64 = conn
            .query_row("PRAGMA cache_size", [], |row| row.get(0))
            .unwrap();
        let temp_page_size: i64 = conn
            .query_row("PRAGMA temp.page_size", [], |row| row.get(0))
            .unwrap();
        let temp_max_page_count: i64 = conn
            .query_row("PRAGMA temp.max_page_count", [], |row| row.get(0))
            .unwrap();
        let temp_cache_size: i64 = conn
            .query_row("PRAGMA temp.cache_size", [], |row| row.get(0))
            .unwrap();
        assert_eq!(page_size, GRAPH_SQLITE_PAGE_BYTES);
        assert_eq!(max_page_count, MAX_GRAPH_SQLITE_PAGE_COUNT);
        assert_eq!(cache_size, -GRAPH_SQLITE_CACHE_KIB);
        assert_eq!(temp_page_size, GRAPH_SQLITE_PAGE_BYTES);
        assert_eq!(temp_max_page_count, MAX_GRAPH_SQLITE_TEMP_PAGE_COUNT);
        assert_eq!(temp_cache_size, -GRAPH_SQLITE_TEMP_CACHE_KIB);
        assert_eq!(
            conn.limit(Limit::SQLITE_LIMIT_LENGTH).unwrap(),
            MAX_GRAPH_SQLITE_VALUE_BYTES
        );
        assert_eq!(conn.limit(Limit::SQLITE_LIMIT_ATTACHED).unwrap(), 0);
        assert_eq!(conn.limit(Limit::SQLITE_LIMIT_WORKER_THREADS).unwrap(), 0);
    }

    #[test]
    fn graph_operation_uses_full_corpus_envelopes_per_attestation_pass() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut operation = GraphOperationBudget::new(deadline);
        let first = operation.next_pass().unwrap();
        consume_graph_corpus(&first, 4_096, 512, 80 * 1024 * 1024).unwrap();
        let second = operation.next_pass().unwrap();
        consume_graph_corpus(&second, 4_096, 512, 80 * 1024 * 1024).unwrap();
        for _ in 2..MAX_GRAPH_OPERATION_PASSES {
            operation.next_pass().unwrap();
        }
        assert!(operation.next_pass().is_err());
    }

    #[test]
    fn ordinary_graph_queries_do_not_delegate_payload_sorting_to_sqlite() {
        let conn = open_memory_db().unwrap();
        let queries = [
            "SELECT c.text, c.status, c.due_date, c.created_at, c.commitment_type, m.title, m.date, p.name FROM commitments c JOIN meetings m ON c.meeting_id = m.id LEFT JOIN people p ON c.person_id = p.id WHERE c.status IN ('open', 'stale')",
            "SELECT p.id, p.slug, p.name, p.meeting_count, p.last_seen FROM people p",
            "SELECT t.name FROM meeting_topics mt JOIN topics t ON mt.topic_id = t.id JOIN people_meetings pm ON pm.meeting_id = mt.meeting_id WHERE pm.person_id = 1",
            "SELECT slug, name FROM people ORDER BY slug",
            "SELECT COUNT(*) FROM people_meetings pm1 JOIN people p1 ON pm1.person_id = p1.id JOIN people_meetings pm2 ON pm1.meeting_id = pm2.meeting_id JOIN people p2 ON pm2.person_id = p2.id WHERE p1.slug = 'a' AND p2.slug = 'b'",
        ];
        for query in queries {
            let mut plan = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {query}"))
                .unwrap();
            let details = plan
                .query_map([], |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(
                details.iter().all(|detail| !detail.contains("TEMP B-TREE")),
                "ordinary graph query unexpectedly allocated a SQLite sorter: {details:?}"
            );
        }
    }

    #[test]
    fn private_graph_connection_rejects_an_oversized_sqlite_value() {
        let conn = open_memory_db().unwrap();
        let oversized = "x".repeat(MAX_GRAPH_SQLITE_VALUE_BYTES as usize + 1);
        let result = conn.execute(
            "INSERT INTO graph_metadata (key, value) VALUES ('oversized', ?1)",
            params![oversized],
        );
        assert!(result.is_err());
    }

    #[test]
    fn correction_reader_rejects_an_already_elapsed_local_deadline() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("vocabulary.toml");
        fs::write(&path, "").unwrap();
        let budget = ActiveCorpusReadBudget::new();
        assert!(read_stable_correction_file(
            &path,
            &budget,
            std::time::Instant::now() - std::time::Duration::from_millis(1),
        )
        .is_err());
    }

    #[test]
    fn graph_frontmatter_and_transcript_derivations_are_preallocation_bounded() {
        let oversized_frontmatter = format!(
            "title: Oversized\ntype: meeting\ndate: 2026-07-21T12:00:00Z\ncontext: {}",
            "x".repeat(MAX_GRAPH_FRONTMATTER_BYTES)
        );
        assert!(parse_graph_frontmatter(&oversized_frontmatter).is_none());
        let aliased = r#"title: Alias expansion
type: meeting
date: 2026-07-21T12:00:00Z
tags: &shared [planning]
people: [*shared]
"#;
        assert!(parse_graph_frontmatter(aliased).is_none());
        assert!(!graph_frontmatter_contains_yaml_alias(
            "title: \"Quoted *shared is plain text\"\ncontext: R&D\n"
        ));

        let budget = ActiveCorpusReadBudget::new();
        let mut derived = GraphDerivedBudget::default();
        let oversized_line = format!("I'll send {}", "x".repeat(MAX_GRAPH_ENTITY_FIELD_BYTES));
        let commitments =
            extract_commitments_from_transcript(&oversized_line, &budget, &mut derived).unwrap();
        assert!(commitments.is_empty());

        let mut retained = MAX_GRAPH_QUERY_RETAINED_BYTES - 1;
        retain_graph_query_text(&mut retained, "x").unwrap();
        assert!(retain_graph_query_text(&mut retained, "x").is_err());
    }

    fn test_config(dir: &Path) -> Config {
        Config {
            output_dir: dir.to_path_buf(),
            ..Config::default()
        }
    }

    /// Rebuild index into a temp db file (avoids test parallelism issues).
    fn rebuild_to_temp(config: &Config, tmp: &TempDir) -> GraphStats {
        let db = tmp.path().join("graph.db");
        rebuild_index_at(config, &db).unwrap()
    }

    fn write_meeting(dir: &Path, filename: &str, content: &str) {
        let path = dir.join(filename);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        fs::write(path, content).unwrap();
    }

    struct CorrectionEnv {
        old_home: Option<std::ffi::OsString>,
        old_minutes_home: Option<std::ffi::OsString>,
    }

    impl CorrectionEnv {
        fn install(home: &Path, minutes_home: &Path) -> Self {
            let old_home = std::env::var_os("HOME");
            let old_minutes_home = std::env::var_os("MINUTES_HOME");
            std::env::set_var("HOME", home);
            std::env::set_var("MINUTES_HOME", minutes_home);
            Self {
                old_home,
                old_minutes_home,
            }
        }
    }

    impl Drop for CorrectionEnv {
        fn drop(&mut self) {
            match self.old_home.take() {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match self.old_minutes_home.take() {
                Some(value) => std::env::set_var("MINUTES_HOME", value),
                None => std::env::remove_var("MINUTES_HOME"),
            }
        }
    }

    fn person_vocabulary(canonical: &str, aliases: &[&str]) -> crate::vocabulary::VocabularyStore {
        crate::vocabulary::VocabularyStore {
            entries: vec![crate::vocabulary::VocabularyEntry {
                kind: crate::vocabulary::VocabularyKind::Person,
                canonical: canonical.to_string(),
                aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
                ..crate::vocabulary::VocabularyEntry::default()
            }],
        }
    }

    const MEETING_1: &str = r#"---
title: Q2 Planning
type: meeting
date: 2026-03-20T14:00:00-07:00
duration: 42m
attendees: [Sarah Chen, Alex Kumar]
tags: [planning, roadmap]
action_items:
  - assignee: Alex Kumar
    task: Send tech spec
    due: "2026-03-25"
    status: open
decisions:
  - text: Use SQLite for the graph index
    topic: architecture
intents:
  - kind: commitment
    what: Review pricing grid
    who: Sarah Chen
    status: open
    by_date: "2026-03-22"
---

## Transcript
[SARAH 0:00] So for Q2, I think we should focus on the API
[ALEX 0:45] Right, I'll send the tech spec by Friday
[SARAH 1:20] Perfect, let me follow up on the pricing grid
"#;

    const MEETING_2: &str = r#"---
title: Product Sync
type: meeting
date: 2026-03-22T10:00:00-07:00
duration: 30m
attendees: [Sarah Chen, Jordan Mills]
tags: [product, pricing]
decisions:
  - text: Pricing must pass fairness test
    topic: pricing
---

## Transcript
[SARAH 0:00] Let's discuss the pricing updates
[JORDAN 0:30] I think we need to validate against competitors
"#;

    const MEETING_3: &str = r#"---
title: Onboarding Idea
type: memo
date: 2026-03-21T08:15:00-07:00
duration: 1m 22s
source: voice-memos
tags: [onboarding, product]
---

## Summary
Skip the wizard. Drop users into a pre-populated demo workspace.
"#;

    #[test]
    fn test_rebuild_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();

        // Override db_path for test
        let db = tmp.path().join("test.db");
        let conn = open_db(&db).unwrap();
        // Verify tables exist
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM people", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn production_graph_projection_creates_no_home_cache_or_sidecars() {
        let _guard = crate::test_home_env_lock();
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        let home = tmp.path().join("home");
        fs::create_dir_all(&meetings).unwrap();
        fs::create_dir_all(&home).unwrap();
        write_meeting(&meetings, "meeting.md", MEETING_1);
        let config = test_config(&meetings);
        let old_home = std::env::var_os("HOME");
        let old_minutes_home = std::env::var_os("MINUTES_HOME");
        std::env::set_var("HOME", &home);
        let state = home.join("isolated-minutes");
        std::env::set_var("MINUTES_HOME", &state);
        drop(crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&state).unwrap());
        let legacy = state.join("graph.db");
        fs::write(&legacy, b"PRIVATE-LEGACY-GRAPH-CANARY").unwrap();
        let legacy_holder = std::fs::File::open(&legacy).unwrap();

        rebuild_index(&config).unwrap();
        relationship_map(&config).unwrap();
        assert_eq!(legacy_holder.metadata().unwrap().len(), 0);

        for relative in [
            ".minutes/graph.db",
            ".minutes/graph.db-wal",
            ".minutes/graph.db-shm",
            "isolated-minutes/graph.db",
            "isolated-minutes/graph.db-wal",
            "isolated-minutes/graph.db-shm",
        ] {
            assert!(
                !home.join(relative).exists(),
                "created durable graph cache {relative}"
            );
        }

        match old_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match old_minutes_home {
            Some(value) => std::env::set_var("MINUTES_HOME", value),
            None => std::env::remove_var("MINUTES_HOME"),
        }
    }

    #[test]
    fn graph_projection_fails_closed_when_aggregate_corpus_budget_is_exceeded() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "meeting.md", MEETING_1);
        let config = test_config(&meetings);
        let graph_path = tmp.path().join("graph.db");
        let budget =
            ActiveCorpusReadBudget::for_test(10, 10, 32, std::time::Duration::from_secs(1));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut derived = GraphDerivedBudget::default();
        let corrections = graph_correction_snapshot(
            &GraphCorrectionPaths::beside_graph(&graph_path),
            &budget,
            &mut derived,
            deadline,
        )
        .unwrap();

        let error = match rebuild_in_memory_projection_with_hook(
            &config,
            &corrections,
            &budget,
            &mut derived,
            |_| {},
            false,
        ) {
            Ok(_) => panic!("oversized graph corpus unexpectedly produced a projection"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("resource budget"));
    }

    #[test]
    fn graph_projection_fails_closed_when_derived_item_budget_is_exceeded() {
        let budget = ActiveCorpusReadBudget::for_test(1, 1, 1, std::time::Duration::from_secs(1));
        let mut derived = GraphDerivedBudget::default();
        let error = derived
            .consume(MAX_GRAPH_DERIVED_ITEMS + 1, &budget)
            .unwrap_err();
        assert!(error.to_string().contains("item budget"));

        let mut paths = GraphDerivedBudget {
            retained_path_bytes: MAX_GRAPH_RETAINED_PATH_BYTES,
            ..Default::default()
        };
        let error = paths.consume_path(Path::new("x"), &budget).unwrap_err();
        assert!(error.to_string().contains("retained-path budget"));

        let mut strings = GraphDerivedBudget::default();
        let oversized = "x".repeat(MAX_GRAPH_ENTITY_FIELD_BYTES + 1);
        let error = strings
            .consume_entity_text(&oversized, &budget)
            .unwrap_err();
        assert!(error.to_string().contains("entity field budget"));
    }

    #[test]
    fn graph_revision_charges_every_traversed_non_markdown_entry() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("one.bin"), b"one").unwrap();
        fs::write(tmp.path().join("two.bin"), b"two").unwrap();
        let budget =
            ActiveCorpusReadBudget::for_test(1, 4, 1024, std::time::Duration::from_secs(1));
        let mut derived = GraphDerivedBudget::default();
        let error =
            graph_corpus_revision(&tmp.path().canonicalize().unwrap(), &budget, &mut derived)
                .unwrap_err();
        assert!(error.to_string().contains("resource budget"));
    }

    #[test]
    fn graph_projection_admission_fails_closed_instead_of_queueing_work() {
        static TEST_ADMISSION: Mutex<()> = Mutex::new(());
        let active = TEST_ADMISSION.lock().unwrap();
        let error = try_graph_projection_admission(&TEST_ADMISSION).unwrap_err();
        assert!(error.to_string().contains("already active"));
        drop(active);
        assert!(try_graph_projection_admission(&TEST_ADMISSION).is_ok());
    }

    #[test]
    fn graph_projection_uses_the_shared_cross_process_heap_lease() {
        let tmp = TempDir::new().unwrap();
        let corpus = tmp.path().join("meetings");
        fs::create_dir_all(&corpus).unwrap();
        let _in_process = GRAPH_PROJECTION_ADMISSION.lock().unwrap();
        let held =
            crate::policy_fs::acquire_private_corpus_projection_lease(&corpus, false).unwrap();
        assert!(crate::policy_fs::acquire_private_corpus_projection_lease(&corpus, false).is_err());
        drop(held);
        assert!(crate::policy_fs::acquire_private_corpus_projection_lease(&corpus, false).is_ok());
    }

    #[test]
    fn ordered_snapshot_journal_rejects_corpus_and_correction_mutations() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        let state = tmp.path().join("state");
        fs::create_dir_all(&meetings).unwrap();
        fs::create_dir_all(&state).unwrap();
        let corrections = GraphCorrectionPaths {
            vocabulary: state.join("vocabulary.toml"),
            overlays: state.join("overlays.db"),
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

        let mut correction_journal =
            GraphSnapshotJournal::begin(&meetings, &corrections, deadline).unwrap();
        fs::write(
            &corrections.vocabulary,
            b"[[entries]]\nkind='person'\ncanonical='Avery'\n",
        )
        .unwrap();
        assert!(correction_journal.checkpoint("correction-mutated").is_err());

        let mut corpus_journal =
            GraphSnapshotJournal::begin(&meetings, &corrections, deadline).unwrap();
        fs::write(meetings.join("new.md"), b"---\ntitle: Changed\n---\n").unwrap();
        assert!(corpus_journal.checkpoint("corpus-mutated").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ordered_snapshot_kqueue_rejects_transient_write_and_rename_aba() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        let state = tmp.path().join("state");
        fs::create_dir_all(&meetings).unwrap();
        fs::create_dir_all(&state).unwrap();
        let meeting = meetings.join("meeting.md");
        let original = b"---\ntitle: Original\n---\n";
        fs::write(&meeting, original).unwrap();
        let corrections = GraphCorrectionPaths {
            vocabulary: state.join("vocabulary.toml"),
            overlays: state.join("overlays.db"),
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);

        let mut write_aba = GraphSnapshotJournal::begin(&meetings, &corrections, deadline).unwrap();
        fs::write(&meeting, b"---\ntitle: Transient\n---\n").unwrap();
        fs::write(&meeting, original).unwrap();
        assert!(write_aba.checkpoint("write-aba").is_err());

        let mut rename_aba =
            GraphSnapshotJournal::begin(&meetings, &corrections, deadline).unwrap();
        let displaced = meetings.join("displaced.md");
        fs::rename(&meeting, &displaced).unwrap();
        fs::rename(&displaced, &meeting).unwrap();
        assert!(rename_aba.checkpoint("rename-aba").is_err());

        let vocabulary_original = b"version = 1\n";
        fs::write(&corrections.vocabulary, vocabulary_original).unwrap();
        let mut correction_write_aba =
            GraphSnapshotJournal::begin(&meetings, &corrections, deadline).unwrap();
        fs::write(&corrections.vocabulary, b"version = 2\n").unwrap();
        fs::write(&corrections.vocabulary, vocabulary_original).unwrap();
        assert!(correction_write_aba
            .checkpoint("correction-write-aba")
            .is_err());

        let mut correction_rename_aba =
            GraphSnapshotJournal::begin(&meetings, &corrections, deadline).unwrap();
        let displaced_vocabulary = state.join("vocabulary-displaced.toml");
        fs::rename(&corrections.vocabulary, &displaced_vocabulary).unwrap();
        fs::rename(&displaced_vocabulary, &corrections.vocabulary).unwrap();
        assert!(correction_rename_aba
            .checkpoint("correction-rename-aba")
            .is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ordered_snapshot_kqueue_ignores_unrelated_ancestor_sibling_activity() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        let state = tmp.path().join("state");
        fs::create_dir_all(&meetings).unwrap();
        fs::create_dir_all(&state).unwrap();
        let corrections = GraphCorrectionPaths {
            vocabulary: state.join("vocabulary.toml"),
            overlays: state.join("overlays.db"),
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut journal = GraphSnapshotJournal::begin(&meetings, &corrections, deadline).unwrap();

        fs::write(tmp.path().join("unrelated.txt"), b"unrelated").unwrap();
        journal.checkpoint("unrelated-ancestor-sibling").unwrap();
    }

    #[test]
    fn ordered_snapshot_rescan_can_never_acknowledge_a_fence() {
        #[cfg(not(windows))]
        {
            let fence = PathBuf::from("/tmp/minutes-state/fence");
            let event = notify::Event::new(NotifyEventKind::Other)
                .add_path(fence)
                .set_flag(notify::event::Flag::Rescan);
            assert!(matches!(
                classify_graph_notify_event(Ok(event)),
                Some(GraphJournalEvent::Overflow)
            ));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn ordered_snapshot_watches_every_rename_capable_ancestor() {
        let corpus = PathBuf::from("/tmp/minutes-nested/work/project/meetings");
        let corrections = PathBuf::from("/tmp/minutes-nested/state/private");
        let watches = non_windows_graph_watch_specs(&corpus, &corrections);
        for required in [
            Path::new("/tmp/minutes-nested/work"),
            Path::new("/tmp/minutes-nested/work/project"),
            Path::new("/tmp/minutes-nested/state"),
        ] {
            assert!(watches.iter().any(|(path, mode)| {
                path == required && matches!(mode, RecursiveMode::NonRecursive)
            }));
        }
    }

    #[test]
    fn ordered_snapshot_ignores_unrelated_state_activity() {
        let fixture_root = std::env::temp_dir().join("minutes-graph-event-filter-test");
        let corpus = fixture_root.join("minutes-corpus");
        let corrections = fixture_root.join("minutes-state");
        let vocabulary = corrections.join("vocabulary.toml");
        let overlays = corrections.join("overlays.db");
        let fences = corrections.join(GRAPH_SNAPSHOT_FENCE_DIRECTORY);
        let unrelated = vec![corrections.join("jobs/processing.json")];
        assert!(!graph_snapshot_event_affects_inputs(
            &unrelated,
            &corpus,
            &corrections,
            &vocabulary,
            &overlays,
            &[fences.as_path()],
        ));

        for path in [
            vocabulary,
            overlays.with_file_name("overlays.db-wal"),
            overlays.with_file_name("overlays.db-shm"),
            overlays.with_file_name("overlays.db-journal"),
        ] {
            let mutation = vec![path];
            assert!(graph_snapshot_event_affects_inputs(
                &mutation,
                &corpus,
                &corrections,
                &corrections.join("vocabulary.toml"),
                &corrections.join("overlays.db"),
                &[fences.as_path()],
            ));
        }
    }

    #[test]
    fn production_graph_applies_saved_vocabulary_and_bound_speaker_confirmation() {
        let _guard = crate::test_home_env_lock();
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        let home = tmp.path().join("home");
        let state = home.join("configured-minutes-state");
        fs::create_dir_all(&meetings).unwrap();
        fs::create_dir_all(&home).unwrap();
        let _env = CorrectionEnv::install(&home, &state);

        let variant_a = r#"---
title: Variant A
type: meeting
date: 2026-07-10T10:00:00Z
sensitivity: normal
attendees: [Junlei]
---
Notes.
"#;
        let variant_b = variant_a
            .replace("Variant A", "Variant B")
            .replace("Junlei", "Jun-Rei");
        write_meeting(&meetings, "variant-a.md", variant_a);
        write_meeting(&meetings, "variant-b.md", &variant_b);
        let speaker = meetings.join("speaker.md");
        let speaker_source = r#"---
title: Speaker Review
type: meeting
date: 2026-07-11T10:00:00Z
sensitivity: normal
attendees: []
speaker_map:
  - speaker_label: SPEAKER_0
    name: Unknown Speaker
    confidence: medium
    source: llm
intents:
  - kind: commitment
    what: Send the overlay-confirmed follow-up
    who: SPEAKER_0
    status: open
---
## Transcript
[SPEAKER_0 0:00] Bound correction proof.
"#;
        fs::write(&speaker, speaker_source).unwrap();
        crate::vocabulary::save_at(
            &crate::vocabulary::default_path(),
            &person_vocabulary("Junrei", &["Junlei", "Jun-Rei"]),
        )
        .unwrap();
        crate::overlays::write_speaker_confirmation(
            &speaker,
            "SPEAKER_0",
            "Alex Kim",
            Some("Unknown Speaker"),
            Some("production correction proof"),
        )
        .unwrap();

        let config = test_config(&meetings);
        let stats = rebuild_index(&config).unwrap();
        assert_eq!(
            stats.people_count, 2,
            "canonical person plus confirmed speaker"
        );
        let people = relationship_map(&config).unwrap();
        assert_eq!(
            people
                .iter()
                .filter(|person| person.slug == "junrei")
                .count(),
            1,
            "saved vocabulary must collapse both variants in production"
        );
        assert!(people.iter().any(|person| person.name == "Alex Kim"));
        assert!(people
            .iter()
            .all(|person| { person.slug != "speaker-0" && person.name != "Unknown Speaker" }));
        assert!(parakeet_boost_phrases(&config, 100)
            .unwrap()
            .iter()
            .any(|phrase| phrase == "Alex Kim"));
        assert_eq!(fs::read_to_string(&speaker).unwrap(), speaker_source);
        assert_eq!(
            fs::read_to_string(meetings.join("variant-a.md")).unwrap(),
            variant_a
        );
        assert_eq!(
            fs::read_to_string(meetings.join("variant-b.md")).unwrap(),
            variant_b
        );
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(!PathBuf::from(format!(
                "{}{suffix}",
                crate::overlays::default_db_path().display()
            ))
            .exists());
        }
    }

    #[test]
    fn production_graph_rejects_path_replayed_or_restricted_speaker_corrections() {
        let _guard = crate::test_home_env_lock();
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        let home = tmp.path().join("home");
        let state = home.join("configured-minutes-state");
        fs::create_dir_all(&meetings).unwrap();
        fs::create_dir_all(&home).unwrap();
        let _env = CorrectionEnv::install(&home, &state);
        let source = meetings.join("same-path.md");
        let restricted_a = r#"---
title: Restricted A
type: meeting
date: 2026-07-12T10:00:00Z
sensitivity: restricted
attendees: []
---
[SPEAKER_0 0:00] Restricted source.
"#;
        fs::write(&source, restricted_a).unwrap();
        crate::overlays::write_speaker_confirmation(
            &source,
            "SPEAKER_0",
            "RESTRICTED-ALICE",
            None,
            None,
        )
        .unwrap();
        let normal_b = r#"---
title: Unrelated Normal B
type: meeting
date: 2026-07-13T10:00:00Z
sensitivity: normal
attendees: []
---
[SPEAKER_0 0:00] Unrelated replacement.
"#;
        fs::write(&source, normal_b).unwrap();
        let config = test_config(&meetings);
        assert!(relationship_map(&config)
            .unwrap()
            .iter()
            .all(|person| person.name != "RESTRICTED-ALICE"));
        assert!(parakeet_boost_phrases(&config, 100)
            .unwrap()
            .iter()
            .all(|phrase| phrase != "RESTRICTED-ALICE"));
        assert_eq!(fs::read_to_string(&source).unwrap(), normal_b);

        crate::overlays::write_speaker_confirmation(
            &source,
            "SPEAKER_0",
            "Normal Bound Alice",
            None,
            None,
        )
        .unwrap();
        fs::write(
            &source,
            normal_b.replace("sensitivity: normal", "sensitivity: restricted"),
        )
        .unwrap();
        assert!(relationship_map(&config)
            .unwrap()
            .iter()
            .all(|person| person.name != "Normal Bound Alice"));
    }

    #[test]
    fn correction_flip_between_projection_and_sql_cannot_escape() {
        use std::cell::Cell;

        let _guard = crate::test_home_env_lock();
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        let home = tmp.path().join("home");
        let state = home.join("configured-minutes-state");
        fs::create_dir_all(&meetings).unwrap();
        fs::create_dir_all(&home).unwrap();
        let _env = CorrectionEnv::install(&home, &state);
        let source = meetings.join("race.md");
        fs::write(
            &source,
            r#"---
title: Correction Race
type: meeting
date: 2026-07-14T10:00:00Z
sensitivity: normal
attendees: [Vocab Alias]
---
[SPEAKER_0 0:00] Race.
"#,
        )
        .unwrap();
        crate::overlays::write_speaker_confirmation(
            &source,
            "SPEAKER_0",
            "Old Overlay Canary",
            None,
            None,
        )
        .unwrap();
        let config = test_config(&meetings);
        let flipped = Cell::new(false);
        let stale_escaped = query_policy_fresh_graph_at_with_hooks(
            &config,
            Path::new(""),
            |conn| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM people WHERE name = 'Old Overlay Canary')",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(GraphError::from)
            },
            |_| {},
            || {
                if !flipped.replace(true) {
                    crate::overlays::write_speaker_confirmation(
                        &source,
                        "SPEAKER_0",
                        "New Overlay Canonical",
                        None,
                        None,
                    )
                    .unwrap();
                }
            },
        )
        .unwrap();
        assert!(flipped.get());
        assert!(!stale_escaped, "stale correction-derived result escaped");

        crate::vocabulary::save_at(
            &crate::vocabulary::default_path(),
            &person_vocabulary("Old Vocabulary Canonical", &["Vocab Alias"]),
        )
        .unwrap();
        let vocabulary_flipped = Cell::new(false);
        let old_vocabulary_escaped = query_policy_fresh_graph_at_with_hooks(
            &config,
            Path::new(""),
            |conn| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM people WHERE slug = 'old-vocabulary-canonical')",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(GraphError::from)
            },
            |_| {},
            || {
                if !vocabulary_flipped.replace(true) {
                    crate::vocabulary::save_at(
                        &crate::vocabulary::default_path(),
                        &person_vocabulary("New Vocabulary Canonical", &["Vocab Alias"]),
                    )
                    .unwrap();
                }
            },
        )
        .unwrap();
        assert!(vocabulary_flipped.get());
        assert!(
            !old_vocabulary_escaped,
            "stale vocabulary-derived result escaped"
        );
    }

    #[test]
    fn test_rebuild_single_meeting() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);

        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);
        assert!(stats.people_count >= 2); // Sarah + Alex (from attendees + transcript)
        assert_eq!(stats.meeting_count, 1);
        assert_eq!(stats.commitment_count, 2); // explicit action item + explicit commitment only
    }

    #[test]
    fn rebuild_excludes_restricted_meetings() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);
        // Restricted meeting with a unique attendee + decision. Sensitivity
        // enforcement must keep ALL of it out of the graph, not just label it.
        let restricted = r#"---
title: Confidential Board Session
type: meeting
date: 2026-03-23T16:00:00-07:00
duration: 20m
capture: none
sensitivity: restricted
attendees: [Zelda Secretholder]
decisions:
  - text: Hold the undisclosed pricing floor
    topic: pricing
---

## Notes
- [0:01] Confidential board discussion.
"#;
        write_meeting(&meetings, "board.md", restricted);

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        let stats = rebuild_index_at(&config, &db).unwrap();

        // Only the non-restricted meeting is indexed.
        assert_eq!(stats.meeting_count, 1);

        // The restricted meeting's unique attendee never enters the people table.
        let conn = open_db(&db).unwrap();
        let leaked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE name LIKE '%Secretholder%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leaked, 0, "restricted attendee leaked into the graph");
    }

    #[test]
    fn rebuild_drops_present_but_invalid_sensitivity_values() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);

        for (suffix, sensitivity) in [
            ("null", "null"),
            ("empty", ""),
            ("unknown", "confidential"),
            ("list", "[normal]"),
            ("map", "{policy: normal}"),
        ] {
            let content = format!(
                "---\ntitle: Invalid {suffix}\ntype: meeting\ndate: 2026-03-23T16:00:00-07:00\nsensitivity: {sensitivity}\nattendees: [Policy Uncertain {suffix}]\n---\n\nPOLICY_UNCERTAIN_GRAPH_CANARY\n"
            );
            write_meeting(&meetings, &format!("invalid-{suffix}.md"), &content);
        }

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        let stats = rebuild_index_at(&config, &db).unwrap();
        assert_eq!(stats.meeting_count, 1);

        let conn = open_db(&db).unwrap();
        let leaked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE name LIKE 'Policy Uncertain%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(leaked, 0, "policy-uncertain facts leaked into graph");
    }

    #[test]
    fn rebuild_removes_facts_after_sensitivity_changes() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let path = meetings.join("policy-change.md");
        let normal = r#"---
title: Policy Change
type: meeting
date: 2026-03-23T16:00:00-07:00
duration: 20m
sensitivity: normal
attendees: [Policy Canary Person]
---

Normal meeting.
"#;
        fs::write(&path, normal).unwrap();
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();
        let conn = open_db(&db).unwrap();
        let before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE name = 'Policy Canary Person'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(before, 1);
        drop(conn);

        fs::write(
            &path,
            normal.replace("sensitivity: normal", "sensitivity: restricted"),
        )
        .unwrap();
        rebuild_index_at(&config, &db).unwrap();

        let conn = open_db(&db).unwrap();
        let after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE name = 'Policy Canary Person'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(after, 0, "stale restricted fact survived graph rebuild");
    }

    #[test]
    fn graph_reads_refresh_when_an_indexed_meeting_becomes_restricted() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting_path = meetings.join("policy-change.md");
        let db = tmp.path().join("graph.db");
        let config = test_config(&meetings);
        let normal = r#"---
title: Graph Policy Canary
type: meeting
date: 2026-03-23T16:00:00-07:00
duration: 20m
sensitivity: normal
attendees: [Graph Policy Canary Person]
tags: [GraphPolicyCanaryTopic]
action_items:
  - assignee: Graph Policy Canary Person
    task: Graph policy canary commitment
    status: open
---

Normal meeting.
"#;
        let restricted = normal.replace("sensitivity: normal", "sensitivity: restricted");

        let seed_stale_normal_graph = || {
            fs::write(&meeting_path, normal).unwrap();
            rebuild_index_at(&config, &db).unwrap();
            fs::write(&meeting_path, &restricted).unwrap();
        };

        seed_stale_normal_graph();
        assert!(query_person_at(&config, "Graph Policy Canary Person", &db)
            .unwrap()
            .is_none());

        seed_stale_normal_graph();
        assert!(query_commitments_at(&config, None, &db)
            .unwrap()
            .iter()
            .all(|item| !item.text.contains("Graph policy canary")));

        seed_stale_normal_graph();
        assert!(relationship_map_at(&config, &db)
            .unwrap()
            .iter()
            .all(|person| person.name != "Graph Policy Canary Person"));

        seed_stale_normal_graph();
        assert!(parakeet_boost_phrases_at(&config, 100, &db)
            .unwrap()
            .iter()
            .all(|phrase| !phrase.contains("Graph Policy Canary")));
    }

    fn graph_race_fixture(title: &str, person: &str, sensitivity: &str) -> String {
        format!(
            r#"---
title: {title}
type: meeting
date: 2026-03-23T16:00:00-07:00
duration: 20m
sensitivity: {sensitivity}
attendees: [{person}]
tags: [GraphRaceCanaryTopic]
action_items:
  - assignee: {person}
    task: Graph race canary commitment
    status: open
---

Graph race canary body.
"#
        )
    }

    fn graph_race_canary_present(conn: &Connection) -> Result<bool, GraphError> {
        let people: i64 = conn.query_row(
            "SELECT COUNT(*) FROM people WHERE name = 'Graph Race Canary Person'",
            [],
            |row| row.get(0),
        )?;
        let commitments: i64 = conn.query_row(
            "SELECT COUNT(*) FROM commitments WHERE text = 'Graph race canary commitment'",
            [],
            |row| row.get(0),
        )?;
        let topics: i64 = conn.query_row(
            "SELECT COUNT(*) FROM topics WHERE name = 'graphracecanarytopic'",
            [],
            |row| row.get(0),
        )?;
        let meetings: i64 = conn.query_row(
            "SELECT COUNT(*) FROM meetings WHERE title = 'Graph Race Canary Session'",
            [],
            |row| row.get(0),
        )?;
        Ok(people + commitments + topics + meetings > 0)
    }

    #[test]
    fn graph_query_discards_result_when_an_early_source_flips_during_policy_scan() {
        use std::cell::Cell;

        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let source_a = meetings.join("a-race.md");
        fs::write(
            &source_a,
            graph_race_fixture(
                "Graph Race Canary Session",
                "Graph Race Canary Person",
                "normal",
            ),
        )
        .unwrap();
        fs::write(
            meetings.join("b-stable.md"),
            corpus_binding_meeting("Stable Session", "Stable Person", "Stable commitment"),
        )
        .unwrap();
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let flipped = Cell::new(false);
        let canary_present = query_policy_fresh_graph_at_with_hooks(
            &config,
            &db,
            graph_race_canary_present,
            |verified_path| {
                if !flipped.get() && verified_path.file_name() == source_a.file_name() {
                    flipped.set(true);
                    fs::write(
                        &source_a,
                        graph_race_fixture(
                            "Graph Race Canary Session",
                            "Graph Race Canary Person",
                            "restricted",
                        ),
                    )
                    .unwrap();
                }
            },
            || {},
        )
        .unwrap();

        assert!(flipped.get(), "race hook did not run");
        assert!(
            !canary_present,
            "stale graph result escaped after policy flip"
        );
    }

    #[test]
    fn graph_query_discards_result_when_source_flips_after_precheck_before_sql() {
        use std::cell::Cell;

        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let source = meetings.join("race.md");
        fs::write(
            &source,
            graph_race_fixture(
                "Graph Race Canary Session",
                "Graph Race Canary Person",
                "normal",
            ),
        )
        .unwrap();
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let flipped = Cell::new(false);
        let canary_present = query_policy_fresh_graph_at_with_hooks(
            &config,
            &db,
            graph_race_canary_present,
            |_| {},
            || {
                if !flipped.replace(true) {
                    fs::write(
                        &source,
                        graph_race_fixture(
                            "Graph Race Canary Session",
                            "Graph Race Canary Person",
                            "restricted",
                        ),
                    )
                    .unwrap();
                }
            },
        )
        .unwrap();

        assert!(flipped.get(), "pre-query race hook did not run");
        assert!(
            !canary_present,
            "stale graph result escaped after pre-query flip"
        );
    }

    #[test]
    fn graph_rebuild_does_not_publish_stats_when_source_flips_after_snapshot() {
        use std::cell::Cell;

        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let source = meetings.join("race.md");
        fs::write(
            &source,
            graph_race_fixture(
                "Graph Race Canary Session",
                "Graph Race Canary Person",
                "normal",
            ),
        )
        .unwrap();
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        let flipped = Cell::new(false);

        let error = rebuild_index_at_with_vocabulary_entities_and_hook(
            &config,
            &db,
            Vec::new(),
            |verified_path| {
                if !flipped.replace(true) {
                    assert_eq!(verified_path.file_name(), source.file_name());
                    fs::write(
                        &source,
                        graph_race_fixture(
                            "Graph Race Canary Session",
                            "Graph Race Canary Person",
                            "restricted",
                        ),
                    )
                    .unwrap();
                }
            },
        )
        .unwrap_err();

        assert!(flipped.get(), "rebuild race hook did not run");
        assert!(error.to_string().contains("corpus changed"));
        assert!(!error.to_string().contains("Graph Race Canary"));
        assert!(query_person_at(&config, "Graph Race Canary Person", &db)
            .unwrap()
            .is_none());
    }

    #[test]
    fn graph_excludes_every_inactive_corpus_directory() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        fs::write(
            meetings.join("live.md"),
            graph_race_fixture("Live Session", "Live Person", "normal"),
        )
        .unwrap();
        for directory in crate::markdown::INACTIVE_CORPUS_DIRS {
            let title = format!("Inactive {directory} Canary");
            let person = format!("Inactive {directory} Person");
            write_meeting(
                &meetings.join(directory),
                "inactive.md",
                &graph_race_fixture(&title, &person, "normal"),
            );
        }

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        let stats = rebuild_index_at(&config, &db).unwrap();
        assert_eq!(stats.meeting_count, 1);

        let people = relationship_map_at(&config, &db).unwrap();
        assert!(people.iter().any(|person| person.name == "Live Person"));
        assert!(people
            .iter()
            .all(|person| !person.name.starts_with("Inactive ")));

        let commitments = query_commitments_at(&config, None, &db).unwrap();
        assert_eq!(commitments.len(), 1);
        let phrases = parakeet_boost_phrases_at(&config, 100, &db).unwrap();
        assert!(phrases
            .iter()
            .all(|phrase| !phrase.starts_with("Inactive ")));
        for directory in crate::markdown::INACTIVE_CORPUS_DIRS {
            assert!(
                query_person_at(&config, &format!("Inactive {directory} Person"), &db)
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn graph_reads_rebind_a_shared_cache_when_output_dir_changes() {
        let tmp = TempDir::new().unwrap();
        let root_a = tmp.path().join("meetings-a");
        let root_b = tmp.path().join("meetings-b");
        fs::create_dir_all(&root_a).unwrap();
        fs::create_dir_all(&root_b).unwrap();
        write_meeting(
            &root_a,
            "root-a.md",
            &corpus_binding_meeting("Root A Session", "Root A Person", "Root A commitment"),
        );
        write_meeting(
            &root_b,
            "root-b.md",
            &corpus_binding_meeting("Root B Session", "Root B Person", "Root B commitment"),
        );
        let config_a = test_config(&root_a);
        let config_b = test_config(&root_b);
        let db = tmp.path().join("graph.db");

        rebuild_index_at(&config_a, &db).unwrap();
        assert!(query_person_at(&config_b, "Root A Person", &db)
            .unwrap()
            .is_none());
        assert!(query_person_at(&config_b, "Root B Person", &db)
            .unwrap()
            .is_some());

        rebuild_index_at(&config_a, &db).unwrap();
        let commitments = query_commitments_at(&config_b, None, &db).unwrap();
        assert!(commitments
            .iter()
            .any(|item| item.text == "Root B commitment"));
        assert!(commitments
            .iter()
            .all(|item| item.text != "Root A commitment"));

        rebuild_index_at(&config_a, &db).unwrap();
        let people = relationship_map_at(&config_b, &db).unwrap();
        assert!(people.iter().any(|person| person.name == "Root B Person"));
        assert!(people.iter().all(|person| person.name != "Root A Person"));

        rebuild_index_at(&config_a, &db).unwrap();
        let phrases = parakeet_boost_phrases_at(&config_b, 100, &db).unwrap();
        assert!(phrases.iter().any(|phrase| phrase.contains("Root B")));
        assert!(phrases.iter().all(|phrase| !phrase.contains("Root A")));
    }

    #[test]
    fn graph_queries_ignore_tampered_persistent_rows_and_colocated_metadata() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(
            &meetings,
            "live.md",
            &corpus_binding_meeting("Live Session", "Live Person", "Live commitment"),
        );
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let conn = open_db(&db).unwrap();
        conn.execute(
            "INSERT INTO people (slug, name, first_seen, last_seen, meeting_count)
             VALUES ('tampered-canary', 'Tampered Graph Canary', '2026-01-01', '2026-01-01', 99)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO meetings (path, title, date, content_type)
             VALUES ('/tampered/canary.md', 'Tampered Session', '2026-01-01', 'meeting')",
            [],
        )
        .unwrap();
        let meeting_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO commitments
             (meeting_id, person_id, text, status, created_at, commitment_type)
             VALUES (?1, NULL, 'Tampered commitment canary', 'open', '2026-01-01', 'intent')",
            params![meeting_id],
        )
        .unwrap();
        let mut derived = GraphDerivedBudget::default();
        let live_revision = graph_corpus_revision(
            &meetings.canonicalize().unwrap(),
            &ActiveCorpusReadBudget::new(),
            &mut derived,
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO graph_metadata (key, value) VALUES (?1, ?2)",
            params![GRAPH_CORPUS_REVISION_KEY, live_revision],
        )
        .unwrap();
        drop(conn);

        assert!(query_person_at(&config, "Tampered Graph Canary", &db)
            .unwrap()
            .is_none());
        assert!(query_commitments_at(&config, None, &db)
            .unwrap()
            .iter()
            .all(|commitment| !commitment.text.contains("Tampered")));
        assert!(relationship_map_at(&config, &db)
            .unwrap()
            .iter()
            .all(|person| !person.name.contains("Tampered")));
        assert!(parakeet_boost_phrases_at(&config, 100, &db)
            .unwrap()
            .iter()
            .all(|phrase| !phrase.contains("Tampered")));
    }

    fn corpus_binding_meeting(title: &str, person: &str, commitment: &str) -> String {
        format!(
            r#"---
title: {title}
type: meeting
date: 2026-03-23T16:00:00-07:00
duration: 20m
sensitivity: normal
attendees: [{person}]
action_items:
  - assignee: {person}
    task: {commitment}
    status: open
---

Corpus binding test.
"#
        )
    }

    #[cfg(unix)]
    #[test]
    fn rebuild_rejects_symlinked_meeting_outside_root() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&meetings).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let outside_file = outside.join("outside.md");
        fs::write(
            &outside_file,
            r#"---
title: Outside
type: meeting
date: 2026-03-23T16:00:00-07:00
duration: 20m
attendees: [Outside Symlink Canary]
---

Outside content.
"#,
        )
        .unwrap();
        symlink(&outside_file, meetings.join("linked.md")).unwrap();

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        let stats = rebuild_index_at(&config, &db).unwrap();

        assert_eq!(stats.meeting_count, 0);
        let conn = open_db(&db).unwrap();
        let leaked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE name = 'Outside Symlink Canary'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(leaked, 0);
    }

    #[test]
    fn rebuild_layers_speaker_overlays_without_rewriting_markdown() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = meetings.join("speaker.md");
        let content = r#"---
title: Speaker Review
type: meeting
date: 2026-03-20T14:00:00-07:00
duration: 10m
attendees: []
speaker_map:
  - speaker_label: SPEAKER_0
    name: Unknown Speaker
    confidence: medium
    source: llm
intents:
  - kind: commitment
    what: Send the overlay-confirmed follow-up
    who: SPEAKER_0
    status: open
---

## Transcript
[SPEAKER_0 0:00] I will send the follow-up.
"#;
        fs::write(&meeting, content).unwrap();

        let graph_db = tmp.path().join("graph.db");
        let overlay_db = crate::overlays::db_path_for_graph_path(&graph_db);
        crate::overlays::write_speaker_confirmation_at(
            &overlay_db,
            &meeting,
            "SPEAKER_0",
            "Alex Kim",
            Some("Unknown Speaker"),
            Some("test confirmation"),
        )
        .unwrap();

        let config = test_config(&meetings);
        rebuild_index_at(&config, &graph_db).unwrap();

        let conn = open_db(&graph_db).unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM people WHERE slug = 'alex-kim'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Alex Kim");
        let raw_speaker_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = 'speaker-0'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(raw_speaker_count, 0);
        let owner: String = conn
            .query_row(
                "SELECT p.name FROM commitments c JOIN people p ON p.id = c.person_id WHERE c.text = 'Send the overlay-confirmed follow-up'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(owner, "Alex Kim");
        assert_eq!(fs::read_to_string(&meeting).unwrap(), content);
    }

    #[test]
    fn test_rebuild_multiple_meetings() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);
        write_meeting(&meetings, "product-sync.md", MEETING_2);
        write_meeting(&meetings, "memos/onboarding.md", MEETING_3);

        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);
        assert!(stats.people_count >= 3); // Sarah, Alex, Jordan
        assert_eq!(stats.meeting_count, 3);
        assert!(stats.topic_count >= 3); // planning, roadmap, pricing, product, ...
    }

    #[test]
    fn test_rebuild_malformed_yaml() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "good.md", MEETING_1);
        write_meeting(&meetings, "bad.md", "---\ntitle: [invalid yaml\n---\nbody");

        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);
        assert_eq!(stats.meeting_count, 1); // Only the good file
    }

    #[test]
    fn test_extract_speakers_from_transcript() {
        let body =
            "[SARAH 0:00] Hello\n[ALEX 0:45] Hi there\n[SARAH 1:20] Let's begin\nNo bracket line";
        let budget = ActiveCorpusReadBudget::new();
        let mut derived = GraphDerivedBudget::default();
        let speakers = extract_speakers_from_transcript(body, &budget, &mut derived).unwrap();
        assert_eq!(speakers, vec!["Sarah", "Alex"]);
    }

    #[test]
    fn test_extract_speakers_empty() {
        let body = "Just plain text with no speaker labels.";
        let budget = ActiveCorpusReadBudget::new();
        let mut derived = GraphDerivedBudget::default();
        let speakers = extract_speakers_from_transcript(body, &budget, &mut derived).unwrap();
        assert!(speakers.is_empty());
    }

    #[test]
    fn test_extract_commitments_from_transcript() {
        let body = "[ALEX 0:45] Right, I'll send the tech spec by Friday\n[SARAH 1:20] Let me follow up on pricing";
        let budget = ActiveCorpusReadBudget::new();
        let mut derived = GraphDerivedBudget::default();
        let commitments = extract_commitments_from_transcript(body, &budget, &mut derived).unwrap();
        assert_eq!(commitments.len(), 2);
        assert!(commitments[0].0.contains("tech spec"));
        assert!(commitments[1].0.contains("pricing"));
    }

    #[test]
    fn test_extract_title_keywords() {
        let keywords = extract_title_keywords("Q2 Planning Discussion with Team");
        assert!(keywords.contains(&"planning".to_string()));
        assert!(!keywords.contains(&"with".to_string())); // stopword
    }

    #[test]
    fn test_names_likely_same() {
        assert!(names_likely_same("Sarah Chen", "Sarah"));
        assert!(names_likely_same("Sarah", "Sarah Chen"));
        assert!(!names_likely_same("Sarah", "Sam"));
        assert!(!names_likely_same("Sarah Chen", "Sarah Chen")); // exact match = already deduped
                                                                 // False positive fix: same first name, different last name = different people
        assert!(!names_likely_same("Alex Chen", "Alex Kumar"));
        assert!(!names_likely_same("Jordan Mills", "Jordan Lee"));
        // Both have same first + last (case insensitive) = same slug, already deduped
        assert!(!names_likely_same("Sarah Chen", "Sarah chen"));
        // Different last name initials are different people
        assert!(!names_likely_same("Sarah C.", "Sarah Chen"));
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Sarah Chen"), "sarah-chen");
        assert_eq!(slugify("Alex  Kumar"), "alex-kumar");
        assert_eq!(slugify("  Mat  "), "mat");
    }

    #[test]
    fn canonical_display_name_is_stable_across_reverse_traversal() {
        assert_eq!(
            preferred_person_display_name("avery-stone", "Avery", "Avery Stone"),
            "Avery Stone"
        );
        assert_eq!(
            preferred_person_display_name("avery-stone", "Avery Stone", "Avery"),
            "Avery Stone"
        );
        assert_eq!(
            preferred_person_display_name("alex", "Alex", "ALEX"),
            preferred_person_display_name("alex", "ALEX", "Alex")
        );
    }

    #[test]
    fn divergent_labels_union_into_one_stable_profile_and_selector() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        for (file, label, date) in [
            ("z-short.md", "Avery", "2026-07-01T12:00:00Z"),
            ("a-full.md", "Avery Stone", "2026-07-02T12:00:00Z"),
        ] {
            write_meeting(
                &meetings,
                file,
                &format!(
                    r#"---
title: Stable Identity
type: meeting
date: {date}
duration: 5m
attendees: [{label}]
entities:
  people:
    - slug: avery-stone
      label: {label}
      aliases: []
---
"#,
                ),
            );
        }
        let config = test_config(&meetings);
        let path = tmp.path().join("stable-identity.db");
        let people = relationship_map_at(&config, &path).unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].name, "Avery Stone");
        let deadline = graph_operation_deadline();
        let profile = query_policy_fresh_graph_at(&config, &path, |conn| {
            policy_person_profile_from_connection(conn, "Avery", deadline)
        })
        .unwrap();
        assert_eq!(profile.name, "Avery Stone");
        assert_eq!(profile.recent_meetings.len(), 2);
    }

    #[test]
    fn test_parse_duration_secs() {
        assert_eq!(parse_duration_secs("42m"), Some(2520));
        assert_eq!(parse_duration_secs("1h 2m"), Some(3720));
        assert_eq!(parse_duration_secs("5m 30s"), Some(330));
        assert_eq!(parse_duration_secs("1m 22s"), Some(82));
        assert_eq!(parse_duration_secs(""), None);
    }

    #[test]
    fn test_relationship_scoring() {
        // meeting_count=5, days_since=0, topic_depth=1.0 (3+ topics)
        let recency_weight = 1.0 / (1.0 + 0.0 / 30.0); // 1.0
        let topic_depth = (3.0_f64 / 3.0).min(1.0); // 1.0
        let score = 5.0 * recency_weight * topic_depth;
        assert!((score - 5.0).abs() < 0.001);

        // meeting_count=5, days_since=30, topic_depth=0.33 (1 topic)
        let recency_weight = 1.0 / (1.0 + 30.0 / 30.0); // 0.5
        let topic_depth = (1.0_f64 / 3.0).min(1.0); // 0.33
        let score = 5.0 * recency_weight * topic_depth;
        assert!(score < 1.0); // Decayed significantly
    }

    #[test]
    fn future_dates_are_clamped_to_today_for_finite_relationship_scores() {
        let future = (Local::now() + chrono::Duration::days(90)).to_rfc3339();
        let days = days_since_date(&future);
        assert_eq!(days, 0.0);
        assert!(relationship_score(3, days, 2).is_finite());
    }

    #[test]
    fn test_query_person_not_found() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let conn = open_db(&db).unwrap();
        let result = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = ?1",
                params!["nonexistent-person"],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_query_person_found() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);
        write_meeting(&meetings, "product-sync.md", MEETING_2);

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let conn = open_db(&db).unwrap();
        let (name, count): (String, i64) = conn
            .query_row(
                "SELECT name, meeting_count FROM people WHERE slug = 'sarah-chen'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "Sarah Chen");
        assert_eq!(count, 2);

        // Check open commitments
        let person_id: i64 = conn
            .query_row(
                "SELECT id FROM people WHERE slug = 'sarah-chen'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let open: i64 = conn.query_row(
            "SELECT COUNT(*) FROM commitments WHERE person_id = ?1 AND status IN ('open', 'stale')",
            params![person_id],
            |row| row.get(0),
        ).unwrap();
        assert!(open >= 1, "Sarah should have at least 1 open commitment");
    }

    #[test]
    fn test_query_commitments() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let conn = open_db(&db).unwrap();
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commitments WHERE status IN ('open', 'stale')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(total > 0, "Should have at least 1 open commitment");
    }

    #[test]
    fn graph_commitments_require_explicit_action_or_commitment_semantics() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(
            &meetings,
            "semantics.md",
            r#"---
title: Semantics
type: meeting
date: 2026-07-01T12:00:00Z
duration: 5m
attendees: [Avery Quinn]
entities:
  people:
    - slug: avery-quinn
      label: Avery Quinn
      aliases: [Avery, AQ]
action_items:
  - assignee: Avery
    task: Send the reviewed proposal
    due: "2026-07-10"
    status: open
intents:
  - kind: open-question
    what: Should this ship?
    status: open
  - kind: commitment
    what: Send the reviewed proposal
    who: Avery
    by_date: "2026-07-10"
    status: stale
decisions:
  - text: Use the reviewed approach
---

## Transcript

[Avery 00:01] Someone said I will maybe send a draft, but that was hearsay.
"#,
        );
        let config = test_config(&meetings);
        let commitments =
            query_commitments_at(&config, None, &tmp.path().join("graph-semantics.db")).unwrap();
        assert_eq!(commitments.len(), 1);
        assert_eq!(commitments[0].text, "Send the reviewed proposal");
        assert_eq!(commitments[0].status, "stale");
        for selector in ["avery-quinn", "Avery Quinn", "AQ"] {
            let selected = query_commitments_at(
                &config,
                Some(selector),
                &tmp.path().join("graph-semantics.db"),
            )
            .unwrap();
            assert_eq!(selected.len(), 1, "selector {selector} lost its commitment");
        }
    }

    #[test]
    fn test_relationship_map_ordering() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);
        write_meeting(&meetings, "product-sync.md", MEETING_2);
        write_meeting(&meetings, "memos/onboarding.md", MEETING_3);

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let conn = open_db(&db).unwrap();
        // Sarah appears in 2 meetings, should have highest meeting count
        let top: (String, i64) = conn
            .query_row(
                "SELECT name, meeting_count FROM people ORDER BY meeting_count DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(top.0, "Sarah Chen");
        assert_eq!(top.1, 2);
    }

    #[test]
    fn requested_projection_limits_bound_retained_top_k_storage() {
        let mut people = Vec::new();
        for index in 0..100 {
            retain_person_top_k(
                &mut people,
                PersonSummary {
                    slug: format!("person-{index}"),
                    name: format!("Person {index}"),
                    meeting_count: index,
                    last_seen: "2026-07-01T00:00:00Z".into(),
                    days_since: 1.0,
                    open_commitments: 0,
                    top_topics: Vec::new(),
                    score: index as f64,
                    losing_touch: false,
                },
                3,
            );
            assert!(people.len() <= 3);
        }
        assert_eq!(
            people.iter().map(|person| person.score).collect::<Vec<_>>(),
            [99.0, 98.0, 97.0]
        );

        let mut commitments = Vec::new();
        for index in 0..100 {
            retain_commitment_top_k(
                &mut commitments,
                Commitment {
                    text: format!("Commitment {index}"),
                    status: "open".into(),
                    due_date: None,
                    created_at: String::new(),
                    commitment_type: "commitment".into(),
                    meeting_title: "Synthetic".into(),
                    meeting_date: format!("2026-07-{:02}", index % 28 + 1),
                    person_name: None,
                },
                5,
            );
            assert!(commitments.len() <= 5);
        }
    }

    #[test]
    fn mentioned_entities_never_become_relationship_meetings_or_losing_touch() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        for index in 0..3 {
            write_meeting(
                &meetings,
                &format!("mention-{index}.md"),
                &format!(
                    r#"---
title: Mention {index}
type: meeting
date: 2020-01-0{}T12:00:00Z
duration: 5m
attendees: [Actual Contact]
people: [Discussed Person]
tags: [private-strategy]
entities:
  people:
    - slug: discussed-person
      label: Discussed Person
      aliases: []
decisions:
  - text: Do not attribute this meeting to the discussed person
---

## Transcript

[Actual Contact 00:01] We discussed a third party.
"#,
                    index + 1
                ),
            );
        }
        let config = test_config(&meetings);
        let path = tmp.path().join("mentions.db");
        let people = relationship_map_at(&config, &path).unwrap();
        assert!(people.iter().any(|person| person.name == "Actual Contact"));
        assert!(people
            .iter()
            .all(|person| person.name != "Discussed Person"));
        let deadline = graph_operation_deadline();
        let profile = query_policy_fresh_graph_at(&config, &path, |conn| {
            policy_person_profile_from_connection(conn, "Discussed Person", deadline)
        })
        .unwrap();
        assert!(profile.recent_meetings.is_empty());
        assert!(profile.recent_decisions.is_empty());
        assert!(profile.top_topics.is_empty());
    }

    #[test]
    fn commitment_owner_is_preserved_without_becoming_a_relationship_contact() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(
            &meetings,
            "owner-only.md",
            r#"---
title: Owner only
type: meeting
date: 2026-07-01T12:00:00Z
duration: 5m
action_items:
  - assignee: Casey Owner
    task: Send the owner-only plan
    status: open
---
"#,
        );
        let config = test_config(&meetings);
        let path = tmp.path().join("owner-only.db");
        let commitments = query_commitments_at(&config, Some("Casey Owner"), &path).unwrap();
        assert_eq!(commitments.len(), 1);
        assert_eq!(commitments[0].person_name.as_deref(), Some("Casey Owner"));
        assert!(relationship_map_at(&config, &path).unwrap().is_empty());
    }

    #[test]
    fn confirmed_speaker_labels_resolve_commitment_owners() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(
            &meetings,
            "confirmed-owner.md",
            r#"---
title: Confirmed owner
type: meeting
date: 2026-07-01T12:00:00Z
duration: 5m
speaker_map:
  - speaker_label: SPEAKER_0
    name: Alex Kim
    confidence: high
    source: manual
intents:
  - kind: commitment
    what: Send the confirmed follow-up
    who: SPEAKER_0
    status: open
---

## Transcript

[SPEAKER_0 00:01] I will send the follow-up.
"#,
        );
        let config = test_config(&meetings);
        let path = tmp.path().join("confirmed-owner.db");
        let commitments = query_commitments_at(&config, Some("Alex Kim"), &path).unwrap();
        assert_eq!(commitments.len(), 1);
        assert_eq!(commitments[0].person_name.as_deref(), Some("Alex Kim"));
        let alex = relationship_map_at(&config, &path)
            .unwrap()
            .into_iter()
            .find(|person| person.name == "Alex Kim")
            .unwrap();
        assert_eq!(alex.open_commitments, 1);
    }

    #[test]
    fn test_relationship_map_includes_attendees_raw_imports() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = r#"---
title: Imported Granola Meeting
type: meeting
date: 2026-03-24T09:00:00-07:00
duration: 25m
source: granola-import
attendees_raw: Alice Smith (alice@example.com), Bob Brown (bob@example.com)
---

## Notes

Imported notes only.
"#;
        write_meeting(&meetings, "granola.md", meeting);

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let conn = open_db(&db).unwrap();
        let names: Vec<String> = conn
            .prepare("SELECT name FROM people ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|row| row.ok())
            .collect();
        assert!(names.contains(&"Alice Smith".to_string()));
        assert!(names.contains(&"Bob Brown".to_string()));
    }

    #[test]
    fn test_alias_detection() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "m1.md", MEETING_1);

        let meeting_sarah_only = r#"---
title: Quick Chat
type: meeting
date: 2026-03-23T09:00:00-07:00
duration: 15m
attendees: [Sarah]
tags: []
---
Short meeting.
"#;
        write_meeting(&meetings, "m2.md", meeting_sarah_only);

        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);

        assert!(
            stats.alias_suggestions.iter().any(|s| {
                (s.name_a == "Sarah Chen" && s.name_b == "Sarah")
                    || (s.name_a == "Sarah" && s.name_b == "Sarah Chen")
            }),
            "Expected alias suggestion for Sarah Chen / Sarah, got: {:?}",
            stats.alias_suggestions
        );
    }

    #[test]
    fn test_no_false_positive_aliases() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "m1.md", MEETING_1);

        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);

        assert!(
            !stats.alias_suggestions.iter().any(|s| {
                (s.name_a.contains("Sarah") && s.name_b.contains("Alex"))
                    || (s.name_a.contains("Alex") && s.name_b.contains("Sarah"))
            }),
            "False positive alias detected: {:?}",
            stats.alias_suggestions
        );
    }

    #[test]
    fn test_alias_clusters_full_rebuild() {
        // Full-path (#385): spelling-drift variants of one synthetic person
        // spread across DIFFERENT meetings (so shared_meetings is 0) must still
        // surface as one alias cluster, and a distinct person must not join it.
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();

        let mk = |date: &str, attendee: &str| {
            format!(
                "---\ntitle: Sync\ntype: meeting\ndate: {date}T09:00:00-07:00\nduration: 15m\nattendees: [{attendee}, Bright]\ntags: []\n---\nNotes.\n"
            )
        };
        write_meeting(&meetings, "d1.md", &mk("2026-03-21", "Tanvir"));
        write_meeting(&meetings, "d2.md", &mk("2026-03-22", "Tan-Vir"));
        write_meeting(&meetings, "d3.md", &mk("2026-03-23", "Tanmir"));

        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);

        let cluster = stats
            .alias_clusters
            .iter()
            .find(|c| {
                c.slugs.iter().any(|s| s == "tanvir")
                    || c.members.iter().any(|m| m.eq_ignore_ascii_case("tanvir"))
            })
            .unwrap_or_else(|| {
                panic!(
                    "expected a cluster containing the Tanvir drift variants, got: {:?}",
                    stats.alias_clusters
                )
            });

        let slugs: std::collections::HashSet<&String> = cluster.slugs.iter().collect();
        assert!(slugs.contains(&"tanvir".to_string()));
        assert!(slugs.contains(&"tan-vir".to_string()));
        assert!(slugs.contains(&"tanmir".to_string()));
        // The distinct attendee present in every meeting must NOT be in the cluster.
        assert!(
            !cluster
                .members
                .iter()
                .any(|m| m.eq_ignore_ascii_case("bright")),
            "distinct person leaked into drift cluster: {cluster:?}"
        );
    }

    #[test]
    fn test_confirmed_merge_collapses_variants_on_rebuild() {
        // #385 confirm-merge durability: a Person vocabulary entry (canonical +
        // variant aliases) must collapse the fragmented people into one on the
        // next rebuild, with all meetings re-pointed to the canonical slug. This
        // is the persistence contract the `minutes people merge` command relies on.
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let mk = |date: &str, attendee: &str| {
            format!(
                "---\ntitle: Sync\ntype: meeting\ndate: {date}T09:00:00-07:00\nduration: 15m\nattendees: [{attendee}]\ntags: []\n---\nNotes.\n"
            )
        };
        write_meeting(&meetings, "a.md", &mk("2026-03-21", "Junrei"));
        write_meeting(&meetings, "b.md", &mk("2026-03-22", "Junlei"));
        write_meeting(&meetings, "c.md", &mk("2026-03-23", "Jun-Rei"));

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");

        // Without a confirmed merge: three fragmented people.
        let stats = rebuild_index_at(&config, &db).unwrap();
        assert_eq!(stats.people_count, 3, "expected 3 fragments before merge");

        // Confirmed merge = a Person vocab entry (canonical + aliases), the exact
        // shape `minutes people merge` writes.
        let merged = vec![EntityRef {
            slug: "junrei".into(),
            label: "Junrei".into(),
            aliases: vec!["Junlei".into(), "Jun-Rei".into()],
        }];
        let stats = rebuild_index_at_with_vocabulary_entities(&config, &db, merged).unwrap();
        assert_eq!(
            stats.people_count, 1,
            "variants must collapse to one person"
        );

        let conn = open_db(&db).unwrap();
        let (slug, name, meetings_count): (String, String, i64) = conn
            .query_row("SELECT slug, name, meeting_count FROM people", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(slug, "junrei");
        assert_eq!(name, "Junrei");
        assert_eq!(
            meetings_count, 3,
            "all three meetings re-point to the canonical"
        );
    }

    #[test]
    fn test_alias_clusters_do_not_bridge_distinct_people() {
        // #385 codex: `Jon`~`Jan` (edit) must not transitively fuse with
        // `Jan`~`Jan Smith` (prefix) into one cluster. Prefix/last-name links
        // stay pairwise alias_suggestions; clusters use only the variant predicate.
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let mk = |date: &str, attendee: &str| {
            format!(
                "---\ntitle: Sync\ntype: meeting\ndate: {date}T09:00:00-07:00\nduration: 15m\nattendees: [{attendee}]\ntags: []\n---\nNotes.\n"
            )
        };
        write_meeting(&meetings, "b1.md", &mk("2026-03-21", "Jon"));
        write_meeting(&meetings, "b2.md", &mk("2026-03-22", "Jan"));
        write_meeting(&meetings, "b3.md", &mk("2026-03-23", "Jan Smith"));

        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);

        for c in &stats.alias_clusters {
            let has_jon = c.slugs.iter().any(|s| s == "jon");
            let has_jan_smith = c.slugs.iter().any(|s| s == "jan-smith");
            assert!(
                !(has_jon && has_jan_smith),
                "distinct people bridged into one cluster: {c:?}"
            );
        }
    }

    #[test]
    fn test_fix_frontmatter_date() {
        let fm = "title: Test\ntype: meeting\ndate: 2026-03-17T14:00:00\nduration: 5m";
        let fixed = fix_frontmatter(fm);
        let date = fixed
            .lines()
            .find_map(|line| line.strip_prefix("date: "))
            .expect("fixed frontmatter should include a date");
        let offset = &date[date.len().saturating_sub(6)..];
        let offset_bytes = offset.as_bytes();

        // Should have a local timezone offset, independent of the machine's zone.
        assert!(
            offset.len() == 6
                && matches!(offset_bytes[0], b'+' | b'-')
                && offset_bytes[1].is_ascii_digit()
                && offset_bytes[2].is_ascii_digit()
                && offset_bytes[3] == b':'
                && offset_bytes[4].is_ascii_digit()
                && offset_bytes[5].is_ascii_digit(),
            "Date should have timezone offset: {}",
            fixed
        );
    }

    #[test]
    fn test_fix_frontmatter_wikilinks() {
        let fm = "title: Test\ntype: meeting\ndate: 2026-03-17T14:00:00-07:00\nduration: 5m\npeople: [[alex-chen], [mat]]";
        let fixed = fix_frontmatter(fm);
        assert!(
            fixed.contains("people: [alex-chen, mat]"),
            "Wikilinks should be flattened: {}",
            fixed
        );
    }

    #[test]
    fn test_fix_frontmatter_due_string() {
        let fm = "  due: Friday";
        let fixed = fix_frontmatter(fm);
        assert!(
            fixed.contains("due: \"Friday\""),
            "Non-date due should be quoted: {}",
            fixed
        );
    }

    #[test]
    fn test_extract_dedup_person() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = "---\ntitle: Dedup Test\ntype: meeting\ndate: 2026-03-20T14:00:00-07:00\nduration: 10m\nattendees: [Sarah]\n---\n\n## Transcript\n[SARAH 0:00] Hello everyone\n";
        write_meeting(&meetings, "dedup.md", meeting);
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();
        let conn = open_db(&db).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = 'sarah'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "Sarah should appear once (deduped)");
    }

    #[test]
    fn test_canonicalizes_attendee_aliases_to_entity_slug() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = r#"---
title: Canonical Dan
type: meeting
date: 2026-03-20T14:00:00-07:00
duration: 10m
attendees: [Dan]
entities:
  people:
    - slug: dan-benamoz
      label: Dan Benamoz
      aliases: [Dan, dan]
action_items:
  - assignee: Dan
    task: Review extraction pass
    status: open
intents:
  - kind: commitment
    what: Follow up with Mat
    who: DAN
    status: open
---

## Transcript
[DAN 0:00] Happy to help
"#;
        write_meeting(&meetings, "canonical-dan.md", meeting);
        write_meeting(
            &meetings,
            "canonical-dan-followup.md",
            r#"---
title: Canonical Dan Follow-up
type: meeting
date: 2026-03-21T14:00:00-07:00
duration: 10m
attendees: [Danny]
entities:
  people:
    - slug: dan-benamoz
      label: Dan Benamoz
      aliases: [Danny]
---
"#,
        );
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();
        let conn = open_db(&db).unwrap();

        let canonical_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = 'dan-benamoz'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let alias_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM people WHERE slug = 'dan'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let (name, aliases): (String, String) = conn
            .query_row(
                "SELECT name, aliases FROM people WHERE slug = 'dan-benamoz'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let commitment_owner_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commitments c
                 JOIN people p ON c.person_id = p.id
                 WHERE p.slug = 'dan-benamoz'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(canonical_count, 1, "canonical person row should exist once");
        assert_eq!(
            alias_count, 0,
            "raw alias slug should not be written separately"
        );
        assert_eq!(name, "Dan Benamoz");
        assert!(aliases.contains("Dan"));
        assert!(
            aliases.contains("Danny"),
            "aliases from separate meetings must be unioned rather than selected by JSON length"
        );
        assert!(
            commitment_owner_count >= 2,
            "action items and intents should resolve to canonical person"
        );
    }

    #[test]
    fn test_vocabulary_person_aliases_canonicalize_graph_people() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = r#"---
title: Vocabulary Dan
type: meeting
date: 2026-03-20T14:00:00-07:00
duration: 10m
attendees: [Dan]
action_items:
  - assignee: Dan
    task: Review vocabulary plan
    status: open
---

## Transcript
[DAN 0:00] Happy to help
"#;
        write_meeting(&meetings, "vocabulary-dan.md", meeting);
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        let vocabulary_people = vec![EntityRef {
            slug: "dan-benamoz".into(),
            label: "Dan Benamoz".into(),
            aliases: vec!["Dan".into()],
        }];

        rebuild_index_at_with_vocabulary_entities(&config, &db, vocabulary_people).unwrap();
        let conn = open_db(&db).unwrap();

        let canonical_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = 'dan-benamoz'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let alias_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM people WHERE slug = 'dan'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let commitment_owner_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commitments c
                 JOIN people p ON c.person_id = p.id
                 WHERE p.slug = 'dan-benamoz'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(canonical_count, 1);
        assert_eq!(alias_count, 0);
        assert_eq!(commitment_owner_count, 1);
    }

    #[test]
    fn test_vocabulary_does_not_merge_different_full_name_by_first_name() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = r#"---
title: Sarah Miller Call
type: meeting
date: 2026-03-20T14:00:00-07:00
duration: 10m
attendees: [Sarah Miller]
---

## Transcript
[SARAH MILLER 0:00] Hello
"#;
        write_meeting(&meetings, "sarah-miller.md", meeting);
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        let vocabulary_people = vec![EntityRef {
            slug: "sarah-chen".into(),
            label: "Sarah Chen".into(),
            aliases: vec!["SC".into()],
        }];

        rebuild_index_at_with_vocabulary_entities(&config, &db, vocabulary_people).unwrap();
        let conn = open_db(&db).unwrap();

        let sarah_miller_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = 'sarah-miller'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let sarah_chen_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = 'sarah-chen'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(sarah_miller_count, 1);
        assert_eq!(
            sarah_chen_count, 0,
            "unused vocabulary entities must not be inserted into every meeting"
        );
    }

    #[test]
    fn test_commitment_staleness_detection() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = "---\ntitle: Stale Test\ntype: meeting\ndate: 2026-01-01T10:00:00-07:00\nduration: 30m\nattendees: [Alex]\nintents:\n  - kind: commitment\n    what: Deliver the report\n    who: Alex\n    status: open\n    by_date: \"2026-01-15\"\n---\nContent.\n";
        write_meeting(&meetings, "stale.md", meeting);
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();
        let conn = open_db(&db).unwrap();
        let stale: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commitments WHERE status = 'stale'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(stale >= 1, "Past-due commitment should be stale");
    }

    #[test]
    fn commitment_staleness_uses_local_end_of_day_and_absolute_timestamps() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-21T12:00:00Z")
            .unwrap()
            .with_timezone(&Local);
        assert_eq!(
            actionable_commitment_status("open", Some("2026-07-21"), now),
            Some("open")
        );
        assert_eq!(
            actionable_commitment_status("open", Some("2026-07-20"), now),
            Some("stale")
        );
        assert_eq!(
            actionable_commitment_status("open", Some("2026-07-21T23:30:00-07:00"), now,),
            Some("open")
        );
    }

    #[test]
    fn test_no_transcript_section() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = "---\ntitle: Memo Only\ntype: memo\ndate: 2026-03-20T10:00:00-07:00\nduration: 1m\ntags: [idea]\n---\n\n## Summary\nJust a summary.\n";
        write_meeting(&meetings, "memo.md", meeting);
        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);
        assert_eq!(stats.meeting_count, 1);
        assert_eq!(stats.people_count, 0);
        assert!(stats.topic_count >= 1); // "idea" from tags
    }

    #[test]
    fn role_suffix_on_entity_does_not_create_spurious_graph_node() {
        // Regression for issue #370: the LLM sometimes appends role context to
        // entity labels and slugs ("Junlei, tech lead" → slug "junlei-tech-lead").
        // After the fix, only the clean slug "junlei" should appear in the graph.
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = r#"---
title: Architecture Review
type: meeting
date: 2026-03-20T14:00:00-07:00
duration: 30m
attendees: [Junlei, Junrei]
entities:
  people:
    - slug: junlei-tech-lead
      label: "Junlei, tech lead"
      aliases: []
    - slug: junrei-core-team
      label: "Junrei (core team)"
      aliases: []
action_items:
  - assignee: Junlei
    task: Review the architecture doc
    due: "2026-03-25"
    status: open
---

## Transcript
[JUNLEI 0:00] Let's review the design.
[JUNREI 0:30] I agree with the approach.
"#;
        write_meeting(&meetings, "arch-review.md", meeting);
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();
        let conn = open_db(&db).unwrap();

        // Contaminated slugs must not appear in the people table
        let contaminated_junlei: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = 'junlei-tech-lead'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            contaminated_junlei, 0,
            "role-contaminated slug 'junlei-tech-lead' must not exist in the graph"
        );
        let contaminated_junrei: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = 'junrei-core-team'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            contaminated_junrei, 0,
            "role-contaminated slug 'junrei-core-team' must not exist in the graph"
        );

        // Clean slugs must be present
        let junlei: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = 'junlei'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(junlei, 1, "clean slug 'junlei' must exist in the graph");
        let junrei: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = 'junrei'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(junrei, 1, "clean slug 'junrei' must exist in the graph");

        // The action item assigned to "Junlei" must resolve to the clean slug
        let commitment_person_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commitments c
                 JOIN people p ON c.person_id = p.id
                 WHERE c.commitment_type = 'action_item' AND p.slug = 'junlei'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            commitment_person_count, 1,
            "action item must be linked to clean slug 'junlei'"
        );
    }

    #[test]
    fn test_corrupted_db_auto_rebuild() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "m1.md", MEETING_1);
        let db = tmp.path().join("graph.db");
        fs::write(&db, b"not a sqlite database").unwrap();
        let config = test_config(&meetings);
        let stats = rebuild_index_at(&config, &db).unwrap();
        assert_eq!(stats.meeting_count, 1);
        assert!(stats.people_count >= 2);
    }

    #[test]
    fn decisions_are_not_reported_as_actionable_commitments() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "m1.md", MEETING_1);
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();
        let conn = open_db(&db).unwrap();
        let decisions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commitments WHERE commitment_type = 'decision'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(decisions, 0, "Decisions are not actionable commitments");
    }

    #[test]
    fn normalize_boost_phrase_filters_placeholder_people() {
        assert!(normalize_boost_phrase("Speaker 1", Some("speaker-1")).is_none());
        assert!(normalize_boost_phrase("Unknown speaker", Some("unknown-speaker")).is_none());
        assert_eq!(
            normalize_boost_phrase("Matt Mullenweg", Some("matt-mullenweg")),
            Some("Matt Mullenweg".into())
        );
    }

    #[test]
    fn split_boost_title_fragments_keeps_high_signal_chunks() {
        let parts = split_boost_title_fragments("Wesley Asana, Box & X1 Integration");
        assert_eq!(
            parts,
            vec![
                "Wesley Asana".to_string(),
                "Box".to_string(),
                "X1 Integration".to_string()
            ]
        );
    }

    #[test]
    fn vocabulary_attestation_rejects_a_file_above_the_exact_byte_budget() {
        let tmp = TempDir::new().unwrap();
        let vocabulary = tmp.path().join("vocabulary.toml");
        let file = std::fs::File::create(&vocabulary).unwrap();
        file.set_len(MAX_CORRECTION_FILE_BYTES + 1).unwrap();
        let budget = ActiveCorpusReadBudget::new();
        let error = read_stable_correction_file(
            &vocabulary,
            &budget,
            std::time::Instant::now() + CORRECTION_READ_DEADLINE,
        )
        .unwrap_err();
        assert!(error.to_string().contains("byte budget"));
    }
}
